use super::test_support::Viewer;
use crate::application::commands::CommandId;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::point;

const SALES: &str = "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n";

fn sales(home: &std::path::Path) -> std::path::PathBuf {
    let path = home.join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    path
}

/// Presses M in the grid and returns the manager's window.
fn open_manager(viewer: &Viewer) -> Viewer {
    viewer.render();
    viewer.press("m");
    viewer.run_until_parked();
    let id = viewer.read(|view| view.format_window.as_ref().map(|window| window.id)).expect("the manager opened");
    let manager = viewer.window(id);
    assert!(manager.read(|view| view.format_window.as_ref().unwrap().manager.is_some()));
    manager.render();
    manager
}

/// Whether each shown row's `column` is bold.
fn bold(viewer: &Viewer, column: usize) -> Vec<bool> {
    // Statistics arrive from workers as the grid paints.
    for _ in 0..3 {
        viewer.settle();
    }
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let result = &view.session.result;
        let rows = result.fetch_rows(0, result.row_count()).unwrap();
        let Some(formatting) = view.grid.formatting() else {
            return vec![false; rows.len()];
        };
        rows.iter()
            .enumerate()
            .map(|(index, row)| formatting.style(result.column_indices[column], row, index).bold)
            .collect()
    })
}

#[test]
fn m_opens_the_manager_and_duplicates_count_the_filtered_rows_not_the_search() {
    let home = tempfile::tempdir().unwrap();
    let path = sales(home.path());
    let viewer = Viewer::file(&path);
    viewer.select(0, 1);
    viewer.render();
    let manager = open_manager(&viewer);
    let opened = manager.read(|view| view.format_window.as_ref().unwrap().manager.as_ref().unwrap().formats.clone());
    assert_eq!(opened[0].columns, ["kind"]);
    manager.input("if duplicate(c) then bold");
    viewer.settle();
    // `a` is on three rows, `b` on one.
    assert_eq!(bold(&viewer, 1), [true, false, true, true]);
    // The first Escape closes the completion list typing opened.
    manager.press("escape escape");
    viewer.run_until_parked();
    assert!(!manager.is_open());
    assert!(viewer.read(|view| view.format_window.is_none() && view.dialog.is_none()));

    // A search narrows the rows shown, not the rows counted.
    viewer.update(|view, cx| view.set_search_text("South".into(), cx));
    viewer.settle();
    assert_eq!(bold(&viewer, 1), [true]);
    viewer.update(|view, cx| view.set_search_text(String::new(), cx));
    viewer.settle();

    // A filter does narrow them: in the South there is one `a`.
    viewer.select(2, 0);
    viewer.render();
    viewer.update(|view, cx| view.dispatch_command(CommandId::FilterToValue, cx));
    viewer.settle();
    assert_eq!(bold(&viewer, 1), [false]);

    // The file remembers its formats.
    viewer.update(|view, cx| view.autosave_table_preferences(cx));
    viewer.run_until_parked();
    let saved = viewer.read(|view| view.repository.load_table_preferences(&path).unwrap());
    assert_eq!(saved.formats.len(), 1);
    assert_eq!(saved.formats[0].rules, "if duplicate(c) then bold");
}

#[test]
fn the_grid_redraws_the_manager_only_when_what_it_shows_changes() {
    let home = tempfile::tempdir().unwrap();
    let path = sales(home.path());
    let viewer = Viewer::file(&path);
    let manager = open_manager(&viewer);
    manager.input("if duplicate(c) then bold");
    viewer.settle();
    manager.render();
    assert!(!viewer.app.is_dirty(manager.id));
    // Moving through the grid, as scrolling does, redraws only the grid.
    viewer.press("down down");
    assert!(viewer.app.is_dirty(viewer.id));
    viewer.render();
    assert!(!viewer.app.is_dirty(manager.id));
    // The formats changing elsewhere redraw the manager too.
    viewer.update(|view, _| view.session.edit_definition(|definition| definition.formats.clear()).unwrap());
    viewer.render();
    assert!(viewer.app.is_dirty(manager.id));
    manager.render();
    let shown = manager.read(|view| view.format_window.as_ref().unwrap().manager.as_ref().unwrap().formats.clone());
    assert!(shown.iter().all(|format| format.rules.is_empty()), "{shown:?}");
    viewer.render();
    assert!(!viewer.app.is_dirty(manager.id));
}

#[test]
fn every_kind_of_mark_paints() {
    let home = tempfile::tempdir().unwrap();
    let path = sales(home.path());
    let viewer = Viewer::file(&path);
    viewer.update(|view, cx| {
        view.ensure_working_view();
        view.session
            .edit_definition(|definition| {
                definition.formats = vec![
                    crate::domain::documents::ConditionalFormat {
                        rules: "scale(white, green); bar(blue, fill=gradient); icons(stars); \
                                if c > 10 then border: 2px dashed red, underline, strike, italic; \
                                sparkline(\"n\", \"n\"); if rank(c) = 1 then bold"
                            .into(),
                        columns: vec!["n".into()],
                    },
                    crate::domain::documents::ConditionalFormat {
                        rules: "icon: check; if c = 'b' then hide_text, border: 1px dotted".into(),
                        columns: vec!["kind".into()],
                    },
                ]
            })
            .unwrap();
        view.refresh_formatting(cx);
    });
    // The highest n, 30, ranks first.
    assert_eq!(bold(&viewer, 2), [false, true, false, false]);
    let style = viewer.read(|view| {
        let result = &view.session.result;
        let rows = result.fetch_rows(0, 1).unwrap();
        view.grid.formatting().unwrap().style(result.column_indices[2], &rows[0], 0)
    });
    assert!(style.background.is_some() && style.bar.is_some() && style.icon.is_some() && style.sparkline.is_some());
}

#[test]
fn the_manager_reopens_where_it_was_beside_its_window_and_closes_with_it() {
    let home = tempfile::tempdir().unwrap();
    let path = sales(home.path());
    let viewer = Viewer::file(&path);
    viewer.app.move_to(viewer.id, 10.0, 10.0);
    let manager = open_manager(&viewer);
    // A second M brings the open manager forward rather than opening another.
    viewer.press("m");
    viewer.run_until_parked();
    assert_eq!(viewer.others().len(), 1);
    manager.app.move_to(manager.id, 50.0, 40.0);
    manager.resize(500.0, 300.0);
    manager.press("escape");
    viewer.run_until_parked();
    assert!(!manager.is_open());
    let saved = viewer.read(|view| view.repository.load_history().unwrap().format_window.unwrap());
    assert_eq!((saved.x, saved.y, saved.width, saved.height), (40.0, 30.0, 500.0, 300.0));

    // It keeps that offset when the grid's window has moved.
    viewer.app.move_to(viewer.id, 60.0, 40.0);
    let manager = open_manager(&viewer);
    let placement = manager.app.update(manager.id, |_, cx| cx.placement()).unwrap();
    assert_eq!((placement.x, placement.y, placement.width, placement.height), (100.0, 70.0, 500.0, 300.0));

    // Closing the grid's window closes the manager first.
    assert!(viewer.request_close());
    assert!(!manager.is_open());
}

#[test]
fn presets_open_from_a_scrolling_list_and_edits_show_in_the_grid() {
    let home = tempfile::tempdir().unwrap();
    let path = sales(home.path());
    let viewer = Viewer::file(&path);
    viewer.select(0, 1);
    let manager = open_manager(&viewer);
    manager.resize(700.0, 360.0);
    assert!(manager.find(|hit| matches!(hit, Hit::Button(Button::FormatPreset(_)))).is_none());
    manager.click(|hit| *hit == Hit::Button(Button::FormatPresetsToggle));
    // The first preset row on show, in painting order.
    let top = |manager: &Viewer| {
        manager.app.hits(manager.id).into_iter().find_map(|region| match region.hit {
            Hit::Button(Button::FormatPreset(index)) => Some((index, region.rect)),
            _ => None,
        })
    };
    let (first, row) = top(&manager).expect("the presets show");
    // The list is taller than the window, so it scrolls, by wheel or bar.
    let bar = manager.find(|hit| *hit == Hit::Button(Button::FormatPresetScroll)).expect("a scroll bar");
    manager.app.scroll(manager.id, row.center(), point(0.0, -3.0));
    manager.render();
    let (scrolled, _) = top(&manager).unwrap();
    assert!(scrolled > first, "{scrolled} > {first}");
    manager.drag(bar.center(), point(bar.center().x, bar.bottom() + 400.0));
    let last = diver_format::PRESETS.len() - 1;
    assert!(manager.find(|hit| *hit == Hit::Button(Button::FormatPreset(last))).is_some());
    // Escape closes the list, not the window.
    manager.press("escape");
    manager.render();
    assert!(manager.is_open() && top(&manager).is_none());
    manager.click(|hit| *hit == Hit::Button(Button::FormatPresetsToggle));
    manager.click(|hit| *hit == Hit::Button(Button::FormatPreset(last)));
    viewer.settle();
    assert!(top(&manager).is_none());
    assert_eq!(viewer.read(|view| view.session.working.as_ref().unwrap().formats.len()), 1);
}

#[test]
fn rules_read_list_items_and_struct_fields_on_both_engines() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("nested.parquet");
    let file = path.display().to_string().replace('\\', "/");
    duckdb::Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES \
             (1, [5, 1, 9], {{'kind': 'gold', 'place name': 'a, b', 'inner': {{'n': 3}}}}), \
             (2, [0], {{'kind': 'it''s', 'place name': NULL, 'inner': {{'n': 1}}}}), \
             (3, NULL, NULL)) t(id, scores, info)) TO '{file}'"
        ))
        .unwrap();
    for engine in crate::infrastructure::store::Engine::ALL {
        let viewer = Viewer::file(&path);
        viewer.update(|view, cx| view.set_file_engine(engine, cx));
        viewer.settle();
        viewer.update(|view, cx| {
            view.ensure_working_view();
            view.session
                .edit_definition(|definition| {
                    definition.formats = vec![
                        crate::domain::documents::ConditionalFormat {
                            rules: "display(c[0] * 1000, '#,##0.0', blank='-'); if c[0] > 1 and c[-1] = 9 then bold"
                                .into(),
                            columns: vec!["scores".into()],
                        },
                        crate::domain::documents::ConditionalFormat {
                            rules: "if c.inner.n + \"scores\"[0] = 8 or c.kind = 'it''s' then bold".into(),
                            columns: vec!["info".into()],
                        },
                        crate::domain::documents::ConditionalFormat {
                            rules: "if \"info\".\"place name\" = 'a, b' then bold".into(),
                            columns: vec!["id".into()],
                        },
                    ]
                })
                .unwrap();
            view.refresh_formatting(cx);
        });
        viewer.render();
        assert_eq!(bold(&viewer, 1), [true, false, false], "{engine:?}");
        assert_eq!(bold(&viewer, 2), [true, true, false], "{engine:?}");
        assert_eq!(bold(&viewer, 0), [true, false, false], "{engine:?}");
        let shown = viewer.read(|view| {
            let result = &view.session.result;
            let rows = result.fetch_rows(0, result.row_count()).unwrap();
            let formatting = view.grid.formatting().unwrap();
            rows.iter()
                .enumerate()
                .map(|(index, row)| formatting.style(result.column_indices[1], row, index).display)
                .collect::<Vec<_>>()
        });
        assert_eq!(shown, [Some("5,000.0".into()), Some("0.0".into()), Some("-".into())], "{engine:?}");
    }
}
