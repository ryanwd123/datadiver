use std::collections::HashSet;

use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, InteractiveElement, IntoElement, Keystroke, ParentElement, ScrollHandle,
    StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use super::font_picker::FontPreferences;
use crate::application::keymap::{ACTIONS, ActionCategory, KeyContext, KeymapFile};

const OUTER_MARGIN: f32 = 24.;
pub(super) const DIALOG_PADDING: f32 = 24.;
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
pub(super) struct HelpSection {
    category: ActionCategory,
    rows: Vec<HelpRow>,
}

pub(super) fn sections(keymap: &KeymapFile) -> Vec<HelpSection> {
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
                        | KeyContext::TableSpaceMenu
                        | KeyContext::QuerySpaceMenu
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

pub(super) struct HelpLayout {
    pub dialog_width: f32,
    column_count: usize,
    column_width: f32,
    key_width: f32,
}

impl HelpLayout {
    pub fn for_viewport(width: f32) -> Self {
        let dialog_width = (width - OUTER_MARGIN * 2.).clamp(0., 1500.);
        // The dialog's border and content padding reduce the usable width.
        let content_width = (dialog_width - DIALOG_PADDING * 2. - 2. - SCROLLBAR_GUTTER).max(0.);
        let column_count = (1..=3)
            .rev()
            .find(|&count| {
                content_width >= MIN_COLUMN_WIDTH * count as f32 + COLUMN_GAP * (count - 1) as f32
            })
            .unwrap_or(1);
        let column_width =
            (content_width - COLUMN_GAP * (column_count - 1) as f32) / column_count as f32;
        Self {
            dialog_width,
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

pub(super) fn render_help(
    sections: &[HelpSection],
    scroll_handle: &ScrollHandle,
    font: &FontPreferences,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let layout = HelpLayout::for_viewport(window.viewport_size().width.as_f32());
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let key_background = cx.theme().tokens.muted;
    let columns = distribute_sections(sections, layout.column_count);
    let font_size = px(f32::from(font.size));
    let font_family = font
        .family
        .clone()
        .map(Into::into)
        .unwrap_or_else(|| cx.theme().font_family.clone());
    // Keep the overlay outside the scrolling content so its track stays fixed while dragging.
    div()
        .id("help-viewport")
        .test_support()
        .min_h_0()
        .h(px((window.viewport_size().height.as_f32() - 140.).max(0.)))
        .max_h(px((window.viewport_size().height.as_f32() - 140.).max(0.)))
        .relative()
        .overflow_hidden()
        .text_size(font_size)
        .font_family(font_family.clone())
        .child(
            div()
                .id("help-body")
                .test_support()
                .size_full()
                .overflow_y_scroll()
                .track_scroll(scroll_handle)
                .pr(px(SCROLLBAR_GUTTER))
                .flex()
                .items_start()
                .gap(px(COLUMN_GAP))
                .children(columns.into_iter().enumerate().map(|(index, sections)| {
                    div()
                        .id(format!("help-column-{index}"))
                        .test_support()
                        .w(px(layout.column_width))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(24.))
                        .children(sections.into_iter().map(|section| {
                            let category = section.category;
                            div()
                                .id(format!("help-section-{category:?}"))
                                .test_support()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(div().font_bold().pb_2().child(category.label()))
                                .when(category == ActionCategory::SpaceMenu, |section| {
                                    section.child(
                                        div()
                                            .text_color(muted)
                                            .pb_2()
                                            .child("Keys apply after opening the Space menu."),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .gap_3()
                                        .pb_1()
                                        .border_b_1()
                                        .border_color(border)
                                        .font_bold()
                                        .text_color(muted)
                                        .child(
                                            div()
                                                .w(px(layout.key_width))
                                                .flex_shrink_0()
                                                .child("Key"),
                                        )
                                        .child(div().flex_1().min_w_0().child("Description")),
                                )
                                .children(section.rows.iter().enumerate().map(|(index, row)| {
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap_3()
                                        .py_1()
                                        .border_b_1()
                                        .border_color(border)
                                        .child(
                                            div()
                                                .id(format!("help-key-{category:?}-{index}"))
                                                .test_support()
                                                .w(px(layout.key_width))
                                                .flex_shrink_0()
                                                .flex()
                                                .flex_wrap()
                                                .gap_1()
                                                .children(row.keys.iter().map(|key| {
                                                    match key {
                                                        HelpKey::Stroke(stroke) => Kbd::new(
                                                            Keystroke::parse(stroke)
                                                                .expect("validated keymap stroke"),
                                                        )
                                                        .max_w_full()
                                                        .text_size(font_size)
                                                        .font_family(font_family.clone())
                                                        .font_bold()
                                                        .into_any_element(),
                                                        HelpKey::Range(label) => div()
                                                            .max_w_full()
                                                            .rounded_sm()
                                                            .px_1()
                                                            .py_0p5()
                                                            .font_bold()
                                                            .text_color(muted)
                                                            .bg(key_background)
                                                            .child(*label)
                                                            .into_any_element(),
                                                    }
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id(format!(
                                                    "help-description-{category:?}-{index}"
                                                ))
                                                .test_support()
                                                .flex_1()
                                                .min_w_0()
                                                .child(row.description)
                                                .when_some(row.context, |description, context| {
                                                    let label = match context {
                                                        KeyContext::TableSpaceMenu => "Table Grid",
                                                        KeyContext::QuerySpaceMenu => "Query Grid",
                                                        _ => context.label(),
                                                    };
                                                    description
                                                        .child(div().text_color(muted).child(label))
                                                }),
                                        )
                                }))
                        }))
                })),
        )
        .child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(scroll_handle)
                    .id("help-scrollbar")
                    .mode(ScrollbarMode::Always)
                    .viewport_from_layout(),
            ),
        )
        .into_any_element()
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
            (975., 1),
            (976., 2),
            (1200., 2),
            (1419., 2),
            (1420., 3),
            (1920., 3),
        ] {
            let layout = HelpLayout::for_viewport(width);
            assert_eq!(layout.column_count, count, "viewport {width}");
            assert!(layout.dialog_width <= width - 48.);
            assert!(layout.dialog_width <= 1500.);
            assert_eq!(layout.key_width, (layout.column_width * 0.42).min(190.));
            if count > 1 {
                assert!(layout.column_width >= MIN_COLUMN_WIDTH);
            }
        }
        assert_eq!(HelpLayout::for_viewport(300.).column_count, 1);
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
