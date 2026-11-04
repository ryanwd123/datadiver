//! The help window's contents: a tab per topic across the top, each a
//! Markdown page from `help/`, and a last tab listing the keyboard shortcuts
//! as they are bound now.

use crate::application::keymap::KeymapFile;
use crate::ui::help::ShortcutsPage;
use crate::ui::hit::Hit;
use crate::ui::markdown_view::MarkdownView;
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Cursor, Point, Rect, rect, system, text};
use crate::ui::widgets::{Fonts, colors};

pub struct HelpPage {
    /// What links and F1 topics call the page.
    pub id: &'static str,
    pub title: &'static str,
    pub markdown: &'static str,
}

pub const PAGES: &[HelpPage] = &[
    HelpPage { id: "start", title: "Getting started", markdown: include_str!("../../help/start.md") },
    HelpPage { id: "data", title: "Opening data", markdown: include_str!("../../help/data.md") },
    HelpPage { id: "grid", title: "Grid", markdown: include_str!("../../help/grid.md") },
    HelpPage { id: "search", title: "Search", markdown: include_str!("../../help/search.md") },
    HelpPage { id: "slicers", title: "Slicers & filters", markdown: include_str!("../../help/slicers.md") },
    HelpPage { id: "pivot", title: "Group & pivot", markdown: include_str!("../../help/pivot.md") },
    HelpPage { id: "formatting", title: "Formatting", markdown: include_str!("../../help/formatting.md") },
    HelpPage { id: "sql", title: "SQL editor", markdown: include_str!("../../help/sql.md") },
    HelpPage { id: "ask-ai", title: "Ask AI", markdown: include_str!("../../help/ask-ai.md") },
    HelpPage { id: "views", title: "Views & actions", markdown: include_str!("../../help/views.md") },
    HelpPage { id: "settings", title: "Settings", markdown: include_str!("../../help/settings.md") },
];

/// The shortcuts tab, after the pages.
pub const SHORTCUTS: &str = "shortcuts";
const SHORTCUTS_TITLE: &str = "Shortcuts";

const TAB_GAP: f32 = 4.0;

pub struct HelpView {
    /// The open tab: a page's index, or `PAGES.len()` for the shortcuts.
    pub active: usize,
    pages: Vec<MarkdownView>,
    shortcuts: ShortcutsPage,
    tabs: Vec<Rect>,
    body: Rect,
}

impl HelpView {
    pub fn new(keymap: &KeymapFile) -> Self {
        Self {
            active: 0,
            pages: PAGES.iter().map(|page| MarkdownView::new(page.markdown)).collect(),
            shortcuts: ShortcutsPage::new(keymap),
            tabs: Vec::new(),
            body: Rect::default(),
        }
    }

    fn tab_count(&self) -> usize {
        PAGES.len() + 1
    }

    /// The id of the open tab.
    #[cfg(test)]
    pub fn active_id(&self) -> &'static str {
        PAGES.get(self.active).map_or(SHORTCUTS, |page| page.id)
    }

    #[cfg(test)]
    pub fn shortcuts(&self) -> &ShortcutsPage {
        &self.shortcuts
    }

    /// The open page, unless the shortcuts are showing.
    pub fn page(&self) -> Option<&MarkdownView> {
        self.pages.get(self.active)
    }

    /// Shows `topic`: a page id, optionally with `#heading`, or `shortcuts`.
    /// Returns whether it names a page.
    pub fn show(&mut self, topic: &str) -> bool {
        let (id, anchor) = topic.split_once('#').unwrap_or((topic, ""));
        let index = if id == SHORTCUTS {
            Some(PAGES.len())
        } else if id.is_empty() {
            Some(self.active)
        } else {
            PAGES.iter().position(|page| page.id == id)
        };
        let Some(index) = index else { return false };
        self.active = index;
        if let Some(page) = self.pages.get_mut(index) {
            if anchor.is_empty() {
                page.scroll_to_top();
            } else {
                page.scroll_to(anchor);
            }
        }
        true
    }

    /// Moves to the next tab, or the previous one with `back`, wrapping.
    pub fn cycle(&mut self, back: bool) {
        let count = self.tab_count();
        self.active = if back { (self.active + count - 1) % count } else { (self.active + 1) % count };
    }

    /// Keys for the help: switching tabs and scrolling. Escape and Ctrl+W
    /// are the window's.
    pub fn key(&mut self, event: &KeyEvent, line: f32) -> bool {
        let m = event.modifiers;
        let key = event.key.as_str();
        match key {
            "tab" if m.control => self.cycle(m.shift),
            "pagedown" if m.control => self.cycle(false),
            "pageup" if m.control => self.cycle(true),
            "left" if !m.modified() => self.cycle(true),
            "right" if !m.modified() => self.cycle(false),
            _ if m.control || m.alt => return false,
            _ => {
                return match self.pages.get_mut(self.active) {
                    Some(page) => page.key(key, line),
                    None => self.shortcuts.key(key, line),
                };
            }
        }
        true
    }

    pub fn mouse_down(&mut self, hit: Option<&Hit>, pos: Point) -> bool {
        match hit {
            Some(Hit::HelpTab(index)) => {
                self.active = (*index).min(self.tab_count() - 1);
                true
            }
            Some(Hit::HelpLink(index)) => {
                let Some(target) = self.page().and_then(|page| page.link(*index)).map(str::to_string) else {
                    return false;
                };
                if target.starts_with("http://") || target.starts_with("https://") {
                    system::open_url(&target);
                    return true;
                }
                self.show(&target)
            }
            Some(Hit::DialogBar) => {
                match self.pages.get_mut(self.active) {
                    Some(page) => page.press_bar(pos),
                    None => self.shortcuts.press_bar(pos),
                }
                true
            }
            _ => false,
        }
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        match self.pages.get_mut(self.active) {
            Some(page) => page.drag(pos),
            None => self.shortcuts.drag(pos),
        }
    }

    pub fn mouse_up(&mut self) {
        match self.pages.get_mut(self.active) {
            Some(page) => page.release(),
            None => self.shortcuts.release(),
        }
    }

    pub fn wheel(&mut self, delta: Point, line: f32) -> bool {
        match self.pages.get_mut(self.active) {
            Some(page) => page.wheel(delta, line),
            None => self.shortcuts.wheel(delta, line),
        }
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts) {
        canvas.fill(r, colors::BACKGROUND);
        let font = &fonts.ui;
        let line = text::line_height(font);
        let tab_height = (line + 16.0).round();
        // Tabs wrap onto more rows in a narrow window.
        let mut x = r.x + 16.0;
        let mut y = r.y + 10.0;
        self.tabs.clear();
        let titles = PAGES.iter().map(|page| page.title).chain([SHORTCUTS_TITLE]);
        for title in titles {
            let width = text::measure(title, font).0 + 20.0;
            if x > r.x + 16.0 && x + width > r.right() - 16.0 {
                x = r.x + 16.0;
                y += tab_height + TAB_GAP;
            }
            self.tabs.push(rect(x, y, width, tab_height));
            x += width + TAB_GAP;
        }
        let strip_bottom = y + tab_height + 8.0;
        canvas.fill(rect(r.x, r.y, r.w, strip_bottom - r.y), colors::SIDEBAR);
        canvas.fill(rect(r.x, strip_bottom - 1.0, r.w, 1.0), colors::BORDER);
        let titles = PAGES.iter().map(|page| page.title).chain([SHORTCUTS_TITLE]);
        for (index, (tab, title)) in self.tabs.clone().into_iter().zip(titles).enumerate() {
            let active = index == self.active;
            if active {
                canvas.fill_rounded(tab, 5.0, colors::SELECTED);
            }
            let label_font = if active { font.semibold() } else { font.clone() };
            canvas.text(tab, title, &label_font, if active { colors::SELECTED_STRONG } else { colors::FOREGROUND }, Align::Center);
            canvas.hit_with(tab, Hit::HelpTab(index), Cursor::Hand, None);
        }
        let body = rect(r.x, strip_bottom, r.w, r.bottom() - strip_bottom);
        self.body = body;
        match self.pages.get_mut(self.active) {
            Some(page) => page.paint(canvas, body, fonts),
            None => {
                let inner = rect(body.x + 24.0, body.y + 16.0, body.w - 36.0, body.h - 16.0);
                self.shortcuts.paint(canvas, inner, fonts);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_link_goes_somewhere() {
        let views: Vec<MarkdownView> = PAGES.iter().map(|page| MarkdownView::new(page.markdown)).collect();
        for (page, view) in PAGES.iter().zip(&views) {
            for block in code_editor::markdown::parse(page.markdown) {
                let inlines: Vec<code_editor::markdown::Inline> = match block {
                    code_editor::markdown::Block::Heading(_, i)
                    | code_editor::markdown::Block::Paragraph(i)
                    | code_editor::markdown::Block::Quote(i)
                    | code_editor::markdown::Block::Item { text: i, .. } => vec![i],
                    code_editor::markdown::Block::Table { header, rows } => {
                        header.into_iter().chain(rows.into_iter().flatten()).collect()
                    }
                    _ => Vec::new(),
                };
                for inline in inlines {
                    for (_, target) in inline.links {
                        if target.starts_with("http") {
                            continue;
                        }
                        let (id, anchor) = target.split_once('#').unwrap_or((&target, ""));
                        if id == SHORTCUTS {
                            continue;
                        }
                        let index = if id.is_empty() {
                            PAGES.iter().position(|p| p.id == page.id)
                        } else {
                            PAGES.iter().position(|p| p.id == id)
                        };
                        let index = index.unwrap_or_else(|| panic!("{}: no page {id}", page.id));
                        assert!(
                            anchor.is_empty() || views[index].anchors().iter().any(|a| a == anchor),
                            "{}: no heading #{anchor} on {}",
                            page.id,
                            PAGES[index].id
                        );
                    }
                }
            }
            assert!(!view.anchors().is_empty(), "{} has no headings", page.id);
        }
    }

    #[test]
    fn topics_open_their_page_and_unknown_ones_are_refused() {
        let mut help = HelpView::new(&KeymapFile::default());
        assert!(help.show("sql"));
        assert_eq!(help.active_id(), "sql");
        assert!(help.show(SHORTCUTS));
        assert_eq!(help.active_id(), SHORTCUTS);
        assert!(!help.show("nowhere"));
        assert_eq!(help.active_id(), SHORTCUTS);
        help.cycle(false);
        assert_eq!(help.active_id(), "start");
        help.cycle(true);
        assert_eq!(help.active_id(), SHORTCUTS);
    }
}
