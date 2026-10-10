use super::{DataFrameView, FileEntry};
use crate::application::commands::CommandId;
use crate::domain::documents::ViewAction;
use crate::domain::frame::FrameSnapshot;
use crate::domain::selection::Cell;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, Focusable, TestAppContext, WindowHandle, px, size};

fn viewer(
    cx: &mut TestAppContext,
    path: &std::path::Path,
) -> (WindowHandle<Root>, Entity<DataFrameView>) {
    let home = path.parent().unwrap().to_path_buf();
    let repository = ViewRepository::new(StorePaths::for_home(&home));
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
    });
    let mut entity = None;
    let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let view = cx.new(|cx| {
            DataFrameView::new(source, None, String::new(), None, repository, window, cx)
        });
        entity = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        entity.as_ref().unwrap().update(cx, |view, cx| {
            view.navigate_to_file(
                FileEntry::Source(path.to_path_buf()),
                false,
                false,
                false,
                handle.into(),
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    (handle, entity.unwrap())
}

fn press_keys(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entity: &Entity<DataFrameView>,
    keys: &[&str],
) {
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        grid.update(cx, |grid, cx| {
            grid.select(Cell { row: 1, column: 1 }, false, cx)
        });
        for key in keys {
            window.press(key, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn space_x_runs_the_keyed_action_for_the_active_row(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,city\nAda,London\nO'Brien & Co,New York\n").unwrap();
    let (handle, entity) = viewer(cx, &path);

    // Without actions, the menu key opens the editor instead.
    press_keys(cx, handle, &entity, &["space", "x"]);
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(!entity.read(cx).action_menu_open);
        window.close_dialog(cx);
    })
    .unwrap();

    cx.update(|cx| {
        entity.update(cx, |view, _| {
            view.ensure_working_view();
            view.session
                .edit_definition(|definition| {
                    definition.actions = vec![
                        ViewAction {
                            key: "M".into(),
                            description: "Map".into(),
                            command: "https://maps.example/?q={name} {city}".into(),
                        },
                        ViewAction {
                            key: "b".into(),
                            description: "Broken".into(),
                            command: "datadiver-missing-program {name}".into(),
                        },
                    ]
                })
                .unwrap();
        })
    });
    cx.run_until_parked();

    press_keys(cx, handle, &entity, &["space", "x"]);
    cx.update(|cx| assert!(entity.read(cx).action_menu_open));
    // Lowercase m is not the action's key, so the menu stays open.
    press_keys(cx, handle, &entity, &["m"]);
    cx.update(|cx| assert!(entity.read(cx).action_menu_open));
    assert_eq!(cx.opened_url(), None);
    press_keys(cx, handle, &entity, &["shift-m"]);
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://maps.example/?q=O%27Brien%20%26%20Co%20New%20York")
    );
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(!view.action_menu_open);
        assert!(view.error.is_none(), "{:?}", view.error);
    });

    press_keys(cx, handle, &entity, &["space", "x", "b"]);
    cx.update(|cx| {
        let error = entity.read(cx).error.clone().unwrap();
        assert!(error.contains("Broken") && error.contains("datadiver-missing-program"));
    });

    press_keys(cx, handle, &entity, &["space", "x", "escape"]);
    cx.update(|cx| assert!(!entity.read(cx).action_menu_open));

    // The editor's Run button uses the active row too, here the first one.
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        grid.update(cx, |grid, cx| {
            grid.select(Cell { row: 0, column: 0 }, false, cx)
        });
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::EditActions, window, cx)
        });
        assert!(window.has_active_dialog(cx));
        window.render_frame(cx);
        window.click(("view-action-run", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://maps.example/?q=Ada%20London")
    );
}
