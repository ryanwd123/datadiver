use super::DataFrameView;
use crate::{
    domain::{documents::DocumentDefinition, frame::FrameSnapshot},
    infrastructure::json_store::{StorePaths, ViewRepository},
};
use gpui_kit::{
    AppContext, Entity, Focusable, TestAppContext, WindowHandle,
    component::{Root, WindowExt},
    px, size,
    test::TestWindowExt,
};

fn viewer(
    cx: &mut TestAppContext,
    repository: ViewRepository,
    document: Option<DocumentDefinition>,
) -> (WindowHandle<Root>, Entity<DataFrameView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
    });
    let mut entity = None;
    let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
        let store = crate::infrastructure::duckdb_store::DuckDbStore::synthetic(2).unwrap();
        let view = cx.new(|cx| {
            DataFrameView::new(
                FrameSnapshot::new(store, 1),
                None,
                String::new(),
                document,
                repository,
                window,
                cx,
            )
        });
        entity = Some(view.clone());
        Root::new(view, window, cx)
    });
    (handle, entity.unwrap())
}

fn edit_sql(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    view: &Entity<DataFrameView>,
    text: &str,
) {
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            cx.emit(gpui_kit::component::input::InputEvent::Change);
        });
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn startup_sql_command_saves_plain_file_autosaves_and_runs_only_on_request(
    cx: &mut TestAppContext,
) {
    use crate::application::session::SessionIdentity;
    use std::time::Duration;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("startup sql", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(repository.startup_sql_path().is_file());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.identity, SessionIdentity::StartupSql);
        assert!(!view.read(cx).session.dirty);
        let state = view
            .read(cx)
            .sql_panel
            .as_ref()
            .unwrap()
            .read(cx)
            .editor
            .read(cx)
            .state()
            .clone();
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
        assert!(window.try_find("sql-save-script").is_some());
    })
    .unwrap();
    let sql = "CREATE TABLE should_not_auto_run(id INT);\nSELECT 42 AS answer;";
    edit_sql(cx, handle, &view, sql);
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("ctrl-s", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        sql
    );
    assert!(repository.load_views().0.is_empty());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-auto-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(repository.load_settings().unwrap().startup_sql_auto_save);
    let edited = "CREATE TABLE manual_run(id INT); SELECT 7 AS answer;";
    edit_sql(cx, handle, &view, edited);
    cx.executor().advance_clock(Duration::from_millis(1999));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        sql
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        edited
    );
    cx.update(|cx| {
        let connection = view.read(cx).database.connect().unwrap();
        assert_eq!(connection.query_row("SELECT count(*) FROM duckdb_tables() WHERE table_name IN ('should_not_auto_run', 'manual_run')", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("sql-run-all", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).session.result.row_count(), 1));
}

#[gpui_kit::test]
fn startup_sql_is_absent_from_picker_and_command_history_keeps_save_destination(
    cx: &mut TestAppContext,
) {
    use crate::application::session::SessionIdentity;
    use crate::domain::documents::{SqlConfig, ViewKind};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    repository
        .save_startup_sql("SELECT * FROM missing_table;")
        .unwrap();
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            view.read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("startup.sql")
        )
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_file_palette(window, cx));
        window.render_frame(cx);
        window.input("startup.sql", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx)
                .file_groups
                .iter()
                .flatten()
                .all(|entry| !matches!(entry, super::FileEntry::StartupSql))
        );
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(0))
                .is_none()
        );
        window.close_dialog(cx);
        view.update(cx, |view, cx| {
            view.dispatch_command(
                crate::application::commands::CommandId::StartupSql,
                window,
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.session.identity, SessionIdentity::StartupSql);
        assert!(
            view.sql_panel
                .as_ref()
                .unwrap()
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("Statement 1")
        );
    });
    edit_sql(cx, handle, &view, "SELECT 99 AS answer;");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-save-script", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        "SELECT 99 AS answer;"
    );
    let mut sql_view = DocumentDefinition::draft_table("Scratch".into(), String::new());
    sql_view.view = ViewKind::Sql(SqlConfig::default());
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.navigate_to_file(
                super::FileEntry::View(sql_view),
                false,
                false,
                false,
                window.window_handle(),
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.navigate_history(false, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).session.identity, SessionIdentity::StartupSql));
    edit_sql(cx, handle, &view, "SELECT 100 AS answer;");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-save-script", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repository.startup_sql_path()).unwrap(),
        "SELECT 100 AS answer;"
    );
    assert!(repository.load_views().0.is_empty());
    assert!(repository.load_history().unwrap().recent_sources.is_empty());
}

#[gpui_kit::test]
fn sql_editor_rows_follow_grid_font_size_changes(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut document = DocumentDefinition::draft_table("SQL".into(), String::new());
    document.view = crate::domain::documents::ViewKind::Sql(Default::default());
    let (handle, view) = viewer(cx, repository, Some(document));
    cx.run_until_parked();
    for font_size in [13, 22, 10] {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.change_font_size(font_size - i16::from(view.font.size), cx);
            });
            window.render_frame(cx);
            let editor = view
                .read(cx)
                .sql_panel
                .as_ref()
                .unwrap()
                .read(cx)
                .editor
                .read(cx)
                .state()
                .clone();
            assert_eq!(
                editor.read(cx).line_height(),
                Some(px((font_size as f32 * 1.5).ceil()))
            );
            assert_eq!(window.rem_size(), px(font_size as f32));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn sql_auto_save_debounces_persists_checkbox_and_never_executes(cx: &mut TestAppContext) {
    use crate::domain::documents::{SqlConfig, ViewKind};
    use std::time::Duration;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut saved = DocumentDefinition::draft_table("Auto SQL".into(), String::new());
    saved.view = ViewKind::Sql(SqlConfig {
        text: "SELECT 1".into(),
        ..Default::default()
    });
    repository.save_view(&mut saved).unwrap();
    let (handle, view) = viewer(cx, repository.clone(), Some(saved));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-auto-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if config.auto_save)
    );
    edit_sql(cx, handle, &view, "CREATE TABLE should_not_run(id INT)");
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if config.text == "SELECT 1")
    );
    let latest = "CREATE TABLE should_not_run(id INT); SELECT 2";
    edit_sql(cx, handle, &view, latest);
    cx.executor().advance_clock(Duration::from_millis(1999));
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if config.text == "SELECT 1")
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    let saved = repository.load_views().0.remove(0);
    assert!(
        matches!(&saved.view, ViewKind::Sql(config) if config.auto_save && config.text == latest)
    );
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(!v.session.dirty);
        assert_eq!(v.session.result.row_count(), 0);
        assert_eq!(
            v.database
                .connect()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'should_not_run'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    });
    let (_, reopened) = viewer(cx, repository.clone(), Some(saved));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            reopened
                .read(cx)
                .sql_panel
                .as_ref()
                .unwrap()
                .read(cx)
                .auto_save
        )
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-auto-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    edit_sql(cx, handle, &view, "SELECT 3");
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if !config.auto_save && config.text == latest)
    );
    cx.update(|cx| assert!(view.read(cx).session.dirty));
}

#[gpui_kit::test]
fn sql_auto_save_flushes_before_navigation_and_cancels_old_timer(cx: &mut TestAppContext) {
    use crate::domain::documents::{SqlConfig, ViewKind};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut saved = DocumentDefinition::draft_table("Auto SQL".into(), String::new());
    saved.view = ViewKind::Sql(SqlConfig {
        auto_save: true,
        text: "SELECT 1".into(),
        ..Default::default()
    });
    repository.save_view(&mut saved).unwrap();
    let (handle, view) = viewer(cx, repository.clone(), Some(saved));
    cx.run_until_parked();
    edit_sql(cx, handle, &view, "SELECT 9");
    let path = home.path().join("other.csv");
    std::fs::write(&path, "n\n1\n").unwrap();
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |v, cx| {
            v.navigate_to_file(
                super::FileEntry::Source(path),
                false,
                false,
                true,
                handle.into(),
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if config.text == "SELECT 9")
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    cx.update(|cx| assert!(!view.read(cx).session.is_sql()));
    assert_eq!(repository.load_views().0.len(), 1);
}

#[gpui_kit::test]
fn enabling_sql_auto_save_on_a_draft_names_and_saves_it_once(cx: &mut TestAppContext) {
    use crate::domain::documents::ViewKind;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    edit_sql(cx, handle, &view, "SELECT 7");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("sql-auto-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        window.click("save-confirm", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    cx.run_until_parked();
    let saved = repository.load_views().0.remove(0);
    assert!(
        matches!(&saved.view, ViewKind::Sql(config) if config.auto_save && config.text == "SELECT 7")
    );
    edit_sql(cx, handle, &view, "SELECT 8");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    let views = repository.load_views().0;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, saved.id);
    assert!(matches!(&views[0].view, ViewKind::Sql(config) if config.text == "SELECT 8"));
}

#[gpui_kit::test]
fn sql_auto_save_failure_keeps_dirty_text_and_recovers_on_next_edit(cx: &mut TestAppContext) {
    use crate::domain::documents::{SqlConfig, ViewKind};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut saved = DocumentDefinition::draft_table("Auto SQL".into(), String::new());
    saved.view = ViewKind::Sql(SqlConfig {
        auto_save: true,
        ..Default::default()
    });
    let saved_path = repository.save_view(&mut saved).unwrap();
    let (handle, view) = viewer(cx, repository.clone(), Some(saved));
    cx.run_until_parked();
    std::fs::remove_file(&saved_path).unwrap();
    std::fs::create_dir(&saved_path).unwrap();
    edit_sql(cx, handle, &view, "SELECT 9");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.session.dirty);
        assert!(view.error.as_ref().unwrap().contains("SQL auto-save failed"));
        assert!(matches!(&view.session.working.as_ref().unwrap().view, ViewKind::Sql(config) if config.text == "SELECT 9"));
    });
    std::fs::remove_dir(&saved_path).unwrap();
    edit_sql(cx, handle, &view, "SELECT 10");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert!(
        matches!(&repository.load_views().0[0].view, ViewKind::Sql(config) if config.text == "SELECT 10")
    );
    cx.update(|cx| {
        assert!(!view.read(cx).session.dirty);
        assert!(view.read(cx).error.is_none());
    });
}

#[gpui_kit::test]
fn switching_from_saved_sql_with_file_picker_restores_grid_focus(cx: &mut TestAppContext) {
    check_sql_file_picker_focus(cx, false);
}

#[gpui_kit::test]
fn switching_from_hidden_sql_with_file_picker_restores_grid_focus(cx: &mut TestAppContext) {
    check_sql_file_picker_focus(cx, true);
}

fn check_sql_file_picker_focus(cx: &mut TestAppContext, hide_panel: bool) {
    use crate::domain::documents::{SqlConfig, ViewKind};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("books.csv");
    std::fs::write(&path, "title\nFirst\nSecond\n").unwrap();
    let mut table =
        DocumentDefinition::draft_table("Books".into(), path.to_string_lossy().into_owned());
    repository.save_view(&mut table).unwrap();
    let mut scratch = DocumentDefinition::draft_table("Scratch".into(), String::new());
    scratch.view = ViewKind::Sql(SqlConfig {
        text: "SELECT 42 AS answer".into(),
        ..Default::default()
    });
    repository.save_view(&mut scratch).unwrap();
    let (handle, view) = viewer(cx, repository, Some(table));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-p", cx);
        window.render_frame(cx);
        window.input("Scratch", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).session.is_sql());
        window.render_frame(cx);
        window.press("ctrl-j", cx);
        window.render_frame(cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let v = view.read(cx);
        let panel = v.sql_panel.as_ref().unwrap().read(cx);
        assert_eq!(
            v.session.result.row_count(),
            1,
            "hidden={hide_panel}, status={}, error={:?}, text={}",
            panel.status,
            panel.error,
            panel.sql_to_run(true, cx)
        );
        window.render_frame(cx);
        if hide_panel {
            window.click("sql-close", cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).sql_panel_open, !hide_panel);
        window.press("ctrl-p", cx);
        window.render_frame(cx);
        window.input("Books", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let v = view.read(cx);
        assert!(!v.session.is_sql());
        assert_eq!(v.session.result.row_count(), 2);
        assert!(
            v.grid.read(cx).focus_handle(cx).is_focused(window),
            "grid lost focus after SQL navigation (hidden={hide_panel})"
        );
        window.press("ctrl-p", cx);
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn sql_panel_shortcuts_preserve_grid_and_run_current_statement(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
        assert!(view.read(cx).session.is_sql());
        assert!(view.read(cx).sql_panel_open);
        assert_eq!(view.read(cx).session.result.row_count(), 2);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |state, cx| {
            state.set_value("SELECT 10 AS first; SELECT 20 AS second", window, cx);
            state.set_selected_range(35..35, cx);
        });
        panel.update(cx, |p, cx| p.focus(window, cx));
        window.render_frame(cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let v = view.read(cx);
        assert!(!v.sql_running);
        assert_eq!(v.session.result.row_count(), 1);
        assert_eq!(v.session.result.columns[0].0, "second");
        let panel = v.sql_panel.clone().unwrap();
        assert!(panel.read(cx).error.is_none(), "{:?}", panel.read(cx).error);
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(
            view.read(cx)
                .grid
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        window.press("ctrl-j", cx);
        assert!(view.read(cx).sql_panel_open);
        window.render_frame(cx);
        window.click("sql-close", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!view.read(cx).sql_panel_open);
        assert!(view.read(cx).sql_panel.is_some());
        window.press("ctrl-j", cx);
        assert!(view.read(cx).sql_panel_open);
        let split = view.read(cx).sql_split.clone();
        window.render_frame(cx);
        split.update(cx, |s, cx| s.resize_panel(1, px(180.), window, cx));
        window.render_frame(cx);
        assert!((split.read(cx).sizes()[1].as_f32() - 180.).abs() < 2.);
    })
    .unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let saved = repository
        .load_settings()
        .unwrap()
        .sql_editor_height
        .unwrap();
    assert!((saved - 180.).abs() < 2., "{saved}");
    let (handle, view) = viewer(cx, repository, None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
        window.render_frame(cx);
        let split = view.read(cx).sql_split.clone();
        assert!((split.read(cx).sizes()[1].as_f32() - saved).abs() < 2.);
    })
    .unwrap();
}

#[gpui_kit::test]
fn sql_search_sort_and_save_never_reexecute_statements(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |s, cx| {
            s.set_value(
                "CREATE TABLE apples(id BIGINT); INSERT INTO apples VALUES (2),(1) RETURNING *",
                window,
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.run_sql(true, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            assert_eq!(v.session.result.row_count(), 2);
            v.session.set_sort_direction("id".into(), false);
            v.set_search_text("1".into(), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            assert_eq!(v.session.result.row_count(), 1);
            v.set_search_text(String::new(), window, cx);
            let panel = v.sql_panel.clone().unwrap();
            let state = panel.read(cx).editor.read(cx).state().clone();
            state.update(cx, |s, cx| {
                s.set_value("SELECT * FROM apples", window, cx);
                // Programmatic set_value is silent; model the event emitted by user editing.
                cx.emit(gpui_kit::component::input::InputEvent::Change);
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |v, _| {
            v.session
                .save_as("Fruit SQL".into(), &v.repository)
                .unwrap();
        });
    })
    .unwrap();
    cx.run_until_parked();
    let saved = cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.session.result.row_count(), 2);
        assert_eq!(
            v.session.result.fetch_rows(0, 2).unwrap()[0].values[0].as_deref(),
            Some("1")
        );
        assert_eq!(
            v.database
                .connect()
                .unwrap()
                .query_row("SELECT count(*) FROM apples", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let documents = repository.load_views().0;
        assert_eq!(documents.len(), 1);
        assert!(documents[0].source_path.is_empty());
        documents[0].clone()
    });
    let (reopened_handle, reopened) = viewer(cx, repository, Some(saved));
    cx.run_until_parked();
    cx.update(|cx| {
        let v = reopened.read(cx);
        assert!(v.sql_panel_open);
        assert_eq!(v.session.result.row_count(), 0);
        assert!(!v.session.dirty);
        assert!(!v.sql_running);
        assert_eq!(
            v.sql_panel.as_ref().unwrap().read(cx).sql_to_run(true, cx),
            "SELECT * FROM apples"
        );
    });
    cx.update_window(reopened_handle.into(), |_, window, cx| {
        reopened.update(cx, |v, cx| v.run_sql(true, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let v = reopened.read(cx);
        assert!(
            v.sql_panel.as_ref().unwrap().read(cx).error.is_none(),
            "{:?}",
            v.sql_panel.as_ref().unwrap().read(cx).error
        );
        assert_eq!(
            v.session.result.row_count(),
            2,
            "{}; running {}",
            v.sql_panel.as_ref().unwrap().read(cx).status,
            v.sql_running
        );
        assert_eq!(v.session.sort.len(), 1);
        assert_eq!(
            v.session.result.fetch_rows(0, 2).unwrap()[0].values[0].as_deref(),
            Some("1")
        );
    });
}

#[gpui_kit::test]
fn sql_completion_escape_and_selection_are_editor_local(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |s, cx| {
            s.set_value("SELECT 1; SELECT 2", window, cx);
            s.set_selected_range(0..8, cx);
        });
        assert_eq!(panel.read(cx).sql_to_run(false, cx), "SELECT 1");
        panel.update(cx, |p, cx| p.focus(window, cx));
        state.update(cx, |s, cx| {
            s.present_completion_items(
                0,
                "",
                vec![lsp_types::CompletionItem {
                    label: "apples".into(),
                    ..Default::default()
                }],
                cx,
            )
        });
        window.render_frame(cx);
        window.press("escape", cx);
        assert!(!state.read(cx).completion_menu_state().open);
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
        window.press("tab", cx);
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn sql_alias_completion_triggers_on_dot_and_inserts_only_column(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        *panel.read(cx).catalog.write().unwrap() = vec![crate::domain::sql_editor::Relation {
            catalog: "db".into(),
            schema: "main".into(),
            name: "apples".into(),
            columns: vec!["color".into()],
            is_view: false,
        }];
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |s, cx| {
            s.set_value("SELECT a\nFROM apples a", window, cx);
            s.set_selected_range(8..8, cx);
        });
        panel.update(cx, |p, cx| p.focus(window, cx));
        window.render_frame(cx);
        window.input(".", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        window.render_frame(cx);
        assert!(state.read(cx).completion_menu_state().open);
        assert_eq!(
            state.read(cx).completion_menu_state().items[0].label,
            "color"
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        assert_eq!(
            state.read(cx).value().as_ref(),
            "SELECT a.\"color\"\nFROM apples a"
        );
        let v = view.read(cx);
        assert!(v.session.dirty);
        let crate::domain::documents::ViewKind::Sql(config) =
            &v.session.working.as_ref().unwrap().view
        else {
            panic!("expected SQL document")
        };
        assert_eq!(config.text, "SELECT a.\"color\"\nFROM apples a");
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn sql_path_completion_lists_search_roots_and_tab_accepts_into_folders(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("sales data")).unwrap();
    std::fs::write(data.path().join("sales data").join("it's.csv"), "a\n1\n").unwrap();
    std::fs::write(data.path().join("sales data").join("q1.csv"), "a\n1\n").unwrap();
    std::fs::write(data.path().join("summary.txt"), "").unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let panel = cx
        .update_window(handle.into(), |_, window, cx| {
            let panel = view.read(cx).sql_panel.clone().unwrap();
            *panel.read(cx).path_roots.write().unwrap() =
                crate::infrastructure::path_completion::PathRoots {
                    search: vec![data.path().to_path_buf()],
                    ..Default::default()
                };
            let state = panel.read(cx).editor.read(cx).state().clone();
            state.update(cx, |s, cx| {
                s.set_value("SELECT * FROM '", window, cx);
                s.set_selected_range(15..15, cx);
            });
            panel.update(cx, |p, cx| p.focus(window, cx));
            window.render_frame(cx);
            window.input("s", cx);
            panel
        })
        .unwrap();
    let state = cx.update(|cx| panel.read(cx).editor.read(cx).state().clone());
    let labels = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let menu = state.read(cx).completion_menu_state();
            assert!(menu.open);
            menu.items
                .iter()
                .map(|item| item.label.clone())
                .collect::<Vec<_>>()
        })
    };
    cx.run_until_parked();
    assert_eq!(labels(cx), ["sales data/", "summary.txt"]);
    for (expected_text, expected_items) in [
        (
            "SELECT * FROM 'sales data/",
            Some(vec!["it's.csv", "q1.csv"]),
        ),
        ("SELECT * FROM 'sales data/it''s.csv", None),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("tab", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| state.read(cx).value().to_string()),
            expected_text
        );
        if let Some(items) = expected_items {
            assert_eq!(labels(cx), items);
        }
    }
    // With no menu open, Tab keeps its editing meaning.
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!state.read(cx).completion_menu_state().open);
        window.press("tab", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_ne!(
        cx.update(|cx| state.read(cx).value().to_string()),
        "SELECT * FROM 'sales data/it''s.csv"
    );
}

#[gpui_kit::test]
fn sql_completion_retriggers_before_an_earlier_completion(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("empty")).unwrap();
    std::fs::write(data.path().join("sales.csv"), "a\n1\n").unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let panel = cx.update(|cx| view.read(cx).sql_panel.clone().unwrap());
    let state = cx.update(|cx| panel.read(cx).editor.read(cx).state().clone());
    cx.update(|cx| {
        *panel.read(cx).path_roots.write().unwrap() =
            crate::infrastructure::path_completion::PathRoots {
                search: vec![data.path().to_path_buf()],
                ..Default::default()
            };
    });
    let text = "SELECT * FROM 'empty/";
    cx.update_window(handle.into(), |_, window, cx| {
        state.update(cx, |s, cx| {
            s.set_value("SELECT * FROM 'empty", window, cx);
            s.set_selected_range(20..20, cx);
        });
        panel.update(cx, |p, cx| p.focus(window, cx));
        window.render_frame(cx);
        // An empty folder: the trigger finds nothing.
        window.press("/", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(state.read(cx).value().as_ref(), text);
        assert!(!state.read(cx).completion_menu_state().open);
        // Start over at the beginning of the text and type a new path query.
        state.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_selected_range(0..0, cx);
        });
        window.render_frame(cx);
        for key in ["f", "r", "o", "m", "space", "'", "s"] {
            window.press(key, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let menu = state.read(cx).completion_menu_state();
        assert!(menu.open, "{:?}", state.read(cx).value());
        assert_eq!(menu.items[0].label, "sales.csv");
    });
}

#[gpui_kit::test]
fn sql_results_filter_every_row_undo_and_prune_missing_columns(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let (handle, view) = viewer(cx, repository, None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let run = |cx: &mut TestAppContext, sql: &str| {
        edit_sql(cx, handle, &view, sql);
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |v, cx| v.run_sql(true, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
    };
    let rows = |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).session.result.row_count());
    let filter_first_cell = |cx: &mut TestAppContext, exclude: bool| {
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.grid.update(cx, |grid, cx| {
                    grid.select(crate::domain::selection::Cell { row: 0, column: 0 }, false, cx)
                });
                v.filter_active_value(exclude, cx);
            })
        });
        cx.run_until_parked();
    };
    // Setup runs first; the last query keeps all six rows despite the limit of 2.
    run(
        cx,
        "CREATE OR REPLACE TEMP TABLE fruit AS SELECT * FROM (VALUES \
         ('red', 1), ('green', 2), ('red', 3), ('gold', 4), ('red', 5), ('green', 6)) t(color, n);\n\
         SELECT color, n FROM fruit ORDER BY n;",
    );
    assert!(cx.update(|cx| view.read(cx).sql_result_complete));
    assert_eq!(rows(cx), 6);
    let add_slicer = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |v, cx| {
                v.error = None;
                v.slicer_picker = None;
                v.dispatch_command(crate::application::commands::CommandId::AddSlicer, window, cx);
                (v.slicer_picker.is_some(), v.error.clone())
            })
        })
        .unwrap()
    };
    assert_eq!(add_slicer(cx), (true, None));
    cx.update(|cx| view.update(cx, |v, _| v.slicer_picker = None));
    // `=` on "red" finds all three reds, two of them past the row limit.
    filter_first_cell(cx, false);
    cx.update(|cx| assert!(view.read(cx).error.is_none(), "{:?}", view.read(cx).error));
    assert_eq!(rows(cx), 3);
    cx.update(|cx| view.update(cx, |v, cx| v.undo_view_change(false, cx)));
    cx.run_until_parked();
    assert_eq!(rows(cx), 6);
    // `-` excludes; the SQL text is untouched by undo.
    filter_first_cell(cx, true);
    assert_eq!(rows(cx), 3);
    cx.update(|cx| {
        let v = view.read(cx);
        let crate::domain::documents::ViewKind::Sql(config) =
            &v.session.working.as_ref().unwrap().view
        else {
            panic!("expected SQL")
        };
        assert!(config.text.contains("SELECT color, n"));
    });
    // A rerun without the filtered column drops the filter instead of failing.
    run(cx, "SELECT n FROM fruit");
    cx.update(|cx| assert!(view.read(cx).error.is_none(), "{:?}", view.read(cx).error));
    assert_eq!(rows(cx), 6);
    // A writing statement keeps the row limit and explains why filters are off.
    run(cx, "INSERT INTO fruit VALUES ('blue', 7) RETURNING color");
    assert!(!cx.update(|cx| view.read(cx).sql_result_complete));
    let (opened, error) = add_slicer(cx);
    assert!(!opened && error.is_some_and(|error| error.contains("last statement")));
    filter_first_cell(cx, false);
    cx.update(|cx| {
        let error = view.read(cx).error.clone().unwrap_or_default();
        assert!(error.contains("last statement"), "{error}");
        let panel = view.read(cx).sql_panel.clone().unwrap();
        assert!(panel.read(cx).status.contains("filters are off"));
    });
}

fn ask_state(view: &Entity<DataFrameView>, cx: &gpui_kit::App) -> String {
    use crate::ui::sql_panel::AskState;
    match &view.read(cx).sql_panel.as_ref().unwrap().read(cx).ask {
        AskState::Idle => "idle".into(),
        AskState::Working(text) => format!("working: {text}"),
        AskState::Proposal(proposal) => format!("proposal: {}", proposal.sql),
        AskState::Failed(error) => format!("failed: {error}"),
    }
}

#[gpui_kit::test]
fn ask_ai_writes_checked_sql_and_tab_inserts_it(cx: &mut TestAppContext) {
    use crate::domain::ai::{AiProvider, SqlCheck};
    use crate::infrastructure::ai_client::test_server::{reply, serve};
    use crate::ui::sql_panel::AskState;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
    })
    .unwrap();
    cx.run_until_parked();
    edit_sql(cx, handle, &view, "CREATE TEMP TABLE fruit AS SELECT 'red' AS color;");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();

    // Without a provider, the strip says where to add one.
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        state.update(cx, |state, cx| {
            let end = state.value().len();
            state.set_selected_range(end..end, cx);
        });
        panel.update(cx, |panel, cx| panel.focus(window, cx));
        window.render_frame(cx);
        window.press("ctrl-k", cx);
        let panel = view.read(cx).sql_panel.clone().unwrap();
        assert!(panel.read(cx).ask_open);
        assert!(panel.read(cx).ask_input.read(cx).focus_handle(cx).is_focused(window));
        window.render_frame(cx);
        window.input("colors", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = ask_state(&view, cx);
        assert!(state.starts_with("failed: No AI provider"), "{state}");
    });

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
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let requests = server.join().unwrap();
    assert!(requests[0].contains("temp.fruit: color VARCHAR"), "{}", requests[0]);
    assert!(requests[0].contains("CREATE TEMP TABLE fruit"));
    assert!(requests[1].contains("DuckDB rejected"), "{}", requests[1]);
    assert!(requests[1].contains("colour"));
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let AskState::Proposal(proposal) = &panel.read(cx).ask else {
            panic!("{}", ask_state(&view, cx));
        };
        assert_eq!(proposal.sql, "SELECT color FROM fruit");
        assert_eq!(proposal.check, SqlCheck::Passed);
        // The log holds both rounds: sent, received, and DuckDB's verdict.
        use crate::domain::ai::AiEvent;
        let log = &panel.read(cx).ask_log;
        assert_eq!(log.len(), 6, "{log:?}");
        assert!(matches!(&log[2], AiEvent::Checked(SqlCheck::Failed(error)) if error.contains("colour")));
        assert_eq!(log[5], AiEvent::Checked(SqlCheck::Passed));
        window.render_frame(cx);
        window.click("sql-ask-log-toggle", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        assert!(panel.read(cx).show_log);
        window.render_frame(cx);
        assert!(window.try_find("sql-ask-insert").is_some());
        // The click moved focus; Tab accepts from the prompt.
        let focus = panel.read(cx).ask_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        window.press("tab", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        let expected = "CREATE TEMP TABLE fruit AS SELECT 'red' AS color;\nSELECT color FROM fruit";
        assert_eq!(state.read(cx).value().to_string(), expected);
        // The new SQL is selected so Ctrl+Enter runs just it.
        let start = expected.find("SELECT color").unwrap();
        assert_eq!(state.read(cx).selected_range(), start..expected.len());
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
        assert!(!panel.read(cx).ask_open);
        let working = view.read(cx).session.working.clone().unwrap();
        let crate::domain::documents::ViewKind::Sql(config) = working.view else {
            panic!("not a SQL view");
        };
        assert_eq!(config.text, expected);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.session.result.columns[0].0, "color");
        assert_eq!(view.session.result.row_count(), 1);
    });

    // Escape closes the strip and returns to the editor.
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("ctrl-k", cx);
        window.render_frame(cx);
        window.press("escape", cx);
        let panel = view.read(cx).sql_panel.clone().unwrap();
        assert!(!panel.read(cx).ask_open);
        let state = panel.read(cx).editor.read(cx).state().clone();
        assert!(state.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn ask_sql_command_opens_the_editor_with_the_prompt_focused(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            view.dispatch_command(crate::application::commands::CommandId::AskSql, window, cx)
        });
        assert!(view.read(cx).session.is_sql());
        let panel = view.read(cx).sql_panel.clone().unwrap();
        assert!(panel.read(cx).ask_open);
        assert!(panel.read(cx).ask_input.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

fn sql_config(view: &Entity<DataFrameView>, cx: &gpui_kit::App) -> crate::domain::documents::SqlConfig {
    match &view.read(cx).session.working.as_ref().unwrap().view {
        crate::domain::documents::ViewKind::Sql(config) => config.clone(),
        _ => panic!("not a SQL view"),
    }
}

fn run_all_sql(cx: &mut TestAppContext, handle: WindowHandle<Root>, view: &Entity<DataFrameView>, sql: &str) {
    edit_sql(cx, handle, view, sql);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.run_sql(true, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn sql_views_freeze_leading_columns_without_reordering(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(cx, ViewRepository::new(StorePaths::for_home(home.path())), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
    })
    .unwrap();
    cx.run_until_parked();
    run_all_sql(cx, handle, &view, "SELECT 1 AS region, 2 AS product, 3 AS \"2024\"");
    let frozen = |cx: &mut TestAppContext| {
        cx.update(|cx| (sql_config(&view, cx).frozen_columns, view.read(cx).grid.read(cx).frozen_count()))
    };
    // F on the second column freezes the first two.
    cx.update_window(handle.into(), |_, window, cx| {
        window.focus(&view.read(cx).grid.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.press("right", cx);
        window.press("f", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(frozen(cx), (vec!["region".to_owned(), "product".to_owned()], 2));
    cx.update(|cx| {
        let columns = &view.read(cx).session.result.columns;
        assert_eq!(columns.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["region", "product", "2024"]);
        assert!(view.read(cx).session.dirty);
    });
    // Again on it: it unfreezes, the first stays. Undo brings it back.
    cx.update_window(handle.into(), |_, window, cx| window.press("f", cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(frozen(cx), (vec!["region".to_owned()], 1));
    cx.update_window(handle.into(), |_, window, cx| window.press("ctrl-z", cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(frozen(cx).1, 2);
    // A rerun keeps only the part still leading the result.
    run_all_sql(cx, handle, &view, "SELECT 1 AS region, 2 AS month, 3 AS product");
    assert_eq!(frozen(cx).1, 1);
    cx.update_window(handle.into(), |_, window, cx| {
        window.focus(&view.read(cx).grid.read(cx).focus_handle(cx), cx);
        window.press("shift-f", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(frozen(cx), (Vec::new(), 0));
}

#[gpui_kit::test]
fn auto_run_sql_views_run_when_opened(cx: &mut TestAppContext) {
    use crate::domain::documents::{SqlConfig, ViewKind};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut saved = DocumentDefinition::draft_table("Report".into(), String::new());
    saved.view = ViewKind::Sql(SqlConfig {
        text: "SELECT 7 AS lucky".into(),
        ..Default::default()
    });
    repository.save_view(&mut saved).unwrap();
    // Off by default: opening shows the SQL without running it.
    let (handle, view) = viewer(cx, repository.clone(), Some(saved.clone()));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_ne!(view.read(cx).session.result.columns.first().map(|c| c.0.as_str()), Some("lucky"));
        window.render_frame(cx);
        window.click("sql-auto-run", cx);
    })
    .unwrap();
    cx.run_until_parked();
    // The checkbox is stored with the saved view at once.
    let stored = repository.load_views().0.into_iter().find(|v| v.name == "Report").unwrap();
    let ViewKind::Sql(config) = &stored.view else { panic!() };
    assert!(config.auto_run);
    let (_, view) = viewer(cx, repository, Some(stored));
    cx.run_until_parked();
    cx.update(|cx| {
        let result = &view.read(cx).session.result;
        assert_eq!(result.columns[0].0, "lucky");
        assert_eq!(result.row_count(), 1);
        assert!(view.read(cx).sql_panel.as_ref().unwrap().read(cx).auto_run);
    });
}

#[gpui_kit::test]
fn sql_editor_indents_four_spaces(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) = viewer(cx, ViewRepository::new(StorePaths::for_home(home.path())), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("tab", cx);
        window.input("x", cx);
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let state = panel.read(cx).editor.read(cx).state().clone();
        assert_eq!(state.read(cx).value().to_string(), "    x");
    })
    .unwrap();
}

#[gpui_kit::test]
fn ask_ai_at_mentions_complete_and_send_only_those_tables(cx: &mut TestAppContext) {
    use crate::domain::ai::AiProvider;
    use crate::infrastructure::ai_client::test_server::{reply, serve};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx, repository.clone(), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-j", cx);
    })
    .unwrap();
    cx.run_until_parked();
    run_all_sql(
        cx,
        handle,
        &view,
        "CREATE TEMP TABLE fruit AS SELECT * FROM (VALUES ('red', 1), ('green', 2)) t(color, n); COMMENT ON COLUMN fruit.color IS 'Skin color'; CREATE TEMP TABLE other AS SELECT 1 AS secret_column",
    );
    let ask = |cx: &mut TestAppContext, send_data: bool, typed: &str| {
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
        let typed = typed.to_owned();
        cx.update_window(handle.into(), |_, window, cx| {
            let panel = view.read(cx).sql_panel.clone().unwrap();
            panel.update(cx, |panel, cx| {
                panel.ask_input.update(cx, |input, cx| input.set_value("", window, cx));
                panel.open_ask(window, cx);
            });
            window.render_frame(cx);
            window.input(&typed, cx);
        })
        .unwrap();
        cx.run_until_parked();
        server
    };

    // `@` lists tables; Tab inserts the SQL name and a space.
    let server = ask(cx, false, "colors in @fru");
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let provider = panel.read(cx).ask_provider.clone().unwrap();
        assert_eq!((provider.name.as_str(), provider.send_data), ("local", false));
        let items = panel.read(cx).mentions.read(cx).items().unwrap();
        assert_eq!(items, ["temp.fruit"]);
        window.press("tab", cx);
        assert_eq!(panel.read(cx).ask_input.read(cx).value(), "colors in @temp.fruit ");
        // Tab completed the name; it did not accept anything else.
        assert!(panel.read(cx).ask_open);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let request = server.join().unwrap().remove(0);
    assert!(request.contains("Relations the user named"), "{request}");
    assert!(request.contains("temp.fruit · 2 rows"), "{request}");
    assert!(request.contains("color VARCHAR [Skin color]: ~2 distinct"), "{request}");
    // Counts only without the opt-in; other tables only by name.
    assert!(!request.contains("values:") && !request.contains("min '"), "{request}");
    // (Its column name is still in the editor SQL sent as context.)
    assert!(request.contains("not described): temp.other"), "{request}");
    assert!(!request.contains("secret_column INTEGER"));

    // With data allowed, the few values are listed.
    let server = ask(cx, true, "colors in @temp.fruit please");
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx)).unwrap();
    cx.run_until_parked();
    let request = server.join().unwrap().remove(0);
    assert!(
        request.contains("color VARCHAR [Skin color]: 2 distinct, 0.0% null; min 'green', max 'red'; values: 'green' (1), 'red' (1)"),
        "{request}"
    );
    cx.update(|cx| {
        let panel = view.read(cx).sql_panel.clone().unwrap();
        let log = &panel.read(cx).ask_log;
        assert!(log.iter().any(|event| matches!(event, crate::domain::ai::AiEvent::Note(text) if text.contains("with data values"))));
    });
}
