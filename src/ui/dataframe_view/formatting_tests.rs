use super::test_support::Viewer;
use super::dialogs::Dialog;
use crate::application::commands::CommandId;

const SALES: &str = "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n";

fn sales(home: &std::path::Path) -> std::path::PathBuf {
    let path = home.join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    path
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
    viewer.press("m");
    viewer.run_until_parked();
    let opened = viewer.read(|view| match &view.dialog {
        Some(Dialog::Formatting(manager)) => Some(manager.formats.clone()),
        _ => None,
    });
    assert_eq!(opened.unwrap()[0].columns, ["kind"]);
    viewer.input("if duplicate(c) then bold");
    viewer.settle();
    // `a` is on three rows, `b` on one.
    assert_eq!(bold(&viewer, 1), [true, false, true, true]);
    // The first Escape closes the completion list typing opened.
    viewer.press("escape escape");
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.dialog.is_none()));

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
