use std::collections::HashSet;

use crate::application::keymap::{ACTIONS, ActionCategory, KeyContext, KeymapFile};
use crate::ui::hit::Hit;
use crate::ui::platform::{Align, Canvas, Font, Point, Rect, TextOptions, point, rect, text};
use crate::ui::widgets::{Fonts, colors, scroll};

const COLUMN_GAP: f32 = 24.;
const MIN_COLUMN_WIDTH: f32 = 420.;
const SCROLLBAR_GUTTER: f32 = 14.;

#[derive(Clone, Debug, PartialEq, Eq)]
enum HelpKey {
    Stroke(String),
    Range(&'static str),
}

#[derive(Clone, Debug)]
struct HelpRow {
    keys: Vec<HelpKey>,
    description: &'static str,
    context: Option<KeyContext>,
}

#[derive(Clone, Debug)]
pub struct HelpSection {
    category: ActionCategory,
    rows: Vec<HelpRow>,
}

pub(crate) fn sections(keymap: &KeymapFile) -> Vec<HelpSection> {
    let mut sections: Vec<_> = ActionCategory::ALL
        .into_iter()
        .map(|category| HelpSection {
            category,
            rows: Vec::new(),
        })
        .collect();
    let mut seen = HashSet::new();
    // An action listed in several contexts gets one row. Its first context
    // comes before its parents, so that row lists their keys too.
    for spec in ACTIONS {
        if !seen.insert(spec.id) {
            continue;
        }
        let mut seen_keys = HashSet::new();
        let keys: Vec<_> = keymap
            .bindings_for(spec.id, spec.context)
            .into_iter()
            .filter(|(stroke, _)| seen_keys.insert(stroke.to_ascii_lowercase()))
            .map(|(stroke, _)| HelpKey::Stroke(stroke))
            .collect();
        if keys.is_empty() {
            continue;
        }
        sections
            .iter_mut()
            .find(|section| section.category == spec.category)
            .expect("every action category has a section")
            .rows
            .push(HelpRow {
                keys,
                description: spec.name,
                context: matches!(
                    spec.context,
                    KeyContext::TableGrid
                        | KeyContext::QueryGrid
                        | KeyContext::TableCommandMenu
                        | KeyContext::QueryCommandMenu
                )
                .then_some(spec.context),
            });
    }
    let files = sections
        .iter_mut()
        .find(|section| section.category == ActionCategory::FilesViews)
        .unwrap();
    files.rows.extend([
        HelpRow {
            keys: vec![HelpKey::Range("Ctrl+1–9")],
            description: "Select favorite",
            context: None,
        },
        HelpRow {
            keys: vec![HelpKey::Range("Ctrl+Shift+1–9")],
            description: "Set favorite",
            context: None,
        },
        HelpRow {
            keys: vec![HelpKey::Stroke("ctrl-delete".into())],
            description: "Archive selected saved view in Files",
            context: None,
        },
    ]);
    sections.retain(|section| !section.rows.is_empty());
    sections
}

pub(crate) struct HelpLayout {
    pub column_count: usize,
    column_width: f32,
    key_width: f32,
}

impl HelpLayout {
    /// Columns for a page `width` wide, which leaves room for its scroll bar.
    pub fn for_width(width: f32) -> Self {
        let content_width = (width.min(1450.) - SCROLLBAR_GUTTER).max(0.);
        let column_count = (1..=3)
            .rev()
            .find(|&count| {
                content_width >= MIN_COLUMN_WIDTH * count as f32 + COLUMN_GAP * (count - 1) as f32
            })
            .unwrap_or(1);
        let column_width =
            (content_width - COLUMN_GAP * (column_count - 1) as f32) / column_count as f32;
        Self {
            column_count,
            column_width,
            key_width: (column_width * 0.42).min(190.),
        }
    }
}

fn distribute_sections(sections: &[HelpSection], count: usize) -> Vec<Vec<&HelpSection>> {
    let mut columns = vec![Vec::new(); count];
    let mut loads = vec![0; count];
    for section in sections {
        let column = (0..count).min_by_key(|&index| loads[index]).unwrap();
        loads[column] += section.rows.len() + 2;
        columns[column].push(section);
    }
    columns
}

/// The keyboard shortcut reference, laid out in columns that scroll together:
/// the help window's last tab.
pub struct ShortcutsPage {
    sections: Vec<HelpSection>,
    pub(super) scroll: scroll::Scroll,
    viewport: Rect,
    content_height: f32,
}

enum Op {
    Title(&'static str),
    Header(&'static str),
    Muted(String),
    Rule,
    Key(String, bool),
    Text(String),
}

impl ShortcutsPage {
    pub fn new(keymap: &KeymapFile) -> Self {
        Self {
            sections: sections(keymap),
            scroll: scroll::Scroll::default(),
            viewport: Rect::default(),
            content_height: 0.0,
        }
    }

    #[cfg(test)]
    pub(crate) fn content_height(&self) -> f32 {
        self.content_height
    }

    pub fn wheel(&mut self, delta: Point, line: f32) -> bool {
        self.scroll
            .scroll_by(-delta.y * line * 2.0, self.viewport.h, self.content_height)
    }

    pub fn key(&mut self, key: &str, line: f32) -> bool {
        let page = (self.viewport.h - line).max(line);
        let delta = match key {
            "down" => line * 2.0,
            "up" => -line * 2.0,
            "pagedown" | "space" => page,
            "pageup" => -page,
            "home" => -self.content_height,
            "end" => self.content_height,
            _ => return false,
        };
        self.scroll.scroll_by(delta, self.viewport.h, self.content_height);
        true
    }

    pub fn press_bar(&mut self, pos: Point) {
        let bar = self.bar();
        self.scroll.press(pos.y - bar.y, bar.h, self.viewport.h, self.content_height);
    }

    pub fn drag(&mut self, pos: Point) {
        let bar = self.bar();
        self.scroll.drag(pos.y - bar.y, bar.h, self.viewport.h, self.content_height);
    }

    pub fn release(&mut self) {
        self.scroll.release();
    }

    fn bar(&self) -> Rect {
        rect(
            self.viewport.right() - scroll::BAR_WIDTH,
            self.viewport.y,
            scroll::BAR_WIDTH,
            self.viewport.h,
        )
    }

    /// Lays out one section; returns its height, drawing it at `origin` when
    /// a canvas is given.
    fn section(
        section: &HelpSection,
        layout: &HelpLayout,
        font: &Font,
        canvas: Option<(&mut Canvas<Hit>, Point)>,
    ) -> f32 {
        let line = text::line_height(font);
        let bold = font.bold();
        let width = layout.column_width;
        let description_width = (width - layout.key_width - 12.0).max(40.0);
        let mut ops: Vec<(Rect, Op)> = Vec::new();
        let mut y = 0.0;
        ops.push((rect(0.0, y, width, line), Op::Title(section.category.label())));
        y += line + 8.0;
        if section.category == ActionCategory::CommandMenu {
            ops.push((
                rect(0.0, y, width, line),
                Op::Muted("Keys work while the Command menu is open. Space opens the Helper menu, which lists the grid's keys.".into()),
            ));
            y += line + 8.0;
        }
        ops.push((rect(0.0, y, layout.key_width, line), Op::Header("Key")));
        ops.push((
            rect(layout.key_width + 12.0, y, description_width, line),
            Op::Header("Description"),
        ));
        y += line + 4.0;
        ops.push((rect(0.0, y, width, 1.0), Op::Rule));
        y += 1.0;
        for row in &section.rows {
            let top = y + 4.0;
            // Key caps wrap within the key column.
            let mut x = 0.0;
            let mut key_y = top;
            let cap_height = line + 4.0;
            for key in &row.keys {
                let label = match key {
                    HelpKey::Stroke(stroke) => crate::ui::platform::keys::display(stroke),
                    HelpKey::Range(label) => (*label).to_owned(),
                };
                let cap_width = (text::measure(&label, &bold).0 + 10.0).min(layout.key_width);
                if x > 0.0 && x + cap_width > layout.key_width {
                    x = 0.0;
                    key_y += cap_height + 4.0;
                }
                ops.push((
                    rect(x, key_y, cap_width, cap_height),
                    Op::Key(label, matches!(key, HelpKey::Range(_))),
                ));
                x += cap_width + 4.0;
            }
            let keys_height = key_y + cap_height - top;
            let description = text::layout(row.description, font, TextOptions::wrapped(description_width));
            let mut description_height = description.height;
            ops.push((
                rect(layout.key_width + 12.0, top, description_width, description.height),
                Op::Text(row.description.to_owned()),
            ));
            if let Some(context) = row.context {
                let label = match context {
                    KeyContext::TableCommandMenu => "Table Grid",
                    KeyContext::QueryCommandMenu => "Query Grid",
                    _ => context.label(),
                };
                ops.push((
                    rect(layout.key_width + 12.0, top + description_height, description_width, line),
                    Op::Muted(label.into()),
                ));
                description_height += line;
            }
            y = top + keys_height.max(description_height) + 4.0;
            ops.push((rect(0.0, y, width, 1.0), Op::Rule));
            y += 1.0;
        }
        if let Some((canvas, origin)) = canvas {
            for (r, op) in ops {
                let r = r.offset(origin.x, origin.y);
                match op {
                    Op::Title(title) => {
                        canvas.text(r, title, &bold, colors::FOREGROUND, Align::Left);
                    }
                    Op::Header(label) => {
                        canvas.text(r, label, &bold, colors::MUTED, Align::Left);
                    }
                    Op::Muted(label) => {
                        canvas.text(r, &label, font, colors::MUTED, Align::Left);
                    }
                    Op::Rule => canvas.fill(r, colors::BORDER),
                    Op::Key(label, range) => {
                        canvas.fill_rounded(r, 4.0, colors::KEY_BACKGROUND);
                        if !range {
                            canvas.stroke_rounded(r, 4.0, colors::BORDER, 1.0);
                        }
                        canvas.text(
                            r,
                            &label,
                            &bold,
                            if range { colors::MUTED } else { colors::FOREGROUND },
                            Align::Center,
                        );
                    }
                    Op::Text(label) => {
                        let layout = text::layout(&label, font, TextOptions::wrapped(r.w));
                        canvas.draw_layout(&layout, r.origin(), colors::FOREGROUND);
                    }
                }
            }
        }
        y
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts) {
        let layout = HelpLayout::for_width(r.w);
        let font = &fonts.ui;
        self.viewport = r;
        let columns = distribute_sections(&self.sections, layout.column_count);
        let heights = columns
            .iter()
            .map(|sections| {
                sections
                    .iter()
                    .map(|section| Self::section(section, &layout, font, None) + 24.0)
                    .sum::<f32>()
            })
            .collect::<Vec<_>>();
        self.content_height = heights.iter().copied().fold(0.0, f32::max);
        self.scroll.clamp(r.h, self.content_height);
        canvas.push_clip(r);
        for (index, sections) in columns.iter().enumerate() {
            let x = r.x + index as f32 * (layout.column_width + COLUMN_GAP);
            let mut y = r.y - self.scroll.offset;
            for section in sections {
                let height = Self::section(section, &layout, font, None);
                if y + height >= r.y && y <= r.bottom() {
                    Self::section(section, &layout, font, Some((&mut *canvas, point(x, y))));
                }
                y += height + 24.0;
            }
        }
        canvas.pop_clip();
        canvas.hit(r, Hit::DialogScroll);
        scroll::vertical(canvas, self.bar(), r.h, self.content_height, &self.scroll, Hit::DialogBar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::keymap::ActionId;

    fn row<'a>(sections: &'a [HelpSection], description: &str) -> Option<&'a HelpRow> {
        sections
            .iter()
            .flat_map(|section| &section.rows)
            .find(|row| row.description == description)
    }

    #[test]
    fn defaults_are_categorized_once_in_registry_order_with_contexts_and_supplements() {
        let sections = sections(&KeymapFile::default());
        assert_eq!(
            sections
                .iter()
                .map(|section| section.category)
                .collect::<Vec<_>>(),
            ActionCategory::ALL
        );
        assert_eq!(sections[0].rows[0].description, "Move cell left");
        assert_eq!(
            sections
                .iter()
                .flat_map(|section| &section.rows)
                .filter(|row| row.description.eq_ignore_ascii_case("Add slicer"))
                .count(),
            2
        );
        // One direct binding and one distinct Space-menu binding.
        assert_eq!(
            sections[2]
                .rows
                .iter()
                .filter(|row| row.description.eq_ignore_ascii_case("Add slicer"))
                .count(),
            1
        );
        assert!(row(&sections, "Font picker").is_none());
        assert_eq!(
            row(&sections, "Remove active group-by column")
                .unwrap()
                .context,
            Some(KeyContext::QueryGrid)
        );
        assert_eq!(
            row(&sections, "Move column left").unwrap().context,
            Some(KeyContext::TableGrid)
        );
        // ctrl-f is bound in Application and / in Grid; one row lists both.
        assert_eq!(
            sections
                .iter()
                .flat_map(|section| &section.rows)
                .filter(|row| row.description == "Focus search")
                .count(),
            1
        );
        assert_eq!(
            row(&sections, "Focus search").unwrap().keys,
            [
                HelpKey::Stroke("ctrl-f".into()),
                HelpKey::Stroke("/".into())
            ]
        );
        let files = &sections[5];
        assert_eq!(
            files.rows[files.rows.len() - 3].keys,
            [HelpKey::Range("Ctrl+1–9")]
        );
        assert_eq!(
            files.rows[files.rows.len() - 2].keys,
            [HelpKey::Range("Ctrl+Shift+1–9")]
        );
        assert_eq!(
            files.rows.last().unwrap().keys,
            [HelpKey::Stroke("ctrl-delete".into())]
        );
    }

    #[test]
    fn help_reflects_added_and_removed_bindings_and_hides_empty_categories() {
        let mut map = KeymapFile::default();
        map.add(
            ActionId::Fonts,
            KeyContext::Application,
            "ctrl-alt-f".into(),
            false,
        )
        .unwrap();
        map.add(ActionId::Info, KeyContext::Grid, "ctrl-i".into(), false)
            .unwrap();
        map.remove(ActionId::Info, KeyContext::Grid, "i");
        let updated = sections(&map);
        assert_eq!(
            row(&updated, "Font picker").unwrap().keys,
            [HelpKey::Stroke("ctrl-alt-f".into())]
        );
        assert_eq!(
            row(&updated, "Information").unwrap().keys,
            [HelpKey::Stroke("ctrl-i".into())]
        );
        for spec in ACTIONS
            .iter()
            .filter(|spec| spec.category == ActionCategory::CopyExport)
        {
            for binding in spec.default_bindings {
                map.remove(spec.id, spec.context, binding);
            }
        }
        let updated = sections(&map);
        assert!(
            !updated
                .iter()
                .any(|section| section.category == ActionCategory::CopyExport)
        );
        assert!(row(&updated, "Copy selection").is_none());
    }

    #[test]
    fn columns_fit_the_content_area_at_each_threshold() {
        for (width, count) in [
            (800., 1),
            (877., 1),
            (878., 2),
            (1200., 2),
            (1321., 2),
            (1322., 3),
            (1920., 3),
        ] {
            let layout = HelpLayout::for_width(width);
            assert_eq!(layout.column_count, count, "width {width}");
            assert!(layout.column_width * count as f32 <= width);
            assert_eq!(layout.key_width, (layout.column_width * 0.42).min(190.));
            if count > 1 {
                assert!(layout.column_width >= MIN_COLUMN_WIDTH);
            }
        }
        assert_eq!(HelpLayout::for_width(300.).column_count, 1);
    }

    #[test]
    fn sections_stay_whole_and_balance_with_leftmost_tie_breaks() {
        let sections = sections(&KeymapFile::default());
        let columns = distribute_sections(&sections, 3);
        assert_eq!(columns[0][0].category, ActionCategory::Navigation);
        assert_eq!(columns[1][0].category, ActionCategory::Selection);
        assert_eq!(columns[2][0].category, ActionCategory::DataExploration);
        let mut loads = [0; 3];
        for section in &sections {
            let expected = (0..3).min_by_key(|&index| loads[index]).unwrap();
            assert!(
                columns[expected]
                    .iter()
                    .any(|candidate| std::ptr::eq(*candidate, section))
            );
            loads[expected] += section.rows.len() + 2;
        }
        assert_eq!(columns.iter().map(Vec::len).sum::<usize>(), sections.len());
        assert_eq!(
            distribute_sections(&sections, 1)[0]
                .iter()
                .map(|section| section.category)
                .collect::<Vec<_>>(),
            ActionCategory::ALL
        );
    }
}
