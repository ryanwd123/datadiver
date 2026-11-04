//! The help window: F1 opens it on the page about what has focus, its tabs
//! and links move between pages, and Escape or Ctrl+W closes it.

use super::test_support::{Viewer, repository, sales_frame};
use crate::application::commands::CommandId;
use crate::ui::help_view::{HelpView, PAGES, SHORTCUTS};
use crate::ui::hit::Hit;
use crate::ui::platform::{Rect, point};

fn viewer(home: &tempfile::TempDir) -> Viewer {
    Viewer::open(sales_frame(), None, None, repository(home.path()))
}

/// The help window's viewer, once F1 or a command opened it.
fn help_window(viewer: &Viewer) -> Viewer {
    viewer.run_until_parked();
    let id = viewer.read(|view| view.help_window.as_ref().map(|window| window.id)).expect("the help opened");
    let help = viewer.window(id);
    help.render();
    help
}

fn help<R>(viewer: &Viewer, f: impl FnOnce(&HelpView) -> R) -> R {
    viewer.read(|view| f(&view.help_window.as_ref().expect("the help is open").view))
}

fn active(viewer: &Viewer) -> &'static str {
    help(viewer, |help| help.active_id())
}

fn is_open(viewer: &Viewer) -> bool {
    viewer.run_until_parked();
    viewer.read(|view| view.help_window.is_some())
}

fn find(viewer: &Viewer, hit: Hit) -> Rect {
    viewer.render();
    viewer.find(|found| *found == hit).unwrap_or_else(|| panic!("no {hit:?}"))
}

#[test]
fn f1_opens_a_window_on_the_page_for_focus_and_escape_or_ctrl_w_close_it() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.focus_grid();
    viewer.render();
    viewer.press("f1");
    let window = help_window(&viewer);
    assert_eq!(active(&viewer), "grid");
    // A tab for every page and the shortcuts.
    for index in 0..=PAGES.len() {
        find(&window, Hit::HelpTab(index));
    }
    // The grid's window has no help dialog over it.
    assert!(!viewer.has_dialog());

    window.press("ctrl-tab");
    assert_eq!(active(&viewer), "search");
    window.press("ctrl-shift-tab ctrl-shift-tab");
    assert_eq!(active(&viewer), "data");
    window.click(|hit| *hit == Hit::HelpTab(PAGES.len()));
    assert_eq!(active(&viewer), SHORTCUTS);

    window.press("escape");
    assert!(!is_open(&viewer));

    // F1 from the search box opens its page; Ctrl+W closes the window too.
    viewer.focus_search();
    viewer.render();
    viewer.press("f1");
    let window = help_window(&viewer);
    assert_eq!(active(&viewer), "search");
    window.press("ctrl-w");
    assert!(!is_open(&viewer));
}

#[test]
fn f1_again_turns_the_open_help_to_the_new_page() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.focus_grid();
    viewer.render();
    viewer.press("f1");
    let first = help_window(&viewer);
    viewer.focus_search();
    viewer.render();
    viewer.press("f1");
    let second = help_window(&viewer);
    assert_eq!(first.id, second.id);
    assert_eq!(active(&viewer), "search");
}

#[test]
fn f1_in_dialogs_the_sql_editor_and_ask_ai_opens_their_pages() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.focus_grid();
    viewer.render();
    viewer.press("g");
    viewer.run_until_parked();
    assert!(viewer.has_dialog());
    viewer.render();
    viewer.press("f1");
    help_window(&viewer);
    assert_eq!(active(&viewer), "pivot");
    // The dialog stays open behind the help.
    assert!(viewer.has_dialog());
    viewer.close_dialog();

    viewer.update(|view, cx| view.dispatch_command(CommandId::NewSqlSource, cx));
    viewer.run_until_parked();
    viewer.render();
    let editor = viewer.read(|view| view.sql_panel.as_ref().unwrap().editor.id);
    assert_eq!(viewer.focused(), Some(editor));
    viewer.press("f1");
    assert_eq!(active(&viewer), "sql");

    viewer.press("ctrl-i");
    viewer.render();
    let ask = viewer.read(|view| view.sql_panel.as_ref().unwrap().ask_input.id);
    assert_eq!(viewer.focused(), Some(ask));
    viewer.press("f1");
    assert_eq!(active(&viewer), "ask-ai");
}

#[test]
fn f1_in_the_formatting_window_opens_the_formatting_page() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_format_manager(cx));
    viewer.run_until_parked();
    let id = viewer.read(|view| view.format_window.as_ref().map(|window| window.id)).expect("the manager opened");
    let manager = viewer.window(id);
    manager.render();
    manager.press("f1");
    help_window(&viewer);
    assert_eq!(active(&viewer), "formatting");
    // The manager stays open beside the help.
    assert!(viewer.read(|view| view.format_window.is_some()));
}

#[test]
fn links_open_other_pages_at_their_heading() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_help_topic("start", cx));
    let window = help_window(&viewer);
    let link = (0..64)
        .find(|&index| help(&viewer, |help| help.page().and_then(|page| page.link(index)) == Some("data")))
        .expect("the start page links to Opening data");
    window.click(|hit| *hit == Hit::HelpLink(link));
    assert_eq!(active(&viewer), "data");

    // A link with a heading scrolls down to it.
    viewer.update(|view, cx| view.open_help_topic("slicers#filter-by-pasted-values", cx));
    window.render();
    assert!(help(&viewer, |help| help.page().unwrap().scroll.offset) > 0.0);
}

#[test]
fn pages_and_shortcuts_scroll_by_wheel_keys_and_bar() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_help_topic("sql", cx));
    let window = help_window(&viewer);
    window.resize(900., 600.);
    let offset = |viewer: &Viewer| help(viewer, |help| help.page().unwrap().scroll.offset);
    let body = find(&window, Hit::DialogScroll);
    window.app.scroll(window.id, body.center(), point(0., -3.));
    window.render();
    let wheeled = offset(&viewer);
    assert!(wheeled > 0.0);
    window.press("pagedown");
    window.render();
    assert!(offset(&viewer) > wheeled);
    window.press("home");
    window.render();
    assert_eq!(offset(&viewer), 0.0);

    // The shortcuts tab grows with the font and its bar drags.
    viewer.update(|view, cx| view.open_help_topic(SHORTCUTS, cx));
    window.render();
    let height = help(&viewer, |help| help.shortcuts().content_height());
    viewer.update(|view, cx| view.change_font_size(9, cx));
    window.render();
    assert!(help(&viewer, |help| help.shortcuts().content_height()) > height);
    let bar = find(&window, Hit::DialogBar);
    let from = point(bar.center().x, bar.y + 12.);
    window.drag(from, point(from.x, from.y + 60.));
    assert!(help(&viewer, |help| help.shortcuts().scroll.offset) > 0.0);
}

#[test]
fn every_topic_f1_can_open_names_a_page_and_heading() {
    let source = include_str!("help_window.rs");
    let start = source.find("fn help_topic").unwrap();
    let end = source[start..].find("fn help_window_closed").unwrap() + start;
    let mut help = HelpView::new(&Default::default());
    let mut topics = 0;
    for literal in source[start..end].split('"').skip(1).step_by(2) {
        topics += 1;
        assert!(help.show(literal), "no help page for {literal}");
        if let Some((_, anchor)) = literal.split_once('#') {
            let anchors = help.page().unwrap().anchors();
            assert!(anchors.iter().any(|a| a == anchor), "no heading #{anchor} for {literal}: {anchors:?}");
        }
    }
    assert!(topics > 10);
}
