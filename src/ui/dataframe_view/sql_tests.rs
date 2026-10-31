use super::FileEntry;
use super::dialogs::{Dialog, PaletteKind};
use super::view_options::ViewOption;
use super::test_support::{Viewer, repository};
use crate::application::commands::CommandId;
use crate::application::session::SessionIdentity;
use crate::domain::documents::{DocumentDefinition, SqlConfig, ViewKind};
use crate::domain::frame::FrameSnapshot;
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::hit::{Button, Hit, Splitter};
use crate::ui::sql_panel::{AskState, SqlPanel};
use std::time::Duration;

fn viewer(repository: ViewRepository, document: Option<DocumentDefinition>) -> Viewer {
    let store = crate::infrastructure::duckdb_store::DuckDbStore::synthetic(2).unwrap();
    Viewer::open(FrameSnapshot::new(store, 1), None, document, repository)
}

fn panel<R>(viewer: &Viewer, f: impl FnOnce(&SqlPanel) -> R) -> R {
    viewer.read(|view| f(view.sql_panel.as_ref().expect("the SQL panel")))
}

/// Replaces the editor's SQL as if typed.
fn edit_sql(viewer: &Viewer, text: &str) {
    viewer.update(|view, _| view.sql_panel.as_mut().unwrap().edit(text));
    viewer.run_until_parked();
}

fn sql_text(viewer: &Viewer) -> String {
    panel(viewer, |panel| panel.editor.text().to_owned())
}

fn editor_focused(viewer: &Viewer) -> bool {
    viewer.read(|view| view.focus == Some(view.sql_panel.as_ref().unwrap().editor.id))
}

fn ask_focused(viewer: &Viewer) -> bool {
    viewer.read(|view| view.focus == Some(view.sql_panel.as_ref().unwrap().ask_input.id))
}

/// Sets the SQL, puts the cursor at `selection`, and focuses the editor.
fn place(viewer: &Viewer, text: &str, selection: std::ops::Range<usize>) {
    viewer.update(|view, _| {
        let panel = view.sql_panel.as_mut().unwrap();
        panel.editor.set_text(text);
        panel.editor.set_selection(selection);
        let mut focus = view.focus;
        panel.focus(&mut focus);
        view.focus = focus;
    });
    viewer.render();
}

fn completion_labels(viewer: &Viewer) -> Option<Vec<String>> {
    panel(viewer, |panel| panel.editor.completion_labels())
}

fn click(viewer: &Viewer, button: Button) {
    viewer.click(|hit| *hit == Hit::Button(button.clone()));
    viewer.run_until_parked();
}

/// Starts an empty SQL source, as New SQL source does.
fn new_source(viewer: &Viewer) {
    viewer.update(|view, cx| view.dispatch_command(CommandId::NewSqlSource, cx));
    viewer.run_until_parked();
}

/// Switches a View option from the gear, then closes the list unless the
/// switch opened another dialog.
fn switch(viewer: &Viewer, option: ViewOption) {
    viewer.view_option(option);
    if viewer.read(|view| matches!(&view.dialog, Some(Dialog::Palette(dialog)) if matches!(dialog.kind, PaletteKind::ViewOptions))) {
        viewer.close_dialog();
    }
}

fn press(viewer: &Viewer, keys: &str) {
    viewer.render();
    viewer.press(keys);
    viewer.run_until_parked();
}

fn table_count(viewer: &Viewer, names: &str) -> i64 {
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .query_row(
                &format!("SELECT count(*) FROM duckdb_tables() WHERE table_name IN ({names})"),
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    })
}

fn sql_config(viewer: &Viewer) -> SqlConfig {
    viewer.read(|view| match &view.session.working.as_ref().unwrap().view {
        ViewKind::Sql(config) => config.clone(),
        _ => panic!("not a SQL view"),
    })
}

fn run_all_sql(viewer: &Viewer, sql: &str) {
    edit_sql(viewer, sql);
    viewer.update(|view, cx| view.run_sql(true, cx));
    viewer.run_until_parked();
}

fn saved_sql(repository: &ViewRepository) -> SqlConfig {
    match &repository.load_views().0[0].view {
        ViewKind::Sql(config) => config.clone(),
        _ => panic!("not a SQL view"),
    }
}

fn sql_view(name: &str, config: SqlConfig) -> DocumentDefinition {
    let mut document = DocumentDefinition::draft_table(name.into(), String::new());
    document.view = ViewKind::Sql(config);
    document
}

#[test]
fn startup_sql_command_saves_plain_file_autosaves_and_runs_only_on_request() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository.clone(), None);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("startup sql");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert!(repository.startup_sql_path().is_file());
    viewer.render();
    viewer.read(|view| {
        assert_eq!(view.session.identity, SessionIdentity::StartupSql);
        assert!(!view.session.dirty);
    });
    assert!(editor_focused(&viewer));
    assert!(viewer.find(|hit| *hit == Hit::Button(Button::SqlSave)).is_some());
    let sql = "CREATE TABLE should_not_auto_run(id INT);\nSELECT 42 AS answer;";
    edit_sql(&viewer, sql);
    viewer.press("ctrl-s");
    assert!(!viewer.has_dialog());
    viewer.run_until_parked();
    assert_eq!(std::fs::read_to_string(repository.startup_sql_path()).unwrap(), sql);
    assert!(repository.load_views().0.is_empty());
    switch(&viewer, ViewOption::AutoSave);
    assert!(repository.load_settings().unwrap().startup_sql_auto_save);
    let edited = "CREATE TABLE manual_run(id INT); SELECT 7 AS answer;";
    edit_sql(&viewer, edited);
    viewer.advance_clock(Duration::from_millis(1999));
    assert_eq!(std::fs::read_to_string(repository.startup_sql_path()).unwrap(), sql);
    viewer.advance_clock(Duration::from_millis(1));
    assert_eq!(std::fs::read_to_string(repository.startup_sql_path()).unwrap(), edited);
    assert_eq!(table_count(&viewer, "'should_not_auto_run', 'manual_run'"), 0);
    click(&viewer, Button::SqlRunAll);
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
}

#[test]
fn startup_sql_is_absent_from_picker_and_command_history_keeps_save_destination() {
    use super::dialogs::Dialog;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    repository.save_startup_sql("SELECT * FROM missing_table;").unwrap();
    let viewer = viewer(repository.clone(), None);
    assert!(viewer.read(|view| view.error.as_ref().unwrap().contains("duckdb_startup.sql")));
    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("duckdb_startup.sql");
    viewer.run_until_parked();
    viewer.render();
    viewer.read(|view| {
        assert!(
            view.file_groups
                .iter()
                .flatten()
                .all(|entry| !matches!(entry, FileEntry::StartupSql))
        );
        let Some(Dialog::Palette(dialog)) = &view.dialog else {
            panic!("the file palette is open")
        };
        assert_eq!(dialog.palette.list.visible_count(), 0);
    });
    viewer.close_dialog();
    viewer.update(|view, cx| view.dispatch_command(CommandId::StartupSql, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.identity, SessionIdentity::StartupSql);
        assert!(
            view.sql_panel
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("Statement 1")
        );
    });
    edit_sql(&viewer, "SELECT 99 AS answer;");
    click(&viewer, Button::SqlSave);
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        "SELECT 99 AS answer;"
    );
    let scratch = sql_view("Scratch", SqlConfig::default());
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::View(scratch), false, false, false, cx));
    viewer.run_until_parked();
    viewer.update(|view, cx| view.navigate_history(false, cx));
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.identity.clone()), SessionIdentity::StartupSql);
    edit_sql(&viewer, "SELECT 100 AS answer;");
    click(&viewer, Button::SqlSave);
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        "SELECT 100 AS answer;"
    );
    assert!(repository.load_views().0.is_empty());
    assert!(repository.load_history().unwrap().recent_sources.is_empty());
}

#[test]
fn sql_editor_rows_follow_grid_font_size_changes() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), Some(sql_view("SQL", SqlConfig::default())));
    let mut heights = Vec::new();
    for font_size in [13u8, 22, 10] {
        viewer.update(|view, cx| view.change_font_size(i16::from(font_size) - i16::from(view.font.size), cx));
        viewer.render();
        viewer.read(|view| {
            assert_eq!(view.fonts.ui.size, f32::from(font_size));
            assert_eq!(view.fonts.mono.size, f32::from(font_size));
            assert!(view.sql_panel.is_some());
            heights.push(crate::ui::widgets::editor::Editor::line_box_height(&view.fonts.mono));
        });
    }
    assert!(heights[1] > heights[0] && heights[0] > heights[2], "{heights:?}");
}

#[test]
fn sql_auto_save_debounces_persists_checkbox_and_never_executes() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view(
        "Auto SQL",
        SqlConfig {
            text: "SELECT 1".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    switch(&viewer, ViewOption::AutoSave);
    assert!(repository.load_views().0[0].auto_save);
    edit_sql(&viewer, "CREATE TABLE should_not_run(id INT)");
    viewer.advance_clock(Duration::from_millis(1500));
    assert_eq!(saved_sql(&repository).text, "SELECT 1");
    let latest = "CREATE TABLE should_not_run(id INT); SELECT 2";
    edit_sql(&viewer, latest);
    viewer.advance_clock(Duration::from_millis(1999));
    assert_eq!(saved_sql(&repository).text, "SELECT 1");
    viewer.advance_clock(Duration::from_millis(1));
    let saved = repository.load_views().0.remove(0);
    assert!(saved.auto_save && matches!(&saved.view, ViewKind::Sql(config) if config.text == latest));
    viewer.read(|view| {
        assert!(!view.session.dirty);
        assert_eq!(view.session.result.row_count(), 0);
    });
    assert_eq!(table_count(&viewer, "'should_not_run'"), 0);
    let reopened = self::viewer(repository.clone(), Some(saved));
    assert!(reopened.read(|view| view.auto_save()));
    switch(&viewer, ViewOption::AutoSave);
    edit_sql(&viewer, "SELECT 3");
    viewer.advance_clock(Duration::from_secs(3));
    let config = saved_sql(&repository);
    assert!(!repository.load_views().0[0].auto_save && config.text == latest);
    assert!(viewer.read(|view| view.session.dirty));
}

#[test]
fn sql_auto_save_flushes_before_navigation_and_cancels_old_timer() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view(
        "Auto SQL",
        SqlConfig {
            text: "SELECT 1".into(),
            ..Default::default()
        },
    );
    saved.auto_save = true;
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    edit_sql(&viewer, "SELECT 9");
    let path = home.path().join("other.csv");
    std::fs::write(&path, "n\n1\n").unwrap();
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::Source(path), false, false, true, cx));
    viewer.run_until_parked();
    assert_eq!(saved_sql(&repository).text, "SELECT 9");
    viewer.advance_clock(Duration::from_secs(3));
    assert!(!viewer.read(|view| view.session.is_sql()));
    assert_eq!(repository.load_views().0.len(), 1);
}

#[test]
fn enabling_sql_auto_save_on_a_draft_names_and_saves_it_once() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository.clone(), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    edit_sql(&viewer, "SELECT 7");
    switch(&viewer, ViewOption::AutoSave);
    viewer.render();
    assert!(viewer.has_dialog());
    click(&viewer, Button::DialogOk);
    assert!(!viewer.has_dialog());
    let saved = repository.load_views().0.remove(0);
    assert!(saved.auto_save && matches!(&saved.view, ViewKind::Sql(config) if config.text == "SELECT 7"));
    edit_sql(&viewer, "SELECT 8");
    viewer.advance_clock(Duration::from_secs(2));
    let views = repository.load_views().0;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, saved.id);
    assert!(matches!(&views[0].view, ViewKind::Sql(config) if config.text == "SELECT 8"));
}

#[test]
fn sql_auto_save_failure_keeps_dirty_text_and_recovers_on_next_edit() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view(
        "Auto SQL",
        SqlConfig::default(),
    );
    saved.auto_save = true;
    let saved_path = repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    std::fs::remove_file(&saved_path).unwrap();
    std::fs::create_dir(&saved_path).unwrap();
    edit_sql(&viewer, "SELECT 9");
    viewer.advance_clock(Duration::from_secs(2));
    viewer.read(|view| {
        assert!(view.session.dirty);
        assert!(view.error.as_ref().unwrap().contains("Auto-save failed"));
    });
    assert_eq!(sql_config(&viewer).text, "SELECT 9");
    std::fs::remove_dir(&saved_path).unwrap();
    edit_sql(&viewer, "SELECT 10");
    viewer.advance_clock(Duration::from_secs(2));
    assert_eq!(saved_sql(&repository).text, "SELECT 10");
    viewer.read(|view| {
        assert!(!view.session.dirty);
        assert!(view.error.is_none());
    });
}

#[test]
fn switching_from_saved_sql_with_file_picker_restores_grid_focus() {
    check_sql_file_picker_focus(false);
}

#[test]
fn switching_from_hidden_sql_with_file_picker_restores_grid_focus() {
    check_sql_file_picker_focus(true);
}

fn check_sql_file_picker_focus(hide_panel: bool) {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("books.csv");
    std::fs::write(&path, "title\nFirst\nSecond\n").unwrap();
    let mut table = DocumentDefinition::draft_table("Books".into(), path.to_string_lossy().into_owned());
    repository.save_view(&mut table).unwrap();
    let mut scratch = sql_view(
        "Scratch",
        SqlConfig {
            text: "SELECT 42 AS answer".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut scratch).unwrap();
    let viewer = viewer(repository, Some(table));
    let pick = |name: &str| {
        press(&viewer, "ctrl-p");
        viewer.input(name);
        viewer.run_until_parked();
        press(&viewer, "enter");
    };
    // The picker opens a SQL source as its rows; the editor opens directly.
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::View(scratch.clone()), false, false, false, cx));
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.session.is_sql()));
    press(&viewer, "ctrl-j");
    press(&viewer, "ctrl-enter");
    viewer.read(|view| {
        let panel = view.sql_panel.as_ref().unwrap();
        assert_eq!(
            view.session.result.row_count(),
            1,
            "hidden={hide_panel}, status={}, error={:?}, text={}",
            panel.status,
            panel.error,
            panel.sql_to_run(true)
        );
    });
    if hide_panel {
        click(&viewer, Button::SqlClose);
    }
    assert_eq!(viewer.read(|view| view.sql_panel_open), !hide_panel);
    pick("Books");
    viewer.render();
    viewer.read(|view| {
        assert!(!view.session.is_sql());
        assert_eq!(view.session.result.row_count(), 2);
    });
    assert!(
        viewer.grid_focused(),
        "grid lost focus after SQL navigation (hidden={hide_panel})"
    );
    viewer.press("ctrl-p");
    assert!(viewer.has_dialog());
    viewer.press("escape");
    viewer.run_until_parked();
}

#[test]
fn sql_panel_shortcuts_preserve_grid_and_run_current_statement() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    new_source(&viewer);
    viewer.read(|view| {
        assert!(view.session.is_sql());
        assert!(view.sql_panel_open);
        assert_eq!(view.session.result.row_count(), 2);
    });
    place(&viewer, "SELECT 10 AS first; SELECT 20 AS second", 35..35);
    press(&viewer, "ctrl-enter");
    viewer.read(|view| {
        assert!(!view.sql_running);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.session.result.columns[0].0, "second");
        let panel = view.sql_panel.as_ref().unwrap();
        assert!(panel.error.is_none(), "{:?}", panel.error);
    });
    press(&viewer, "escape");
    assert!(viewer.grid_focused());
    viewer.press("ctrl-j");
    assert!(viewer.read(|view| view.sql_panel_open));
    click(&viewer, Button::SqlClose);
    viewer.read(|view| {
        assert!(!view.sql_panel_open);
        assert!(view.sql_panel.is_some());
    });
    viewer.press("ctrl-j");
    assert!(viewer.read(|view| view.sql_panel_open));
    viewer.render();
    let splitter = viewer
        .find(|hit| *hit == Hit::Splitter(Splitter::Sql))
        .expect("the SQL splitter")
        .center();
    let height = viewer.read(|view| view.sql_editor_height);
    viewer.drag(splitter, crate::ui::platform::point(splitter.x, splitter.y + height - 180.0));
    let resized = viewer.read(|view| view.sql_editor_height);
    assert!((resized - 180.0).abs() < 2.0, "{resized}");
    let repository = repository(home.path());
    let saved = repository.load_settings().unwrap().sql_editor_height.unwrap();
    assert!((saved - 180.0).abs() < 2.0, "{saved}");
    let reopened = self::viewer(repository, None);
    new_source(&reopened);
    assert!((reopened.read(|view| view.sql_editor_height) - saved).abs() < 2.0);
}

#[test]
fn sql_editor_keys_follow_the_keymap_and_its_actions_are_commands() {
    use crate::application::keymap::{ActionId, DisabledDefault, KeyContext, UserBinding};
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    new_source(&viewer);
    // Run moves from Ctrl+Enter to F5; Ctrl+/ no longer comments.
    viewer.update(|view, _| {
        for (action, keystroke) in [(ActionId::RunSql, "ctrl-enter"), (ActionId::SqlToggleLineComment, "ctrl-/")] {
            view.keymap.disabled_defaults.push(DisabledDefault {
                action,
                context: KeyContext::SqlEditor,
                keystroke: keystroke.into(),
            });
        }
        view.keymap.bindings.push(UserBinding {
            action: ActionId::RunSql,
            context: KeyContext::SqlEditor,
            keystroke: "f5".into(),
        });
    });
    place(&viewer, "SELECT 10 AS first", 0..0);
    let first_column = || viewer.read(|view| view.session.result.columns[0].0.clone());
    let before = first_column();
    press(&viewer, "ctrl-enter");
    assert_eq!(first_column(), before);
    press(&viewer, "f5");
    assert_eq!(first_column(), "first");
    press(&viewer, "ctrl-/");
    assert_eq!(sql_text(&viewer), "SELECT 10 AS first");
    // Defaults still reach the editor's actions through the keymap.
    press(&viewer, "ctrl-shift-k");
    assert_eq!(sql_text(&viewer), "");

    // The palette lists the editor's commands with their keys while it is open.
    let commands = |viewer: &Viewer| {
        viewer.read(|view| {
            crate::ui::command_palette::matching_commands(&super::active_commands(), &view.keymap, "fold")
                .into_iter()
                .map(|(command, _, shortcut)| (command.id, shortcut))
                .collect::<Vec<_>>()
        })
    };
    assert!(commands(&viewer).contains(&(CommandId::SqlFold, "Ctrl+Shift+[".into())));
    place(&viewer, "SELECT\n  1,\n  2", 0..0);
    viewer.update(|view, cx| view.dispatch_command(CommandId::SqlSelectLine, cx));
    assert_eq!(panel(&viewer, |panel| panel.editor.selected_text()), "SELECT\n");
    assert!(editor_focused(&viewer));
}

#[test]
fn sql_editor_underlines_problems_and_shows_the_call_being_typed() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    place(&viewer, "SELECT * FROM t WHERE;\nSELECT 1", 0..0);
    let problems = panel(&viewer, |panel| panel.editor.diagnostics());
    assert!(!problems.is_empty() && problems.iter().all(|(start, _, _)| *start < 22), "{problems:?}");
    // The connection's functions come with the catalog once it opens.
    let text = "SELECT round(1.5";
    place(&viewer, text, text.len()..text.len());
    viewer.input(",");
    viewer.run_until_parked();
    let (label, active) = panel(&viewer, |panel| panel.editor.signature()).expect("signature help");
    assert!(label.starts_with("round("), "{label}");
    assert!(active.is_some(), "{label}");
    press(&viewer, "escape");
    assert!(panel(&viewer, |panel| panel.editor.signature()).is_none());
    assert!(editor_focused(&viewer));
}

#[test]
fn editor_options_change_the_sql_editor_and_persist() {
    use super::dialogs::{Dialog, PaletteKind};
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    let row = |viewer: &Viewer, label: &str| {
        viewer.read(|view| match &view.dialog {
            Some(Dialog::Palette(dialog)) => dialog.palette.list.items().iter().position(|item| item.label == label),
            _ => None,
        })
    };
    viewer.update(|view, cx| view.open_editor_settings(None, cx));
    // A switch flips in place and the list stays open.
    let wrap = row(&viewer, "Word wrap").unwrap();
    viewer.update(|view, cx| view.confirm_editor_setting(wrap, cx));
    assert!(viewer.read(|view| matches!(&view.dialog, Some(Dialog::Palette(d)) if matches!(d.kind, PaletteKind::EditorSettings))));
    // A size opens its choices and comes back to the options.
    let size = row(&viewer, "Completion list font size").unwrap();
    viewer.update(|view, cx| view.confirm_editor_setting(size, cx));
    let eighteen = row(&viewer, "18 pt").unwrap();
    viewer.update(|view, cx| view.confirm_palette(eighteen, crate::ui::platform::Modifiers::NONE, cx));
    assert!(row(&viewer, "Completion list rows").is_some());
    let options = panel(&viewer, |panel| panel.editor.options());
    assert!(options.word_wrap);
    assert_eq!(options.suggest_font_size, 18.0);
    let saved = repository(home.path()).load_settings().unwrap().editor;
    assert!(saved.word_wrap);
    assert_eq!(saved.completion_font_size, Some(18));
    // Another window of the profile, and the next start, use them too.
    let reopened = self::viewer(repository(home.path()), None);
    assert_eq!(reopened.read(|view| view.editor_settings.completion_font_size), Some(18));
}

#[test]
fn sql_search_sort_and_save_never_reexecute_statements() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository.clone(), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    place(
        &viewer,
        "CREATE TABLE apples(id BIGINT); INSERT INTO apples VALUES (2),(1) RETURNING *",
        0..0,
    );
    viewer.update(|view, cx| view.run_sql(true, cx));
    viewer.run_until_parked();
    viewer.update(|view, cx| {
        assert_eq!(view.session.result.row_count(), 2);
        view.session.set_sort_direction("id".into(), false);
        view.set_search_text("1".into(), cx);
    });
    viewer.run_until_parked();
    viewer.update(|view, cx| {
        assert_eq!(view.session.result.row_count(), 1);
        view.set_search_text(String::new(), cx);
        view.sql_panel.as_mut().unwrap().edit("SELECT * FROM apples");
    });
    viewer.run_until_parked();
    viewer.update(|view, _| {
        view.session.save_as("Fruit SQL".into(), &view.repository).unwrap();
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.result.row_count(), 2);
        assert_eq!(view.session.result.fetch_rows(0, 2).unwrap()[0].values[0].as_deref(), Some("1"));
        assert_eq!(
            view.database
                .connect()
                .unwrap()
                .query_row("SELECT count(*) FROM apples", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    });
    let documents = repository.load_views().0;
    assert_eq!(documents.len(), 1);
    assert!(documents[0].source_path.is_empty());
    let reopened = self::viewer(repository, Some(documents[0].clone()));
    reopened.read(|view| {
        assert!(view.sql_panel_open);
        assert_eq!(view.session.result.row_count(), 0);
        assert!(!view.session.dirty);
        assert!(!view.sql_running);
        assert_eq!(view.sql_panel.as_ref().unwrap().sql_to_run(true), "SELECT * FROM apples");
    });
    reopened.update(|view, cx| view.run_sql(true, cx));
    reopened.run_until_parked();
    reopened.read(|view| {
        let panel = view.sql_panel.as_ref().unwrap();
        assert!(panel.error.is_none(), "{:?}", panel.error);
        assert_eq!(
            view.session.result.row_count(),
            2,
            "{}; running {}",
            panel.status,
            view.sql_running
        );
        assert_eq!(view.session.sort.len(), 1);
        assert_eq!(view.session.result.fetch_rows(0, 2).unwrap()[0].values[0].as_deref(), Some("1"));
    });
}

#[test]
fn sql_completion_escape_and_selection_are_editor_local() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    place(&viewer, "SELECT 1; SELECT 2", 0..8);
    assert_eq!(panel(&viewer, |panel| panel.sql_to_run(false)), "SELECT 1");
    place(&viewer, "SELECT 1; SEL", 13..13);
    press(&viewer, "ctrl-space");
    assert_eq!(completion_labels(&viewer).unwrap()[0], "SELECT");
    viewer.press("escape");
    assert_eq!(completion_labels(&viewer), None);
    assert!(editor_focused(&viewer));
    viewer.press("tab");
    assert!(editor_focused(&viewer));
}

#[test]
fn sql_alias_completion_triggers_on_dot_and_inserts_only_column() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    panel(&viewer, |panel| {
        *panel.catalog.write().unwrap() = crate::domain::sql_editor::SqlCatalog {
            relations: vec![crate::domain::sql_editor::Relation {
                catalog: "db".into(),
                schema: "main".into(),
                name: "apples".into(),
                columns: vec!["color".into()],
                is_view: false,
                current_database: true,
                ..Default::default()
            }],
            current_database: "db".into(),
            ..Default::default()
        };
    });
    place(&viewer, "SELECT a\nFROM apples a", 8..8);
    viewer.input(".");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(completion_labels(&viewer).unwrap()[0], "color");
    press(&viewer, "enter");
    let expected = "SELECT a.color\nFROM apples a";
    assert_eq!(sql_text(&viewer), expected);
    assert!(viewer.read(|view| view.session.dirty));
    assert_eq!(sql_config(&viewer).text, expected);
    assert!(editor_focused(&viewer));
}

#[test]
fn sql_completion_triggers_and_quoting_follow_the_editor_options() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    panel(&viewer, |panel| {
        *panel.catalog.write().unwrap() = crate::domain::sql_editor::SqlCatalog {
            relations: vec![crate::domain::sql_editor::Relation {
                catalog: "db".into(),
                schema: "main".into(),
                name: "apples".into(),
                columns: vec!["color".into()],
                current_database: true,
                ..Default::default()
            }],
            current_database: "db".into(),
            ..Default::default()
        };
    });
    let typed = |text: &str, typed: &str| {
        place(&viewer, text, text.len()..text.len());
        viewer.input(typed);
        viewer.run_until_parked();
        viewer.render();
        completion_labels(&viewer)
    };
    // A space after FROM lists the tables; a space elsewhere opens nothing.
    assert!(typed("SELECT * FROM", " ").unwrap().contains(&"apples".to_string()));
    viewer.press("escape");
    assert_eq!(typed("SELECT", " "), None);
    viewer.update(|view, _| {
        let mut settings = view.editor_settings.clone();
        settings.suggest_after_from = false;
        settings.always_quote_names = true;
        view.set_editor_settings(settings);
    });
    assert_eq!(typed("SELECT * FROM", " "), None);
    assert_eq!(typed("SELECT * FROM apples a WHERE a", ".").unwrap()[0], "color");
    press(&viewer, "enter");
    assert_eq!(sql_text(&viewer), "SELECT * FROM apples a WHERE a.\"color\"");
}

#[test]
fn sql_open_paren_lists_names_only_where_a_call_takes_one() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    let typed = |text: &str| {
        place(&viewer, text, text.len()..text.len());
        viewer.input("(");
        viewer.run_until_parked();
        viewer.render();
        completion_labels(&viewer)
    };
    assert_eq!(typed("SET VARIABLE qq = 12;
SELECT getvariable").unwrap()[0], "qq");
    press(&viewer, "enter");
    assert!(sql_text(&viewer).starts_with("SET VARIABLE qq = 12;
SELECT getvariable('qq'"), "{}", sql_text(&viewer));
    assert_eq!(typed("SELECT sum"), None);
}

#[test]
fn sql_alias_of_a_file_completes_its_columns_once_described() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("athletes.parquet").display().to_string().replace('\\', "/");
    duckdb::Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!("COPY (SELECT 1 AS id, 'Ann' AS name) TO '{file}'"))
        .unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    let text = format!("FROM '{file}' a\nSELECT a");
    place(&viewer, &text, text.len()..text.len());
    // Typing `.` finds no columns yet; the file is described once typing
    // pauses, and the list opens with them.
    viewer.input(".");
    viewer.run_until_parked();
    assert_eq!(completion_labels(&viewer), None);
    viewer.advance_clock(Duration::from_millis(300));
    viewer.render();
    assert_eq!(completion_labels(&viewer).unwrap(), ["id", "name"]);
    press(&viewer, "enter");
    assert_eq!(sql_text(&viewer), format!("FROM '{file}' a\nSELECT a.id"));
    // Known now, so the next `.` lists them at once.
    viewer.input(", a.");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(completion_labels(&viewer).unwrap(), ["id", "name"]);
}

#[test]
fn sql_path_completion_lists_search_roots_and_tab_accepts_into_folders() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("sales data")).unwrap();
    std::fs::write(data.path().join("sales data").join("it's.csv"), "a\n1\n").unwrap();
    std::fs::write(data.path().join("sales data").join("q1.csv"), "a\n1\n").unwrap();
    std::fs::write(data.path().join("summary.txt"), "").unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    panel(&viewer, |panel| {
        *panel.path_roots.write().unwrap() = crate::infrastructure::path_completion::PathRoots {
            search: vec![data.path().to_path_buf()],
            ..Default::default()
        };
    });
    place(&viewer, "SELECT * FROM '", 15..15);
    viewer.input("s");
    viewer.run_until_parked();
    assert_eq!(completion_labels(&viewer).unwrap(), ["sales data/", "summary.txt"]);
    for (expected_text, expected_items) in [
        ("SELECT * FROM 'sales data/", Some(vec!["it's.csv", "q1.csv"])),
        ("SELECT * FROM 'sales data/it''s.csv", None),
    ] {
        press(&viewer, "tab");
        assert_eq!(sql_text(&viewer), expected_text);
        if let Some(items) = expected_items {
            assert_eq!(completion_labels(&viewer).unwrap(), items);
        }
    }
    // With no menu open, Tab keeps its editing meaning.
    assert_eq!(completion_labels(&viewer), None);
    press(&viewer, "tab");
    assert_ne!(sql_text(&viewer), "SELECT * FROM 'sales data/it''s.csv");
}

#[test]
fn sql_completion_retriggers_before_an_earlier_completion() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("empty")).unwrap();
    std::fs::write(data.path().join("sales.csv"), "a\n1\n").unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    panel(&viewer, |panel| {
        *panel.path_roots.write().unwrap() = crate::infrastructure::path_completion::PathRoots {
            search: vec![data.path().to_path_buf()],
            ..Default::default()
        };
    });
    place(&viewer, "SELECT * FROM 'empty", 20..20);
    // An empty folder: the trigger finds nothing.
    press(&viewer, "/");
    assert_eq!(sql_text(&viewer), "SELECT * FROM 'empty/");
    assert_eq!(completion_labels(&viewer), None);
    // Start over at the beginning of the text and type a new path query.
    place(&viewer, "", 0..0);
    for key in ["f", "r", "o", "m", "space", "'", "s"] {
        viewer.press(key);
    }
    viewer.run_until_parked();
    let labels = completion_labels(&viewer);
    assert!(labels.is_some(), "{:?}", sql_text(&viewer));
    assert_eq!(labels.unwrap()[0], "sales.csv");
}

#[test]
fn sql_results_filter_every_row_undo_and_prune_missing_columns() {
    use super::dialogs::Dialog;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let viewer = viewer(repository, None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    let rows = || viewer.read(|view| view.session.result.row_count());
    let filter_first_cell = |exclude: bool| {
        viewer.update(|view, cx| {
            view.grid.select(crate::domain::selection::Cell { row: 0, column: 0 }, false);
            view.filter_active_value(exclude, cx);
        });
        viewer.run_until_parked();
    };
    // Setup runs first; the last query keeps all six rows despite the limit of 2.
    run_all_sql(
        &viewer,
        "CREATE OR REPLACE TEMP TABLE fruit AS SELECT * FROM (VALUES \
         ('red', 1), ('green', 2), ('red', 3), ('gold', 4), ('red', 5), ('green', 6)) t(color, n);\n\
         SELECT color, n FROM fruit ORDER BY n;",
    );
    assert!(viewer.read(|view| view.sql_result_complete));
    assert_eq!(rows(), 6);
    // Every row is there, so the limit warning stays off.
    assert!(!viewer.read(|view| view.limit_reached()));
    let add_slicer = || {
        viewer.update(|view, cx| {
            view.error = None;
            view.dialog = None;
            view.dispatch_command(CommandId::AddSlicer, cx);
            let opened = matches!(view.dialog, Some(Dialog::SlicerPicker(_)));
            view.dialog = None;
            (opened, view.error.clone())
        })
    };
    assert_eq!(add_slicer(), (true, None));
    // `=` on "red" finds all three reds, two of them past the row limit.
    filter_first_cell(false);
    assert!(viewer.read(|view| view.error.is_none()));
    assert_eq!(rows(), 3);
    viewer.update(|view, cx| view.undo_view_change(false, cx));
    viewer.run_until_parked();
    assert_eq!(rows(), 6);
    // `-` excludes; the SQL text is untouched by undo.
    filter_first_cell(true);
    assert_eq!(rows(), 3);
    assert!(sql_config(&viewer).text.contains("SELECT color, n"));
    // A rerun without the filtered column drops the filter instead of failing.
    run_all_sql(&viewer, "SELECT n FROM fruit");
    viewer.read(|view| assert!(view.error.is_none(), "{:?}", view.error));
    assert_eq!(rows(), 6);
    // A writing statement keeps the row limit and explains why filters are off.
    run_all_sql(&viewer, "INSERT INTO fruit SELECT 'blue', range FROM range(5) RETURNING color");
    assert!(viewer.read(|view| view.limit_reached()), "five rows came back and two are shown");
    run_all_sql(&viewer, "INSERT INTO fruit VALUES ('blue', 7) RETURNING color");
    assert!(!viewer.read(|view| view.sql_result_complete));
    assert!(!viewer.read(|view| view.limit_reached()));
    let (opened, error) = add_slicer();
    assert!(!opened && error.is_some_and(|error| error.contains("last statement")));
    filter_first_cell(false);
    viewer.read(|view| {
        let error = view.error.clone().unwrap_or_default();
        assert!(error.contains("last statement"), "{error}");
        assert!(view.sql_panel.as_ref().unwrap().status.contains("filters are off"));
    });
}

fn ask_state(viewer: &Viewer) -> String {
    panel(viewer, |panel| match &panel.ask {
        AskState::Idle => "idle".into(),
        AskState::Working(text) => format!("working: {text}"),
        AskState::Proposal(proposal) => format!("proposal: {}", proposal.sql),
        AskState::Failed(error) => format!("failed: {error}"),
    })
}

#[test]
fn ask_ai_writes_checked_sql_and_tab_inserts_it() {
    use crate::domain::ai::{AiEvent, AiProvider, SqlCheck};
    use crate::infrastructure::ai_client::test_server::{reply, serve};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository.clone(), None);
    new_source(&viewer);
    edit_sql(&viewer, "CREATE TEMP TABLE fruit AS SELECT 'red' AS color;");
    press(&viewer, "ctrl-enter");

    // Without a provider, the strip says where to add one.
    let end = sql_text(&viewer).len();
    place(&viewer, &sql_text(&viewer), end..end);
    viewer.press("ctrl-k");
    assert!(panel(&viewer, |panel| panel.ask_open));
    assert!(ask_focused(&viewer));
    viewer.render();
    viewer.input("colors");
    viewer.press("enter");
    viewer.run_until_parked();
    let state = ask_state(&viewer);
    assert!(state.starts_with("failed: No AI provider"), "{state}");

    // The first reply names a missing column; DuckDB's error goes back and
    // the second reply is offered.
    let (url, server) = serve(vec![
        ("200 OK", reply("```sql\nSELECT colour FROM fruit\n```")),
        ("200 OK", reply("```sql\nSELECT color FROM fruit\n```")),
    ]);
    let mut settings = repository.load_settings().unwrap();
    settings.ai.providers.push(AiProvider {
        name: "local".into(),
        base_url: url,
        model: "test-model".into(),
        ..Default::default()
    });
    repository.save_settings(&settings).unwrap();
    press(&viewer, "enter");
    let requests = server.join().unwrap();
    viewer.run_until_parked();
    assert!(requests[0].contains("temp.fruit: color VARCHAR"), "{}", requests[0]);
    assert!(requests[0].contains("CREATE TEMP TABLE fruit"));
    assert!(requests[1].contains("DuckDB rejected"), "{}", requests[1]);
    assert!(requests[1].contains("colour"));
    panel(&viewer, |panel| {
        let AskState::Proposal(proposal) = &panel.ask else {
            panic!("no proposal");
        };
        assert_eq!(proposal.sql, "SELECT color FROM fruit");
        assert_eq!(proposal.check, SqlCheck::Passed);
        // The log holds both rounds: sent, received, and DuckDB's verdict.
        let log = &panel.ask_log;
        assert_eq!(log.len(), 6, "{log:?}");
        assert!(matches!(&log[2], AiEvent::Checked(SqlCheck::Failed(error)) if error.contains("colour")));
        assert_eq!(log[5], AiEvent::Checked(SqlCheck::Passed));
    });
    click(&viewer, Button::AskLog);
    assert!(panel(&viewer, |panel| panel.show_log));
    viewer.render();
    assert!(viewer.find(|hit| *hit == Hit::Button(Button::AskInsert)).is_some());
    // The click moved focus; Tab accepts from the prompt.
    let ask_input = panel(&viewer, |panel| panel.ask_input.id);
    viewer.focus(ask_input);
    press(&viewer, "tab");
    let expected = "CREATE TEMP TABLE fruit AS SELECT 'red' AS color;\nSELECT color FROM fruit";
    assert_eq!(sql_text(&viewer), expected);
    // The new SQL is selected so Ctrl+Enter runs just it.
    let start = expected.find("SELECT color").unwrap();
    assert_eq!(panel(&viewer, |panel| panel.editor.selected_range()), start..expected.len());
    assert!(editor_focused(&viewer));
    assert!(!panel(&viewer, |panel| panel.ask_open));
    assert_eq!(sql_config(&viewer).text, expected);
    press(&viewer, "ctrl-enter");
    viewer.read(|view| {
        assert_eq!(view.session.result.columns[0].0, "color");
        assert_eq!(view.session.result.row_count(), 1);
    });

    // Escape closes the strip and returns to the editor.
    viewer.press("ctrl-k");
    viewer.render();
    viewer.press("escape");
    assert!(!panel(&viewer, |panel| panel.ask_open));
    assert!(editor_focused(&viewer));
}

#[test]
fn ask_sql_command_opens_the_editor_with_the_prompt_focused() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.dispatch_command(CommandId::AskSql, cx));
    assert!(viewer.read(|view| view.session.is_sql()));
    assert!(panel(&viewer, |panel| panel.ask_open));
    assert!(ask_focused(&viewer));
}

#[test]
fn sql_views_freeze_leading_columns_without_reordering() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    new_source(&viewer);
    run_all_sql(&viewer, "SELECT 1 AS region, 2 AS product, 3 AS \"2024\"");
    let frozen = || (sql_config(&viewer).frozen_columns, viewer.read(|view| view.grid.frozen_count()));
    // F on the second column freezes the first two.
    viewer.focus_grid();
    viewer.render();
    viewer.press("right");
    press(&viewer, "f");
    assert_eq!(frozen(), (vec!["region".to_owned(), "product".to_owned()], 2));
    viewer.read(|view| {
        let columns = &view.session.result.columns;
        assert_eq!(
            columns.iter().map(|column| column.0.as_str()).collect::<Vec<_>>(),
            ["region", "product", "2024"]
        );
        assert!(view.session.dirty);
    });
    // Again on it: it unfreezes, the first stays. Undo brings it back.
    press(&viewer, "f");
    assert_eq!(frozen(), (vec!["region".to_owned()], 1));
    press(&viewer, "ctrl-z");
    assert_eq!(frozen().1, 2);
    // A rerun keeps only the part still leading the result.
    run_all_sql(&viewer, "SELECT 1 AS region, 2 AS month, 3 AS product");
    assert_eq!(frozen().1, 1);
    viewer.focus_grid();
    press(&viewer, "shift-f");
    assert_eq!(frozen(), (Vec::new(), 0));
}

#[test]
fn auto_run_sql_views_run_when_opened() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view(
        "Report",
        SqlConfig {
            text: "SELECT 7 AS lucky".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut saved).unwrap();
    // Off by default: opening shows the SQL without running it.
    let viewer = viewer(repository.clone(), Some(saved));
    assert_ne!(
        viewer.read(|view| view.session.result.columns.first().map(|column| column.0.clone())),
        Some("lucky".into())
    );
    switch(&viewer, ViewOption::AutoRun);
    // The checkbox is stored with the saved view at once.
    let stored = repository
        .load_views()
        .0
        .into_iter()
        .find(|view| view.name == "Report")
        .unwrap();
    let ViewKind::Sql(config) = &stored.view else {
        panic!("not a SQL view")
    };
    assert!(config.auto_run);
    let reopened = self::viewer(repository, Some(stored));
    reopened.read(|view| {
        let result = &view.session.result;
        assert_eq!(result.columns[0].0, "lucky");
        assert_eq!(result.row_count(), 1);
        assert!(matches!(&view.session.working.as_ref().unwrap().view, ViewKind::Sql(config) if config.auto_run));
    });
}

#[test]
fn sql_editor_indents_four_spaces() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    new_source(&viewer);
    viewer.render();
    viewer.press("tab");
    viewer.input("x");
    assert_eq!(sql_text(&viewer), "    x");
}

#[test]
fn ask_ai_at_mentions_complete_and_send_only_those_tables() {
    use crate::domain::ai::{AiEvent, AiProvider};
    use crate::infrastructure::ai_client::test_server::{reply, serve};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository.clone(), None);
    new_source(&viewer);
    run_all_sql(
        &viewer,
        "CREATE TEMP TABLE fruit AS SELECT * FROM (VALUES ('red', 1), ('green', 2)) t(color, n); COMMENT ON COLUMN fruit.color IS 'Skin color'; CREATE TEMP TABLE other AS SELECT 1 AS secret_column",
    );
    let ask = |send_data: bool, typed: &str| {
        let (url, server) = serve(vec![("200 OK", reply("```sql\nSELECT color FROM temp.fruit\n```"))]);
        let mut settings = repository.load_settings().unwrap();
        settings.ai.providers = vec![AiProvider {
            name: "local".into(),
            base_url: url,
            model: "test-model".into(),
            send_data,
            ..Default::default()
        }];
        repository.save_settings(&settings).unwrap();
        viewer.update(|view, _| {
            let panel = view.sql_panel.as_mut().unwrap();
            panel.ask_input.set_text("");
            let mut focus = view.focus;
            panel.open_ask(&mut focus);
            view.focus = focus;
        });
        viewer.render();
        viewer.input(typed);
        viewer.run_until_parked();
        server
    };

    // `@` lists tables; Tab inserts the SQL name and a space.
    let server = ask(false, "colors in @fru");
    panel(&viewer, |panel| {
        let provider = panel.ask_provider.clone().unwrap();
        assert_eq!((provider.name.as_str(), provider.send_data), ("local", false));
        assert_eq!(panel.mentions.items().unwrap(), ["temp.fruit"]);
    });
    viewer.press("tab");
    panel(&viewer, |panel| {
        assert_eq!(panel.ask_input.text(), "colors in @temp.fruit ");
        // Tab completed the name; it did not accept anything else.
        assert!(panel.ask_open);
    });
    viewer.press("enter");
    let request = server.join().unwrap().remove(0);
    viewer.run_until_parked();
    assert!(request.contains("Relations the user named"), "{request}");
    assert!(request.contains("temp.fruit · 2 rows"), "{request}");
    assert!(request.contains("color VARCHAR [Skin color]: ~2 distinct"), "{request}");
    // Counts only without the opt-in; other tables only by name.
    assert!(!request.contains("values:") && !request.contains("min '"), "{request}");
    // (Its column name is still in the editor SQL sent as context.)
    assert!(request.contains("not described): temp.other"), "{request}");
    assert!(!request.contains("secret_column INTEGER"));

    // With data allowed, the few values are listed.
    let server = ask(true, "colors in @temp.fruit please");
    viewer.press("enter");
    let request = server.join().unwrap().remove(0);
    viewer.run_until_parked();
    assert!(
        request.contains(
            "color VARCHAR [Skin color]: 2 distinct, 0.0% null; min 'green', max 'red'; values: 'green' (1), 'red' (1)"
        ),
        "{request}"
    );
    assert!(panel(&viewer, |panel| {
        panel
            .ask_log
            .iter()
            .any(|event| matches!(event, AiEvent::Note(text) if text.contains("with data values")))
    }));
}

#[test]
fn new_sql_source_and_script_replace_sql_text_after_confirming() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()), None);
    viewer.update(|view, cx| view.dispatch_command(CommandId::NewSqlSource, cx));
    viewer.run_until_parked();
    // An empty SQL draft has nothing to lose, so it is replaced without asking.
    assert!(!viewer.has_dialog());
    assert!(viewer.read(|view| view.session.is_sql()));
    assert!(!sql_config(&viewer).script);
    assert_eq!(viewer.read(|view| view.session.working.as_ref().unwrap().name.clone()), "SQL source");
    edit_sql(&viewer, "SELECT 1");
    viewer.update(|view, cx| view.dispatch_command(CommandId::NewSqlScript, cx));
    viewer.render();
    assert!(viewer.has_dialog());
    click(&viewer, Button::DialogOk);
    assert!(!viewer.has_dialog());
    assert_eq!(sql_text(&viewer), "");
    assert_eq!(sql_config(&viewer).text, "");
    assert!(sql_config(&viewer).script);
    assert!(editor_focused(&viewer));
    // A script is a scratchpad: grouping it builds nothing.
    assert!(!viewer.read(|view| view.sql_source_session()));
}

#[test]
fn reset_database_connection_drops_sql_temp_tables_and_reruns_the_startup_script() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    std::fs::write(
        repository.startup_sql_path(),
        "CREATE OR REPLACE TABLE startup_runs AS SELECT 1 AS n;",
    )
    .unwrap();
    let viewer = viewer(repository.clone(), None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    run_all_sql(&viewer, "CREATE TEMP TABLE scratch AS SELECT 1 AS n;\nSELECT * FROM scratch;");
    assert!(viewer.read(|view| view.error.is_none()));
    std::fs::write(
        repository.startup_sql_path(),
        "CREATE OR REPLACE TABLE startup_runs AS SELECT 2 AS n;",
    )
    .unwrap();
    viewer.update(|view, cx| view.dispatch_command(CommandId::ResetDatabase, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.copy_status.as_deref(), Some("Database connection reset"));
    });
    assert_eq!(sql_text(&viewer), "CREATE TEMP TABLE scratch AS SELECT 1 AS n;\nSELECT * FROM scratch;");
    run_all_sql(&viewer, "SELECT n FROM startup_runs;");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.fetch_rows(0, 1).unwrap()[0].values[0].as_deref(), Some("2"));
    });
    run_all_sql(&viewer, "SELECT * FROM scratch;");
    let error = panel(&viewer, |panel| panel.error.clone()).expect("the temp table is gone");
    assert!(format!("{error:?}").contains("scratch"), "{error:?}");
}

#[test]
fn views_build_on_sql_sources_in_either_engine_and_reopen_from_their_script() {
    use crate::domain::documents::{Engine, QueryConfig};
    for engine in [Engine::DuckDb, Engine::Polars] {
        let home = tempfile::tempdir().unwrap();
        let repository = repository(home.path());
        let mut source = sql_view(
            "Fruit",
            SqlConfig {
                // DuckDB keeps a plain table in the profile, where a second run would find it.
                text: format!(
                    "CREATE {} fruit AS SELECT * FROM (VALUES ('red', 12), ('gold', 15), ('red', 3)) AS t(color, quantity);\nSELECT * FROM fruit",
                    if engine == Engine::DuckDb { "TEMP TABLE" } else { "TABLE" }
                ),
                engine,
                ..Default::default()
            },
        );
        repository.save_view(&mut source).unwrap();
        let viewer = viewer(repository.clone(), None);
        // The tree and Ctrl+P open a SQL source as the table of its rows.
        viewer.update(|view, cx| {
            view.navigate_to_file(FileEntry::View(DocumentDefinition::draft_sql_source(&source)), false, false, false, cx)
        });
        viewer.run_until_parked();
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.session.identity, SessionIdentity::DraftTable);
            assert!(view.is_table());
            assert_eq!(view.session.source.engine, engine);
            assert_eq!(view.session.result.row_count(), 3);
        });
        viewer.update(|view, cx| {
            view.session
                .set_query_config(
                    QueryConfig {
                        group_by: vec!["color".into()],
                        ..Default::default()
                    },
                    "Fruit by color".into(),
                )
                .unwrap();
            view.schedule_search(String::new(), cx);
        });
        viewer.run_until_parked();
        assert_eq!(viewer.read(|view| view.session.result.row_count()), 2);
        viewer.update(|view, _| view.session.save_as("Fruit by color".into(), &view.repository.clone()).unwrap());
        let saved = repository
            .load_views()
            .0
            .into_iter()
            .find(|view| view.name == "Fruit by color")
            .unwrap();
        assert_eq!(saved.sql_source.as_deref(), Some(source.id.as_str()));
        // A saved view runs its source's script again when it opens.
        let reopened = self::viewer(repository.clone(), None);
        reopened.update(|view, cx| view.navigate_to_file(FileEntry::View(saved.clone()), false, false, false, cx));
        reopened.run_until_parked();
        reopened.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(matches!(view.session.identity, SessionIdentity::SavedQuery(_)));
            assert_eq!(view.session.result.row_count(), 2);
        });
        // Grouping in the SQL source's editor opens its rows and then the panel.
        reopened.update(|view, cx| view.navigate_to_file(FileEntry::View(source.clone()), false, false, false, cx));
        reopened.run_until_parked();
        assert!(reopened.read(|view| view.sql_source_session()));
        reopened.update(|view, cx| view.dispatch_command(CommandId::Group, cx));
        reopened.run_until_parked();
        reopened.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.session.working.as_ref().unwrap().sql_source.as_deref(), Some(source.id.as_str()));
            assert!(matches!(view.dialog, Some(Dialog::Pivot(_))));
        });
    }
}

#[test]
fn sql_sources_switch_to_polars_sql_and_keep_its_tables_between_runs() {
    use crate::domain::documents::Engine;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view("Polars fruit", SqlConfig::default());
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    switch(&viewer, ViewOption::Engine);
    assert_eq!(sql_config(&viewer).engine, Engine::Polars);
    // A saved source stores the engine at once.
    assert_eq!(saved_sql(&repository).engine, Engine::Polars);
    run_all_sql(&viewer, "CREATE TABLE fruit AS SELECT * FROM (VALUES ('red', 12), ('gold', 15)) AS t(color, quantity);");
    edit_sql(&viewer, "SELECT color FROM fruit WHERE quantity > 12");
    viewer.update(|view, cx| view.run_sql(true, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        let panel = view.sql_panel.as_ref().unwrap();
        assert!(panel.error.is_none(), "{:?}", panel.error);
        assert!(panel.status.contains("Polars SQL"), "{}", panel.status);
        assert_eq!(view.session.source.engine, Engine::Polars);
        assert_eq!(view.session.result.row_count(), 1);
        assert!(view.sql_result_complete);
    });
    // Switching back stores DuckDB again.
    switch(&viewer, ViewOption::Engine);
    assert_eq!(saved_sql(&repository).engine, Engine::DuckDb);
}

#[test]
fn polars_sql_stops_at_once_and_keeps_at_most_the_limit() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view("Polars stop", SqlConfig::default());
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    switch(&viewer, ViewOption::Engine);
    edit_sql(&viewer, "SELECT * FROM (VALUES (1), (2), (3)) AS t(n)");
    viewer.update(|view, cx| {
        view.query_limit = 2;
        view.run_sql(true, cx);
        view.stop_sql();
    });
    // The editor is free again before the stopped run finishes.
    viewer.read(|view| {
        assert!(!view.sql_running);
        assert_eq!(view.sql_panel.as_ref().unwrap().status, "Stopped");
    });
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 0);
    viewer.update(|view, cx| view.run_sql(true, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        let panel = view.sql_panel.as_ref().unwrap();
        assert!(panel.error.is_none(), "{:?}", panel.error);
        assert!(panel.status.contains("truncated at row limit"), "{}", panel.status);
        assert_eq!(view.session.result.row_count(), 2);
        assert!(view.sql_result_truncated && !view.sql_result_complete);
        assert!(view.sql_filters_unavailable().unwrap().contains("row limit"));
    });
}

fn set_limit(viewer: &Viewer, limit: u64) {
    viewer.update(|view, cx| {
        view.limit_input.set_text(limit.to_string());
        view.commit_query_limit(cx);
    });
    viewer.run_until_parked();
}

#[test]
fn a_new_limit_runs_read_only_sql_again_and_keeps_its_search() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut saved = sql_view("Limited", SqlConfig::default());
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone(), Some(saved));
    switch(&viewer, ViewOption::Engine);
    set_limit(&viewer, 2);
    run_all_sql(&viewer, "SELECT * FROM (VALUES (1), (2), (3), (4), (5)) AS t(n)");
    assert!(viewer.read(|view| view.sql_result_truncated));
    viewer.update(|view, cx| view.set_search_text("4".into(), cx));
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 0);
    // Raising the Limit reads the rows it cut, and the search finds one there.
    set_limit(&viewer, 10);
    viewer.read(|view| {
        assert!(!view.sql_result_truncated);
        assert_eq!(view.session.source.row_count(), 5);
        assert_eq!(view.session.search_text, "4");
        assert_eq!(view.session.result.row_count(), 1);
    });
    // Lowering it below the rows shown cuts them again.
    set_limit(&viewer, 3);
    viewer.read(|view| {
        assert!(view.sql_result_truncated);
        assert_eq!(view.session.source.row_count(), 3);
    });
}

#[test]
fn a_new_limit_leaves_sql_that_writes_for_run() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(repository, None);
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    set_limit(&viewer, 2);
    run_all_sql(
        &viewer,
        "CREATE OR REPLACE TABLE limit_log (n INTEGER);\nINSERT INTO limit_log SELECT range FROM range(5) RETURNING n",
    );
    assert!(viewer.read(|view| view.sql_result_truncated));
    let logged = || {
        viewer.read(|view| {
            view.database
                .connect()
                .unwrap()
                .query_row("SELECT count(*) FROM limit_log", [], |row| row.get::<_, i64>(0))
                .unwrap()
        })
    };
    assert_eq!(logged(), 5);
    set_limit(&viewer, 10);
    // Running it again would insert the rows twice.
    assert_eq!(logged(), 5);
    viewer.read(|view| {
        assert_eq!(view.session.source.row_count(), 2);
        let status = &view.sql_panel.as_ref().unwrap().status;
        assert!(status.contains("Run again"), "{status}");
    });
}

#[test]
fn open_sql_editor_opens_only_sql_sources_and_scripts() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut source = sql_view(
        "Answers",
        SqlConfig {
            text: "SELECT 42 AS answer".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut source).unwrap();
    let viewer = viewer(repository.clone(), None);
    // A plain table has no SQL to edit.
    press(&viewer, "ctrl-j");
    viewer.read(|view| {
        assert!(!view.session.is_sql());
        assert!(view.sql_panel.is_none());
        assert!(view.copy_status.as_deref().unwrap().contains("New SQL source"));
    });
    // From the rows of a SQL source it opens that source's editor.
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::View(DocumentDefinition::draft_sql_source(&source)), false, false, false, cx));
    viewer.run_until_parked();
    assert!(!viewer.read(|view| view.session.is_sql()));
    press(&viewer, "ctrl-j");
    viewer.read(|view| {
        assert_eq!(view.session.identity, SessionIdentity::SavedSql(source.id.clone()));
        assert!(view.sql_panel_open);
    });
    assert!(editor_focused(&viewer));
    assert_eq!(sql_text(&viewer), "SELECT 42 AS answer");
}

#[test]
fn sql_sources_without_rows_open_in_their_editor() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    for (text, message) in [
        ("CREATE TEMP TABLE t AS SELECT 1", "returns no rows"),
        ("SELECT * FROM missing", "Statement 1:"),
    ] {
        let mut source = sql_view(
            "Broken",
            SqlConfig {
                text: text.into(),
                ..Default::default()
            },
        );
        repository.save_view(&mut source).unwrap();
        let viewer = viewer(repository.clone(), None);
        open_entry(&viewer, &DocumentDefinition::draft_sql_source(&source));
        viewer.read(|view| {
            assert_eq!(view.session.identity, SessionIdentity::SavedSql(source.id.clone()));
            assert!(view.sql_panel_open);
            assert!(view.error.as_deref().unwrap_or_default().contains(message), "{:?}", view.error);
        });
        assert_eq!(sql_text(&viewer), text);
    }
}

/// The title, message and whether it is destructive, of the open confirmation.
fn confirmation(viewer: &Viewer) -> (String, String, bool) {
    viewer.read(|view| match &view.dialog {
        Some(Dialog::Confirm(confirm)) => {
            let (title, message, danger) = confirm.text();
            (title.to_owned(), message.to_owned(), danger)
        }
        _ => panic!("no confirmation is open"),
    })
}

fn confirm(viewer: &Viewer) {
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    viewer.run_until_parked();
}

fn open_entry(viewer: &Viewer, definition: &DocumentDefinition) {
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::View(definition.clone()), false, false, false, cx));
    viewer.run_until_parked();
}

fn saved_names(repository: &ViewRepository) -> Vec<String> {
    repository.load_views().0.into_iter().map(|view| view.name).collect()
}

#[test]
fn archiving_a_sql_source_warns_about_views_on_it_and_recovering_restores_them() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut source = sql_view(
        "Orders SQL",
        SqlConfig {
            text: "SELECT 1 AS n".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut source).unwrap();
    let mut on_source = DocumentDefinition::draft_sql_source(&source);
    on_source.name = "Orders table".into();
    repository.save_view(&mut on_source).unwrap();
    let viewer = viewer(repository.clone(), None);

    // With no SQL open the command says what it needs.
    viewer.update(|view, cx| view.dispatch_command(CommandId::ArchiveSql, cx));
    assert!(!viewer.has_dialog());
    assert!(viewer.read(|view| view.copy_status.clone().unwrap().contains("Open a saved SQL source")));

    // Archiving the open source names the view that reads it.
    open_entry(&viewer, &source);
    assert!(viewer.read(|view| view.session.is_sql()));
    viewer.update(|view, cx| view.dispatch_command(CommandId::ArchiveSql, cx));
    let (title, message, danger) = confirmation(&viewer);
    assert_eq!(title, "Archive Orders SQL?");
    assert!(message.starts_with("The saved view Orders table reads its rows."), "{message}");
    assert!(!danger);
    confirm(&viewer);
    assert_eq!(saved_names(&repository), ["Orders table"]);
    viewer.read(|view| {
        assert!(!view.session.is_sql());
        assert!(view.sql_panel.is_none());
    });
    // Its editor is gone, so no autosave brings it back.
    viewer.advance_clock(Duration::from_secs(5));
    assert_eq!(saved_names(&repository), ["Orders table"]);

    // The view on it says where its source went.
    open_entry(&viewer, &on_source);
    let error = viewer.read(|view| view.error.clone().unwrap_or_default());
    assert!(error.contains("Orders SQL, which is archived"), "{error}");

    // Recovering brings the source back, and the view reads it again.
    viewer.update(|view, cx| view.dispatch_command(CommandId::RecoverViews, cx));
    viewer.read(|view| match &view.dialog {
        Some(Dialog::Palette(dialog)) => {
            assert!(matches!(dialog.kind, PaletteKind::Recovery(_)));
            let label = &dialog.palette.list.items()[0].label;
            assert!(label.starts_with("Orders SQL · SQL source · "), "{label}");
        }
        _ => panic!("the recovery palette is not open"),
    });
    press(&viewer, "enter");
    assert_eq!(saved_names(&repository), ["Orders SQL", "Orders table"]);
    open_entry(&viewer, &on_source);
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 1);
    });

    // Files lists the source as its rows; Ctrl+Delete archives the source.
    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("Orders SQL");
    viewer.run_until_parked();
    let rows = viewer.read(|view| match &view.dialog {
        Some(Dialog::Palette(dialog)) => {
            let list = &dialog.palette.list;
            list.visible_items().iter().map(|&index| list.items()[index].label.clone()).collect::<Vec<_>>()
        }
        _ => panic!("Files is not open"),
    });
    assert_eq!(rows, ["Orders table", "Orders SQL"]);
    press(&viewer, "down");
    press(&viewer, "ctrl-delete");
    assert_eq!(confirmation(&viewer).0, "Archive Orders SQL?");
    confirm(&viewer);
    assert_eq!(saved_names(&repository), ["Orders table"]);
}

#[test]
fn deleting_sql_permanently_asks_first_from_the_command_tree_and_archive() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut source = sql_view(
        "Orders SQL",
        SqlConfig {
            text: "SELECT 1 AS n".into(),
            ..Default::default()
        },
    );
    repository.save_view(&mut source).unwrap();
    let mut on_source = DocumentDefinition::draft_sql_source(&source);
    on_source.name = "Orders table".into();
    repository.save_view(&mut on_source).unwrap();
    let mut script = sql_view(
        "Cleanup",
        SqlConfig {
            script: true,
            ..Default::default()
        },
    );
    repository.save_view(&mut script).unwrap();
    let viewer = viewer(repository.clone(), None);

    // While its rows are shown, the command deletes the source behind them.
    open_entry(&viewer, &DocumentDefinition::draft_sql_source(&source));
    viewer.update(|view, cx| view.dispatch_command(CommandId::DeleteSql, cx));
    let (title, message, danger) = confirmation(&viewer);
    assert_eq!(title, "Delete Orders SQL permanently?");
    assert!(message.starts_with("The saved view Orders table reads its rows, and stops working."), "{message}");
    assert!(danger);
    viewer.close_dialog();
    assert_eq!(saved_names(&repository).len(), 3);
    viewer.update(|view, cx| view.dispatch_command(CommandId::DeleteSql, cx));
    confirm(&viewer);
    assert_eq!(saved_names(&repository), ["Cleanup", "Orders table"]);
    assert!(repository.load_archives().0.is_empty());
    open_entry(&viewer, &on_source);
    let error = viewer.read(|view| view.error.clone().unwrap_or_default());
    assert_eq!(error, "This view reads a SQL source that was deleted");

    // In the tree Delete archives a script and Shift+Delete deletes it.
    press(&viewer, "ctrl-b");
    let select = |id: &str| {
        viewer.update(|view, _| {
            view.database_tree.select(id);
            let mut focus = view.focus;
            view.database_tree.focus(&mut focus);
            view.focus = focus;
        })
    };
    select(&format!("script:{}", script.id));
    press(&viewer, "shift-delete");
    assert_eq!(confirmation(&viewer).0, "Delete Cleanup permanently?");
    viewer.close_dialog();
    select(&format!("script:{}", script.id));
    press(&viewer, "delete");
    assert!(!viewer.has_dialog());
    assert_eq!(saved_names(&repository), ["Orders table"]);

    // Shift+Delete in the recovery palette deletes an archive for good.
    viewer.update(|view, cx| view.dispatch_command(CommandId::RecoverViews, cx));
    press(&viewer, "shift-delete");
    assert_eq!(confirmation(&viewer), ("Delete Cleanup permanently?".into(), "This removes it from the archive and cannot be undone.".into(), true));
    confirm(&viewer);
    assert!(repository.load_archives().0.is_empty());
    viewer.read(|view| assert!(matches!(&view.dialog, Some(Dialog::Palette(dialog)) if matches!(dialog.kind, PaletteKind::Recovery(_)))));
}
