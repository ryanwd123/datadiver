use super::FileEntry;
use super::dialogs::Dialog;
use super::test_support::{Viewer, clipboard_text, repository, sales_frame};
use crate::application::commands::CommandId;
use crate::domain::documents::{DocumentDefinition, ViewKind};
use crate::domain::formatting::NumericFormat;
use crate::domain::frame::FrameSnapshot;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::hit::{Button, Hit, SlicerHit};
use crate::ui::platform::{Rect, point};
use duckdb::types::Value;
use std::time::Duration;

/// The sales viewer on `source_path`, with settings in `repository`.
fn viewer_with_document(
    repository: ViewRepository,
    source_path: Option<String>,
    document: Option<DocumentDefinition>,
) -> Viewer {
    Viewer::open(sales_frame(), source_path, document, repository)
}

fn viewer_with_repository(repository: ViewRepository) -> Viewer {
    viewer_with_document(repository, None, None)
}

/// Presses `keys` with the grid focused.
fn press_in_grid(viewer: &Viewer, keys: &str) {
    viewer.focus_grid();
    viewer.render();
    viewer.press(keys);
    viewer.run_until_parked();
}

fn press(viewer: &Viewer, keys: &str) {
    viewer.render();
    viewer.press(keys);
    viewer.run_until_parked();
}

fn numeric_format(viewer: &Viewer, column: &str) -> Option<NumericFormat> {
    viewer.read(|view| view.grid.numeric_format(column))
}

fn column_width(viewer: &Viewer, column: &str) -> f32 {
    viewer.read(|view| view.grid.column_width(column))
}

fn sampling_calls(viewer: &Viewer) -> usize {
    viewer.read(|view| view.session.source.store.lock().unwrap().width_sampling_calls())
}

fn first_value(viewer: &Viewer) -> Option<String> {
    viewer.read(|view| {
        view.session
            .result
            .store
            .lock()
            .unwrap()
            .fetch_row_ids(&[0])
            .unwrap()[0]
            .values[0]
            .clone()
    })
}

fn slicer_value(viewer: &Viewer, column: &str, index: usize) -> Rect {
    viewer.render();
    viewer
        .find(|hit| {
            *hit == Hit::Slicer(SlicerHit::Value {
                column: column.into(),
                index,
            })
        })
        .unwrap_or_else(|| panic!("no slicer value {column}:{index}"))
}

const ONE_DECIMAL: NumericFormat = NumericFormat {
    precision: 1,
    grouping: false,
};

#[test]
fn numeric_shortcuts_follow_active_columns_and_restore_from_settings() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let source = home.path().join("source.csv").to_string_lossy().into_owned();
    let viewer = viewer_with_document(repository.clone(), Some(source.clone()), None);
    press_in_grid(&viewer, ", >");
    assert!(repository.load_settings().unwrap().column_formats.is_empty());
    viewer.press("right , shift-. > shift-,");
    assert_eq!(numeric_format(&viewer, "sales"), Some(ONE_DECIMAL));
    viewer.press("alt-left");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(viewer.read(|view| view.session.result.columns[0].0.clone()), "sales");
    assert_eq!(numeric_format(&viewer, "sales"), Some(ONE_DECIMAL));
    assert_eq!(numeric_format(&viewer, "region"), None);
    assert!(!viewer.read(|view| view.session.dirty));
    viewer.focus_search();
    viewer.input(",<>");
    assert_eq!(numeric_format(&viewer, "sales"), Some(ONE_DECIMAL));
    let persisted = repository.load_settings().unwrap();
    assert_eq!(persisted.column_formats[&format!("table:{source}")]["sales"], ONE_DECIMAL);
    super::profiles::with(|profiles| profiles.column_formats.remove(&repository.paths.config));
    let reopened = viewer_with_document(repository, Some(source), None);
    reopened.render();
    assert_eq!(numeric_format(&reopened, "sales"), Some(ONE_DECIMAL));
}

#[test]
fn saved_queries_autosave_formats_without_saving_query_edits() {
    use crate::domain::documents::{Aggregation, QueryConfig};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let source = home.path().join("source.csv").to_string_lossy().into_owned();
    let mut definition = DocumentDefinition::draft_table("Totals".into(), source.clone());
    definition.view = ViewKind::Query(QueryConfig {
        group_by: vec!["region".into()],
        aggregations: vec![Aggregation::sql_expression("sum(sales)".into(), Some("Total".into()))],
        ..Default::default()
    });
    repository.save_view(&mut definition).unwrap();
    let viewer = viewer_with_document(repository.clone(), Some(source.clone()), Some(definition.clone()));
    press_in_grid(&viewer, "right");
    assert_eq!(numeric_format(&viewer, "Total").unwrap().precision, 2);
    viewer.press(", >");
    assert_eq!(numeric_format(&viewer, "Total").unwrap().precision, 3);
    viewer.press("<");
    assert_eq!(numeric_format(&viewer, "Total").unwrap().precision, 2);
    viewer.press("< < <");
    assert_eq!(
        numeric_format(&viewer, "Total"),
        Some(NumericFormat {
            precision: 0,
            grouping: false
        })
    );
    assert!(!viewer.read(|view| view.session.dirty));
    viewer.update(|view, _| view.session.dirty = true);
    viewer.press(">");
    assert!(viewer.read(|view| view.session.dirty));
    let settings = repository.load_settings().unwrap();
    assert_eq!(settings.column_formats[&format!("view:{}", definition.id)]["Total"], ONE_DECIMAL);
    assert_eq!(repository.load_views().0, vec![definition.clone()]);
    let reopened = viewer_with_document(repository, Some(source), Some(definition));
    reopened.render();
    assert_eq!(numeric_format(&reopened, "Total"), Some(ONE_DECIMAL));
}

#[test]
fn query_configuration_changes_refit_existing_columns() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    for (shortcut, field) in [("g", "region"), ("p", "sales"), ("a", "sales")] {
        viewer.focus_grid();
        viewer.render();
        for _ in 0..25 {
            viewer.press(")");
        }
        assert!(column_width(&viewer, "region") > 250.);
        viewer.press(shortcut);
        viewer.render();
        viewer.input(field);
        viewer.press("enter");
        viewer.run_until_parked();
        viewer.render();
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(view.grid.widths_ready());
        });
        assert!(column_width(&viewer, "region") < 250.);
        viewer.close_dialog();
    }
    press_in_grid(&viewer, ")");
    let manual = column_width(&viewer, "region");
    viewer.update(|view, cx| view.schedule_search("North".into(), cx));
    assert_eq!(column_width(&viewer, "region"), manual);
    viewer.run_until_parked();
}

#[test]
fn copy_as_search_is_focused_from_shortcut_and_command() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    for shortcut in [true, false] {
        viewer.focus_grid();
        viewer.render();
        if shortcut {
            viewer.press("ctrl-shift-c");
        } else {
            viewer.update(|view, cx| view.dispatch_command(CommandId::CopyAs, cx));
        }
        viewer.render();
        assert!(viewer.has_dialog());
        viewer.input("json records");
        viewer.run_until_parked();
        press(&viewer, "enter");
        assert!(!viewer.has_dialog());
        let copied: serde_json::Value = serde_json::from_str(&clipboard_text().unwrap()).unwrap();
        assert_eq!(copied[0]["region"], "North");
    }
}

#[test]
fn cached_direct_widths_skip_sampling_and_shortcuts_respect_focus() {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let source = home.path().join("source.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    repository
        .update_table_preferences(
            &source,
            &TablePreferencesPatch {
                reset: false,
                widths: [("region".into(), Some(150.)), ("sales".into(), Some(90.))].into(),
                column_order: Some(vec!["sales".into(), "region".into()]),
                frozen_columns: Some(vec!["sales".into()]),
                slicer_columns: Some(vec!["region".into()]),
                condition_slicers: None,
                hidden_columns: None,
                file: None,
            },
        )
        .unwrap();
    let viewer = viewer_with_document(repository.clone(), Some(source.to_string_lossy().into_owned()), None);
    viewer.render();
    assert_eq!(column_width(&viewer, "region"), 150.);
    assert_eq!(sampling_calls(&viewer), 0);
    viewer.read(|view| {
        assert_eq!(view.session.result.columns[0].0, "sales");
        assert_eq!(view.session.effective_table_config().frozen_count, 1);
        assert!(view.session.working.as_ref().unwrap().slicers[0].selected.is_empty());
    });
    viewer.focus_grid();
    viewer.press("shift-0");
    assert_eq!(column_width(&viewer, "sales"), 100.);
    assert_eq!(column_width(&viewer, "region"), 150.);
    viewer.press("shift-9");
    assert_eq!(column_width(&viewer, "sales"), 90.);
    viewer.press(")");
    assert_eq!(column_width(&viewer, "sales"), 100.);
    viewer.update(|view, cx| view.change_font_size(1, cx));
    viewer.render();
    assert_eq!(column_width(&viewer, "sales"), 100.);
    assert_eq!(column_width(&viewer, "region"), 150.);
    assert_eq!(sampling_calls(&viewer), 0);
    viewer.focus_search();
    viewer.input("()");
    assert_eq!(column_width(&viewer, "sales"), 100.);
    viewer.update(|view, cx| view.open_information(cx));
    viewer.press(")");
    assert_eq!(column_width(&viewer, "sales"), 100.);
    viewer.close_dialog();
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(!view.session.dirty);
        view.table_writer.flush().unwrap();
    });
    assert_eq!(repository.load_table_preferences(&source).unwrap().widths["sales"], 100.);
}

#[test]
fn missing_widths_infer_once_and_manual_widths_survive_pending_inference() {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let source = home.path().join("source.csv");
    repository
        .update_table_preferences(
            &source,
            &TablePreferencesPatch {
                widths: [("sales".into(), Some(175.))].into(),
                ..Default::default()
            },
        )
        .unwrap();
    let viewer = viewer_with_document(repository.clone(), Some(source.to_string_lossy().into_owned()), None);
    // Sampling has returned candidates; a manual change must survive measuring them.
    viewer.focus_grid();
    viewer.press(")");
    let manual = column_width(&viewer, "region");
    viewer.render();
    assert_eq!(column_width(&viewer, "region"), manual);
    assert_eq!(column_width(&viewer, "sales"), 175.);
    assert!(viewer.read(|view| view.grid.widths_ready()));
    assert_eq!(sampling_calls(&viewer), 1);
    viewer.update(|view, cx| view.schedule_search("North".into(), cx));
    viewer.render();
    viewer.run_until_parked();
    assert_eq!(sampling_calls(&viewer), 1);
    viewer.read(|view| view.table_writer.flush().unwrap());
    assert_eq!(repository.load_table_preferences(&source).unwrap().widths["sales"], 175.);
}

#[test]
fn programmatic_updates_submit_once_and_supersede_a_typing_debounce() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    let before = viewer.read(|view| view.session.token());
    viewer.update(|view, cx| view.set_search_text("north".into(), cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(!view.busy);
        assert!(!view.debouncing);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.session.token().request_revision, before.request_revision + 1);
    });
    viewer.render();
    viewer.focus_search();
    viewer.press("ctrl-a");
    viewer.input("south");
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.debouncing));
    viewer.update(|view, cx| view.set_search_text(String::new(), cx));
    viewer.run_until_parked();
    let immediate = viewer.read(|view| {
        assert_eq!(view.session.result.row_count(), 2);
        assert!(!view.busy);
        view.session.token()
    });
    viewer.advance_clock(Duration::from_millis(50));
    viewer.read(|view| {
        assert_eq!(view.session.token(), immediate);
        assert_eq!(view.session.result.row_count(), 2);
    });
}

fn navigate(viewer: &Viewer, definition: DocumentDefinition, carry_search: bool) {
    viewer.update(|view, cx| view.navigate_to_file(FileEntry::View(definition), carry_search, false, false, cx));
    viewer.run_until_parked();
}

#[test]
fn same_source_navigation_shares_rows_but_requeries_changed_sort_and_filter() {
    use crate::domain::documents::{ScalarKey, SlicerDefinition, SortKey};
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("source.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let mut base = DocumentDefinition::draft_table("Base".into(), path.to_string_lossy().into_owned());
    repository.save_view(&mut base).unwrap();
    let viewer = viewer_with_repository(repository);
    navigate(&viewer, base.clone(), false);
    let (source, first) = viewer.read(|view| {
        assert!(!view.busy);
        (view.session.source.clone(), view.session.result.clone())
    });
    let mut reordered = base.clone();
    if let ViewKind::Table(table) = &mut reordered.view {
        table.column_order = vec!["n".into(), "region".into()];
        table.frozen_count = 1;
    }
    navigate(&viewer, reordered, false);
    viewer.read(|view| {
        assert!(Arc::ptr_eq(&source.store, &view.session.source.store));
        assert_eq!(source.source_revision, view.session.source.source_revision);
        let result = &view.session.result;
        assert!(first.row_ids.same(&result.row_ids));
        // An unsorted, unfiltered table reads by rowid without a row map.
        assert!(first.row_map.is_none() && result.row_map.is_none());
        assert!(result.page_is_cached(0));
        assert_eq!(result.columns[0].0, "n");
        assert!(result.result_revision > first.result_revision);
        assert!(result.generation > first.generation);
    });
    let mut sorted = base.clone();
    if let ViewKind::Table(table) = &mut sorted.view {
        table.sort = vec![SortKey {
            column: "n".into(),
            descending: true,
        }];
    }
    navigate(&viewer, sorted, false);
    viewer.read(|view| {
        let result = &view.session.result;
        assert!(!first.row_ids.same(&result.row_ids));
        assert_eq!(result.row_ids.to_vec(), [1, 0]);
    });
    let mut sliced = base;
    sliced.slicers = vec![SlicerDefinition {
        sql: false,
        condition: None,
        column: "region".into(),
        selected: vec![ScalarKey::String("North".into())],
    }];
    navigate(&viewer, sliced, false);
    assert_eq!(viewer.read(|view| view.session.result.row_ids.to_vec()), [0]);
    viewer.read(|view| view.history_writer.flush());
}

#[test]
fn failed_destination_query_restores_the_unsaved_document() {
    use crate::domain::documents::{ScalarKey, SlicerDefinition};
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    viewer.update(|view, cx| {
        view.session.start_table_draft("Draft".into()).unwrap();
        view.session.rename_working("Unsaved".into()).unwrap();
        view.set_search_text("north".into(), cx);
    });
    viewer.run_until_parked();
    let mut definition = DocumentDefinition::draft_table("Invalid".into(), path.to_string_lossy().into_owned());
    definition.slicers = vec![SlicerDefinition {
        sql: false,
        condition: None,
        column: "missing".into(),
        selected: vec![ScalarKey::String("North".into())],
    }];
    navigate(&viewer, definition, false);
    viewer.read(|view| {
        assert!(view.error.as_ref().unwrap().contains("missing"));
        assert_eq!(view.session.working.as_ref().unwrap().name, "Unsaved");
        assert!(view.session.dirty);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.session.search_text, "north");
        assert_eq!(view.search_input.text(), "north");
        assert!(!view.busy);
        assert!(view.navigation_backup.is_none());
    });
}

#[test]
fn rapid_navigation_rejects_old_loads_and_carries_or_clears_search() {
    use crate::domain::documents::SortKey;
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let base = DocumentDefinition::draft_table("Base".into(), path.to_string_lossy().into_owned());
    let mut sorted = base.clone();
    sorted.name = "Latest".into();
    if let ViewKind::Table(table) = &mut sorted.view {
        table.sort = vec![SortKey {
            column: "n".into(),
            descending: true,
        }];
    }
    let viewer = viewer_with_repository(repository(home.path()));
    viewer.update(|view, cx| {
        view.set_search_text("north".into(), cx);
        view.navigate_to_file(FileEntry::View(base.clone()), true, false, false, cx);
        view.navigate_to_file(FileEntry::View(sorted.clone()), true, false, false, cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.working.as_ref().unwrap().name, "Latest");
        assert_eq!(view.session.search_text, "north");
        assert_eq!(view.session.sort[0].column, "n");
        assert_eq!(view.session.result.row_ids.to_vec(), [0]);
        assert!(!view.loading && !view.busy);
    });
    navigate(&viewer, sorted, false);
    viewer.read(|view| {
        assert!(view.session.search_text.is_empty());
        assert_eq!(view.session.result.row_ids.to_vec(), [1, 0]);
        view.history_writer.flush();
    });
}

#[test]
fn two_windows_follow_reloads_and_report_missing_sources() {
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let source = home.path().join("shared.csv");
    std::fs::write(&source, "n\n1\n2\n").unwrap();
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let first_store = database.load(&source, LoadMode::Check, true).unwrap();
    let second_store = database.load(&source, LoadMode::Check, true).unwrap();
    let first = viewer_with_repository(repository.clone());
    let second = Viewer::open_in(&first.app, sales_frame(), None, None, repository);
    for (viewer, store) in [(&first, first_store), (&second, second_store)] {
        viewer.update(|view, cx| {
            view.session.source_path = Some(source.to_string_lossy().into_owned());
            view.replace_dataframe(FrameSnapshot::new(store, 2), cx);
        });
    }
    first.settle();
    std::fs::write(&source, "n\n300\n").unwrap();
    first.update(|view, cx| view.reload(false, cx));
    first.settle();
    assert!(std::sync::Arc::ptr_eq(
        &first.read(|view| view.database.clone()),
        &second.read(|view| view.database.clone())
    ));
    assert_eq!(first.read(|view| view.session.result.row_count()), 1);
    second.advance_clock(Duration::from_secs(1));
    assert_eq!(second.read(|view| view.session.result.row_count()), 1);
    assert_eq!(first_value(&second), Some("300".into()));
    std::fs::remove_file(&source).unwrap();
    second.update(|view, cx| view.reload(false, cx));
    second.run_until_parked();
    second.read(|view| {
        assert!(view.error.as_ref().unwrap().contains("cannot access source"));
        // The rows already read stay on screen.
        assert_eq!(view.session.result.row_count(), 1);
    });
}

#[test]
fn source_monitor_reports_missing_files_and_recovers_when_recreated() {
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let repository = repository(home.path());
    let mut state = crate::application::state::ApplicationState::load(repository.clone()).unwrap();
    state.add_watched_files(std::slice::from_ref(&source)).unwrap();
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let loaded = FrameSnapshot::new(database.load(&source, LoadMode::Check, true).unwrap(), 1);
    let viewer = viewer_with_document(repository, Some(source.to_string_lossy().into_owned()), None);
    viewer.update(|view, cx| view.replace_dataframe(loaded, cx));
    viewer.run_until_parked();
    std::fs::remove_file(&source).unwrap();
    viewer.advance_clock(Duration::from_secs(1));
    assert!(viewer.read(|view| view.error.as_ref().unwrap().contains("Cannot refresh source")));
    std::fs::write(&source, "region,sales\nEast,300\n").unwrap();
    viewer.advance_clock(Duration::from_secs(1));
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 1);
    });
    assert_eq!(first_value(&viewer), Some("East".into()));
}

fn viewer(home: &tempfile::TempDir) -> Viewer {
    viewer_with_repository(repository(home.path()))
}

#[test]
fn slicers_stay_left_of_the_grid_and_use_two_columns_when_the_window_fits() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.resize(1400., 900.);
    let source = DuckDbStore::from_rows(
        &[
            ("region".into(), "VARCHAR".into()),
            ("sales".into(), "BIGINT".into()),
            ("flag".into(), "BOOLEAN".into()),
        ],
        [
            vec![Value::Text("North".into()), Value::BigInt(10), Value::Boolean(true)],
            vec![Value::Text("South".into()), Value::BigInt(20), Value::Boolean(false)],
        ],
    )
    .unwrap();
    viewer.update(|view, cx| {
        view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
        for column in ["region", "sales", "flag"] {
            view.add_slicer(column.into(), cx);
        }
    });
    viewer.settle();
    let region = slicer_value(&viewer, "region", 0);
    let sales = slicer_value(&viewer, "sales", 0);
    let flag = slicer_value(&viewer, "flag", 0);
    assert!(sales.x > region.x);
    assert_eq!(region.y, sales.y);
    assert_eq!(flag.x, region.x);
    assert!(flag.y > region.y);
    assert!(sales.right() <= 480.);
    viewer.click_at(slicer_value(&viewer, "sales", 1).center());
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    viewer.resize(800., 600.);
    viewer.run_until_parked();
    let region = slicer_value(&viewer, "region", 0);
    let sales = slicer_value(&viewer, "sales", 0);
    assert_eq!(region.x, sales.x);
    assert!(sales.y > region.y);
    assert!(region.right() <= 240.);
    viewer.click(|hit| *hit == Hit::Button(Button::SlicerClear("sales".into())));
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 2);
}

#[test]
fn overflowing_slicer_scrollbar_works_without_hover_and_keeps_other_cards_in_place() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.resize(1200., 600.);
    let connection = duckdb::Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE qp_data AS SELECT range AS n, range AS m FROM range(1000)")
        .unwrap();
    let source = DuckDbStore::from_connection(connection).unwrap();
    viewer.update(|view, cx| {
        view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
        for column in ["n", "m"] {
            view.add_slicer(column.into(), cx);
        }
    });
    viewer.settle();
    let first_y = slicer_value(&viewer, "n", 0).y;
    let other_y = slicer_value(&viewer, "m", 0).y;
    let bar = viewer
        .find(|hit| *hit == Hit::Slicer(SlicerHit::Bar("n".into())))
        .expect("the n card scrolls");
    // A press below the thumb, without hovering first, scrolls the values.
    viewer.click_at(point(bar.center().x, (bar.y + 200.).min(bar.bottom() - 4.)));
    viewer.render();
    let moved = viewer
        .find(|hit| {
            *hit == Hit::Slicer(SlicerHit::Value {
                column: "n".into(),
                index: 0,
            })
        })
        .map(|value| value.y);
    assert!(moved.is_none_or(|y| y < first_y));
    assert_eq!(slicer_value(&viewer, "m", 0).y, other_y);
    viewer.read(|view| {
        assert_eq!(view.session.result.row_count(), 1_000);
        assert!(
            view.session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .iter()
                .all(|card| card.selected.is_empty())
        );
    });
}

fn picked_slicers(viewer: &Viewer) -> Vec<crate::domain::documents::SlicerDefinition> {
    viewer.read(|view| view.session.working.as_ref().unwrap().slicers.clone())
}

fn has_button(viewer: &Viewer, button: Button) -> bool {
    viewer.render();
    viewer.find(|hit| *hit == Hit::Button(button.clone())).is_some()
}

fn click_button(viewer: &Viewer, button: Button) {
    viewer.click(|hit| *hit == Hit::Button(button.clone()));
    viewer.settle();
}

const CONTROL: crate::ui::platform::Modifiers = crate::ui::platform::Modifiers {
    control: true,
    alt: false,
    shift: false,
    platform: false,
};

#[test]
fn slicer_picker_filters_by_identity_and_cards_filter_clear_remove() {
    use crate::domain::documents::ScalarKey;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    press(&viewer, "space");
    press(&viewer, "z");
    viewer.input("sales");
    viewer.settle();
    assert!(viewer.has_dialog());
    press(&viewer, "enter");
    viewer.settle();
    assert!(!viewer.has_dialog());
    assert_eq!(picked_slicers(&viewer)[0].column, "sales");
    assert!(!viewer.read(|view| view.session.dirty));
    assert!(repository.load_views().0.is_empty());
    assert!(has_button(&viewer, Button::SlicerRemove("sales".into())));
    viewer.click_at(slicer_value(&viewer, "sales", 0).center());
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    assert_eq!(picked_slicers(&viewer)[0].selected, [ScalarKey::Signed("10".into())]);
    click_button(&viewer, Button::SlicerClear("sales".into()));
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 2);
    viewer.drag(slicer_value(&viewer, "sales", 0).center(), slicer_value(&viewer, "sales", 1).center());
    viewer.settle();
    assert_eq!(picked_slicers(&viewer)[0].selected.len(), 2);
    // Ctrl+click toggles one value.
    let first = slicer_value(&viewer, "sales", 0).center();
    viewer.drag_with(first, first, CONTROL);
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    assert_eq!(picked_slicers(&viewer)[0].selected, [ScalarKey::Signed("20".into())]);
    // Ctrl+drag adds a range.
    viewer.drag_with(
        slicer_value(&viewer, "sales", 1).center(),
        slicer_value(&viewer, "sales", 0).center(),
        CONTROL,
    );
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 2);
    assert_eq!(picked_slicers(&viewer)[0].selected.len(), 2);
    viewer.press("tab");
    assert!(viewer.search_focused());
    click_button(&viewer, Button::SlicerRemove("sales".into()));
    assert!(!has_button(&viewer, Button::SlicerRemove("sales".into())));
    assert!(picked_slicers(&viewer).is_empty());
}

#[test]
fn slicer_availability_ignores_search_and_picker_excludes_existing_cards() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| {
        view.add_slicer("region".into(), cx);
        view.add_slicer("sales".into(), cx);
    });
    viewer.settle();
    viewer.click_at(slicer_value(&viewer, "region", 0).center());
    viewer.settle();
    let catalog = viewer.read(|view| view.slicer_stage.as_ref().unwrap().catalog.clone());
    viewer.update(|view, cx| view.set_search_text("South".into(), cx));
    viewer.settle();
    viewer.read(|view| {
        let stage = view.slicer_stage.as_ref().unwrap();
        assert!(std::sync::Arc::ptr_eq(&stage.catalog, &catalog));
        assert!(stage.cards[0].values.iter().all(|value| value.available));
        assert!(stage.cards[1].values[0].available);
        assert!(!stage.cards[1].values[1].available);
        assert_eq!(view.session.result.row_count(), 0);
    });
    viewer.update(|view, cx| view.open_slicer_picker(cx));
    viewer.render();
    viewer.input("region");
    viewer.press("enter");
    assert!(viewer.has_dialog());
    assert_eq!(picked_slicers(&viewer).len(), 2);
    viewer.press("escape");
    assert!(viewer.has_dialog());
    viewer.press("escape");
    assert!(!viewer.has_dialog());
}

#[test]
fn slicer_picker_offers_cards_without_values_before_discovery_finishes() {
    use crate::ui::slicer_sidebar::SlicerKind;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let kinds = || {
        viewer.read(|view| {
            let Some(Dialog::SlicerPicker(picker)) = &view.dialog else {
                panic!("the slicer picker is open")
            };
            let kinds = picker.choices().iter().map(|choice| choice.kind).collect::<Vec<_>>();
            (picker.loading, kinds)
        })
    };
    viewer.app.update(viewer.id, |view, cx| view.open_slicer_picker(cx));
    // Discovery has not finished yet, but SQL and condition cards are listed.
    let (loading, before) = kinds();
    assert!(loading);
    assert_eq!(before[0], SlicerKind::Sql);
    assert!(before.len() > 1);
    assert!(before[1..].iter().all(|kind| *kind == SlicerKind::Condition));
    viewer.settle();
    let (loading, after) = kinds();
    assert!(!loading);
    assert!(after.contains(&SlicerKind::Values));
    // Closing the picker stops discovering every column.
    viewer.press("escape");
    assert!(!viewer.has_dialog());
    assert!(viewer.read(|view| view.slicer_discovery.is_none()));
}

#[test]
fn slicer_reload_errors_keep_last_result_and_missing_selected_values_remain_removable() {
    use crate::domain::documents::ScalarKey;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| {
        view.add_slicer("region".into(), cx);
        let mut definition = view.session.working.clone().unwrap();
        definition.id = "named-table".into();
        view.session.saved(definition);
    });
    viewer.settle();
    viewer.click_at(slicer_value(&viewer, "region", 0).center());
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    let south = DuckDbStore::from_rows(&[("region".into(), "VARCHAR".into())], [vec![Value::Text("South".into())]]).unwrap();
    viewer.update(|view, cx| view.replace_dataframe(FrameSnapshot::new(south, 2), cx));
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 0);
    assert_eq!(picked_slicers(&viewer)[0].selected, [ScalarKey::String("North".into())]);
    viewer.click_at(slicer_value(&viewer, "region", 1).center());
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    let synthetic = FrameSnapshot::new(DuckDbStore::synthetic(100).unwrap(), 3);
    viewer.update(|view, cx| view.replace_dataframe(synthetic, cx));
    viewer.settle();
    viewer.read(|view| {
        assert_eq!(view.session.result.row_count(), 1);
        assert!(view.error.as_ref().unwrap().contains("missing"));
        assert!(view.slicer_stage.as_ref().unwrap().cards[0].error.is_some());
    });
    click_button(&viewer, Button::SlicerRemove("region".into()));
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 100);
}

fn theme_form(viewer: &Viewer) -> Result<crate::application::theme::ThemeSettings, String> {
    viewer.read(|view| {
        let Some(Dialog::Theme(form)) = &view.dialog else {
            panic!("the theme editor is open")
        };
        form.build()
    })
}

#[test]
fn theme_editor_command_saves_colors_across_windows_and_restarts() {
    use crate::application::theme::{ThemeColor, ThemeSettings};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    let other = Viewer::open_in(&viewer.app, sales_frame(), None, None, repository.clone());
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("editor theme");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert!(has_button(&viewer, Button::DialogOk));
    viewer.press("ctrl-a");
    viewer.input("#AABBCC");
    assert_eq!(viewer.read(|view| view.theme.clone()), ThemeSettings::default());
    assert!(!repository.paths.config.join("theme.json").exists());
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    assert!(!viewer.has_dialog());
    viewer.run_until_parked();
    let saved = repository.load_theme().unwrap();
    assert_eq!(saved.header_background, ThemeColor::rgb(0xaabbcc));
    other.render();
    assert_eq!(viewer.read(|view| view.theme.clone()), saved);
    assert_eq!(other.read(|view| view.theme.clone()), saved);
    assert!(!viewer.read(|view| view.session.dirty));
    // Drop the shared settings so a new viewer must load the file again.
    super::profiles::with(|profiles| profiles.themes.clear());
    let restarted = viewer_with_repository(repository);
    assert_eq!(restarted.read(|view| view.theme.clone()), saved);
}

#[test]
fn theme_editor_cancel_validation_reset_and_failed_save_keep_the_draft() {
    use crate::application::theme::{ThemeColor, ThemeSettings};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let initial = ThemeSettings {
        header_background: ThemeColor::rgb(0xaabbcc),
        ..Default::default()
    };
    repository.save_theme(&initial).unwrap();
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| view.open_theme_editor(cx));
    viewer.render();
    viewer.press("ctrl-a");
    viewer.input("#bad");
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    assert!(viewer.has_dialog());
    assert_eq!(repository.load_theme().unwrap(), initial);
    assert_eq!(viewer.read(|view| view.theme.clone()), initial);
    viewer.click(|hit| *hit == Hit::Button(Button::ThemeReset));
    viewer.press("escape");
    assert!(!viewer.has_dialog());
    assert_eq!(repository.load_theme().unwrap(), initial);
    assert_eq!(viewer.read(|view| view.theme.clone()), initial);
    viewer.run_until_parked();

    viewer.update(|view, cx| view.open_theme_editor(cx));
    viewer.click(|hit| *hit == Hit::Button(Button::ThemeReset));
    assert_eq!(theme_form(&viewer).unwrap(), ThemeSettings::default());
    // A file in place of the config directory forces a write failure.
    let blocked = home.path().join("blocked");
    std::fs::write(&blocked, "blocked").unwrap();
    viewer.update(|view, _| view.repository.paths.config = blocked);
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    assert!(viewer.has_dialog());
    assert_eq!(theme_form(&viewer).unwrap(), ThemeSettings::default());
    assert_eq!(viewer.read(|view| view.theme.clone()), initial);
    assert_eq!(repository.load_theme().unwrap(), initial);
    viewer.update(|view, _| view.repository = repository.clone());
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    assert!(!viewer.has_dialog());
    assert_eq!(repository.load_theme().unwrap(), ThemeSettings::default());
}

#[test]
fn theme_editor_saves_with_enter_and_preserves_typed_alpha() {
    use crate::application::theme::ThemeColor;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| view.open_theme_editor(cx));
    viewer.render();
    viewer.press("ctrl-a");
    viewer.input("#12345680");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert!(!viewer.has_dialog());
    assert_eq!(
        repository.load_theme().unwrap().header_background,
        ThemeColor::parse("#12345680").unwrap()
    );
}

#[test]
fn palettes_focus_their_query() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("info");
    viewer.press("escape");
    assert!(viewer.has_dialog(), "Escape should clear the query first");
    viewer.close_dialog();

    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("report");
    viewer.press("escape");
    assert!(viewer.has_dialog(), "Escape should clear the query first");
}

#[test]
fn font_picker_and_shortcuts_persist_share_and_clamp() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.font_family = Some("Existing font".into());
    repository.save_settings(&settings).unwrap();
    let viewer = viewer_with_repository(repository.clone());
    let other = Viewer::open_in(&viewer.app, sales_frame(), None, None, repository.clone());
    let size = || viewer.read(|view| view.font.size);
    press_in_grid(&viewer, "ctrl-=");
    viewer.render();
    assert_eq!(size(), 14);
    assert_eq!(viewer.read(|view| view.fonts.ui.size), 14.0);
    viewer.press("ctrl--");
    assert_eq!(size(), 13);
    viewer.focus_search();
    viewer.press("ctrl-=");
    assert_eq!(size(), 14);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("picker font");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert!(viewer.has_dialog());
    viewer.input("default");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert!(!viewer.has_dialog());
    assert!(viewer.read(|view| view.font.family.is_none()));
    viewer.update(|view, cx| {
        for _ in 0..50 {
            view.dispatch_command(CommandId::IncreaseFontSize, cx);
        }
    });
    assert_eq!(size(), 40);
    viewer.render();
    let cell = viewer
        .find(|hit| *hit == Hit::Grid(crate::ui::hit::GridHit::Cell { row: 0, column: 0 }))
        .unwrap();
    assert!(cell.h > 40.);
    viewer.update(|view, cx| {
        for _ in 0..50 {
            view.dispatch_command(CommandId::DecreaseFontSize, cx);
        }
    });
    assert_eq!(size(), 7);
    viewer.run_until_parked();
    let saved = repository.load_settings().unwrap();
    assert_eq!(saved.font_size, 7);
    assert!(saved.font_family.is_none());
    other.render();
    assert_eq!(viewer.read(|view| view.font.clone()), other.read(|view| view.font.clone()));
    assert!(!viewer.read(|view| view.session.dirty));
    super::profiles::with(|profiles| profiles.fonts.clear());
    let restarted = viewer_with_repository(repository);
    assert_eq!(restarted.read(|view| view.font.size), 7);
}

#[test]
fn editor_font_follows_its_picker_and_the_grid_size() {
    use crate::ui::font_picker::FontTarget;
    use crate::ui::widgets::DEFAULT_EDITOR_FAMILY;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| {
        let mut font = view.font.clone();
        font.editor_family = Some("Test Mono".into());
        font.size = 17;
        view.save_font(font, cx);
    });
    viewer.render();
    // Every code editor reads the monospace font.
    viewer.read(|view| {
        assert_eq!(view.fonts.mono.family.as_ref(), "Test Mono");
        assert_eq!(view.fonts.mono.size, 17.);
        assert!(view.font.family.is_none());
    });
    viewer.update(|view, cx| view.open_font_picker(FontTarget::Editor, cx));
    viewer.render();
    assert!(viewer.has_dialog());
    viewer.input("default");
    viewer.run_until_parked();
    press(&viewer, "enter");
    viewer.read(|view| {
        assert!(view.font.editor_family.is_none());
        assert_eq!(view.fonts.mono.family.as_ref(), DEFAULT_EDITOR_FAMILY);
    });
    let saved = repository.load_settings().unwrap();
    assert_eq!((saved.font_size, saved.editor_font_family.clone()), (17, None));

    let mut settings = saved;
    settings.editor_font_family = Some("Saved Mono".into());
    repository.save_settings(&settings).unwrap();
    super::profiles::with(|profiles| profiles.fonts.clear());
    let restarted = viewer_with_repository(repository);
    restarted.read(|view| {
        assert_eq!(view.font.editor_family.as_deref(), Some("Saved Mono"));
        assert_eq!(view.fonts.mono.family.as_ref(), "Saved Mono");
    });
}

#[test]
fn clear_filters_button_clears_search_and_all_cards_preserving_sort() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    assert!(!has_button(&viewer, Button::ClearFilters));
    viewer.update(|view, cx| {
        view.add_slicer("region".into(), cx);
        view.add_slicer("sales".into(), cx);
        view.session.set_sort_direction("sales".into(), true);
        view.set_search_text("north".into(), cx);
    });
    viewer.settle();
    viewer.click_at(slicer_value(&viewer, "region", 0).center());
    viewer.click_at(slicer_value(&viewer, "sales", 0).center());
    viewer.settle();
    assert!(picked_slicers(&viewer).iter().all(|card| !card.selected.is_empty()));
    click_button(&viewer, Button::ClearFilters);
    assert!(!has_button(&viewer, Button::ClearFilters));
    viewer.read(|view| {
        let session = &view.session;
        assert_eq!(session.result.row_count(), 2);
        assert!(session.search_text.is_empty());
        assert_eq!(session.sort[0].column, "sales");
        assert_eq!(session.working.as_ref().unwrap().slicers.len(), 2);
        assert!(
            session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .iter()
                .all(|card| card.selected.is_empty())
        );
        assert!(!session.dirty);
        assert_eq!(session.result.row_ids.to_vec(), [1, 0]);
    });
}

/// Labels of the open palette's visible rows, in `section` when given.
fn palette_rows(viewer: &Viewer, section: Option<usize>) -> Vec<String> {
    viewer.read(|view| {
        let Some(Dialog::Palette(dialog)) = &view.dialog else {
            panic!("a palette is open")
        };
        let list = &dialog.palette.list;
        list.visible_items()
            .iter()
            .map(|&index| &list.items()[index])
            .filter(|item| section.is_none_or(|section| item.section == section))
            .map(|item| item.label.clone())
            .collect()
    })
}

#[test]
fn file_picker_matches_terms_across_name_and_path_and_archives_selected_identity() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut first = DocumentDefinition::draft_table("First report".into(), "east.csv".into());
    let mut second = DocumentDefinition::draft_table("Second report".into(), "west.csv".into());
    repository.save_view(&mut first).unwrap();
    repository.save_view(&mut second).unwrap();
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("  WEST\tSECOND  ");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(palette_rows(&viewer, None).len(), 1);
    press(&viewer, "ctrl-delete");
    assert_eq!(repository.load_views().0.len(), 1);
    assert_eq!(repository.load_views().0[0].id, first.id);
    assert_eq!(repository.load_archives().0[0].1.view.id, second.id);
}

#[test]
fn file_and_font_pickers_match_command_palette_width() {
    use crate::ui::font_picker::FontTarget;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let dialog = || {
        viewer.render();
        viewer.find(|hit| *hit == Hit::Surface).expect("a dialog")
    };
    for font in [false, true] {
        viewer.resize(1400., 900.);
        viewer.update(|view, cx| {
            if font {
                view.open_font_picker(FontTarget::Grid, cx);
            } else {
                view.open_file_palette(cx);
            }
        });
        assert_eq!(dialog().w, 896.);
        viewer.resize(700., 600.);
        let bounds = dialog();
        assert!(bounds.w < 700.);
        assert!(bounds.x >= 0.);
        assert!(bounds.right() <= 700.);
        viewer.close_dialog();
    }
}

#[test]
fn file_picker_empty_query_escape_and_filtered_source_confirmation() {
    use crate::application::state::ApplicationState;
    use crate::ui::widgets::list::Part;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("West report.csv");
    std::fs::write(&path, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let other_path = home.path().join("East report.csv");
    std::fs::write(&other_path, "region,sales\nNorth,30\n").unwrap();
    let mut app = ApplicationState::load(repository.clone()).unwrap();
    app.record_source(&path).unwrap();
    app.record_source(&other_path).unwrap();
    let viewer = viewer_with_repository(repository);
    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("west nonexistent");
    viewer.run_until_parked();
    viewer.render();
    assert!(palette_rows(&viewer, Some(2)).is_empty());
    viewer.press("enter");
    assert!(viewer.has_dialog());
    press(&viewer, "escape");
    assert!(palette_rows(&viewer, Some(2)).len() >= 2);
    viewer.input("SOURCE west REPORT");
    viewer.run_until_parked();
    viewer.render();
    let rows = palette_rows(&viewer, None);
    assert_eq!(palette_rows(&viewer, Some(2)).len(), 1, "{rows:?}");
    let position = rows.len() - 1;
    let list = viewer.read(|view| match &view.dialog {
        Some(Dialog::Palette(dialog)) => dialog.palette.list.id,
        _ => panic!("a palette is open"),
    });
    viewer.click(|hit| *hit == Hit::List(list, Part::Row(position)));
    viewer.run_until_parked();
    assert!(!viewer.has_dialog());
    // A recent file opens as a table, read by the default engine.
    viewer.read(|view| {
        assert!(view.session.source_path.as_deref().unwrap().ends_with("West report.csv"));
        assert_eq!(view.session.result.row_count(), 2);
        assert!(matches!(
            view.session.identity,
            crate::application::session::SessionIdentity::DraftTable
        ));
        let engine = view.session.source.store.lock().unwrap().engine();
        assert_eq!(engine, crate::infrastructure::store::Engine::Polars);
    });
}

#[test]
fn save_dialog_focuses_name_for_shortcut_and_palette() {
    for from_palette in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let repository = repository(home.path());
        let source = home.path().join("source.csv");
        std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
        let viewer = viewer_with_document(repository.clone(), Some(source.to_string_lossy().into_owned()), None);
        if from_palette {
            viewer.focus_grid();
            press(&viewer, "ctrl-shift-p");
            viewer.input("save view");
            viewer.run_until_parked();
            press(&viewer, "enter");
        } else {
            viewer.focus_search();
            press(&viewer, "ctrl-s");
        }
        viewer.render();
        assert!(viewer.has_dialog());
        viewer.press("ctrl-a");
        viewer.input("Keyboard saved view");
        press(&viewer, "enter");
        assert!(
            !viewer.has_dialog(),
            "from_palette={from_palette}, saved={:?}, error={:?}",
            repository.load_views().0,
            viewer.read(|view| view.error.clone())
        );
        viewer.read(|view| {
            assert!(view.session.search_text.is_empty());
            assert!(!view.session.dirty);
        });
        let (saved, issues) = repository.load_views();
        assert!(issues.is_empty());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Keyboard saved view");
    }
}

#[test]
fn settings_directory_command_reports_an_unusable_active_profile() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    let blocked = home.path().join("blocked");
    std::fs::write(&blocked, "blocked").unwrap();
    viewer.update(|view, _| view.repository.paths.config = blocked.clone());
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("settings directory");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(palette_rows(&viewer, None).len(), 1);
    press(&viewer, "enter");
    assert!(!viewer.has_dialog());
    let error = viewer.read(|view| view.error.clone()).unwrap();
    assert!(error.starts_with("Cannot open settings directory"));
    assert!(error.contains(&blocked.display().to_string()));
    assert_eq!(std::fs::read_to_string(&blocked).unwrap(), "blocked");
}

#[test]
fn command_palette_add_folder_opens_directory_picker_and_saves_selection() {
    use crate::ui::platform::system::fake;
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("add folder");
    viewer.run_until_parked();
    fake::with(|system| system.path_answers.push_back(Some(vec![folder.path().to_path_buf()])));
    press(&viewer, "enter");
    assert!(!viewer.has_dialog());
    assert_eq!(fake::with(|system| system.path_prompts), 1);
    viewer.run_until_parked();
    assert_eq!(
        repository.load_settings().unwrap().watched_folders,
        [folder.path().canonicalize().unwrap().to_string_lossy().to_string()]
    );
}

#[test]
fn command_palette_filters_all_terms_and_confirms_the_filtered_command() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("SHIFT keymap");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(palette_rows(&viewer, None).len(), 1);
    press(&viewer, "enter");
    assert!(viewer.read(|view| view.keymap_editor.is_some()));
    assert!(!viewer.has_dialog());
}

#[test]
fn command_palette_empty_matches_and_escape_restore_rows() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("keymap nonexistent");
    viewer.run_until_parked();
    viewer.render();
    assert!(palette_rows(&viewer, None).is_empty());
    viewer.press("enter");
    assert!(viewer.has_dialog());
    assert!(viewer.read(|view| view.keymap_editor.is_none()));
    press(&viewer, "escape");
    assert!(palette_rows(&viewer, None).len() > 1);
    viewer.press("escape");
    assert!(!viewer.has_dialog());
}

#[test]
fn command_palette_doubles_width_and_fits_smaller_windows() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let dialog = || {
        viewer.render();
        viewer.find(|hit| *hit == Hit::Surface).expect("a dialog")
    };
    viewer.resize(1400., 900.);
    viewer.update(|view, cx| view.open_command_palette(cx));
    assert_eq!(dialog().w, 896.);
    viewer.resize(700., 600.);
    let bounds = dialog();
    assert!(bounds.w < 700.);
    assert!(bounds.x >= 0.);
    assert!(bounds.right() <= 700.);
}

#[test]
fn search_down_and_tab_focus_grid() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.render();
    assert!(viewer.grid_focused());
    viewer.press("/");
    assert!(viewer.search_focused());
    viewer.press("down");
    assert!(viewer.grid_focused());
    viewer.focus_search();
    viewer.press("tab");
    assert!(viewer.grid_focused());
}

#[test]
fn escape_clears_active_search_without_leaving_search() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.render();
    viewer.update(|view, cx| view.set_search_text("North".into(), cx));
    viewer.focus_search();
    viewer.press("escape");
    viewer.read(|view| {
        assert!(view.search_input.text().is_empty());
        assert!(view.search_text().is_empty());
    });
    assert!(viewer.search_focused());
}

#[test]
fn typing_a_quote_in_search_offers_columns_and_tab_or_enter_inserts_them() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let step = |action: &dyn Fn()| {
        viewer.render();
        action();
        viewer.run_until_parked();
        viewer.read(|view| (view.search_input.text().to_owned(), view.search_completion.items()))
    };
    step(&|| viewer.focus_search());
    let (_, items) = step(&|| viewer.input("\""));
    assert_eq!(items, Some(vec!["region".into(), "sales".into()]));
    let (_, items) = step(&|| viewer.input("S"));
    assert_eq!(items, Some(vec!["sales".into()]));
    let (text, items) = step(&|| viewer.press("tab"));
    assert_eq!(text, "\"sales\"");
    assert!(items.is_none());
    assert!(viewer.search_focused());
    // After an operator a quote names the column to compare against.
    step(&|| viewer.input(">\""));
    step(&|| viewer.press("down"));
    let (text, _) = step(&|| viewer.press("enter"));
    assert_eq!(text, "\"sales\">\"sales\"");
    // Escape closes the list first and keeps the text.
    let (_, items) = step(&|| viewer.input(" \""));
    assert!(items.is_some());
    let (text, items) = step(&|| viewer.press("escape"));
    assert!(items.is_none());
    assert!(text.starts_with("\"sales\">\"sales\" \""), "{text}");
}

#[test]
fn right_clicking_a_sorted_header_clears_only_that_sort() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, _| {
        view.session.set_sort_direction("region".into(), false);
        view.session.set_sort_direction("sales".into(), true);
        let sort = view.session.sort.clone();
        view.grid.set_sort(sort);
    });
    viewer.right_click(|hit| *hit == Hit::Grid(crate::ui::hit::GridHit::Header(0)));
    viewer.run_until_parked();
    assert_eq!(
        viewer.read(|view| view.session.sort.iter().map(|key| key.column.clone()).collect::<Vec<_>>()),
        ["sales"]
    );
}

#[test]
fn ctrl_q_closes_the_focused_window() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    press(&viewer, "ctrl-q");
    assert!(!viewer.is_open());
}

#[test]
fn new_window_shortcut_opens_blank_windows_from_grid_and_search() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    for search_focused in [false, true] {
        viewer.update(|view, cx| {
            view.session.set_sort_direction("sales".into(), true);
            view.set_search_text("north".into(), cx);
            view.session.dirty = true;
        });
        if search_focused {
            viewer.focus_search();
        } else {
            viewer.focus_grid();
        }
        press(&viewer, "ctrl-n");
    }
    let blanks = viewer.others();
    assert_eq!(blanks.len(), 2);
    let store = viewer.read(|view| view.session.source.store.clone());
    for blank in blanks {
        blank.read(|blank| {
            assert_eq!(blank.repository.paths.config, repository.paths.config);
            assert_eq!(blank.repository.paths.data, repository.paths.data);
            assert!(blank.session.source_path.is_none());
            assert!(blank.session.working.is_none());
            assert!(!blank.session.dirty);
            assert!(blank.session.search_text.is_empty());
            assert!(blank.session.sort.is_empty());
            assert_eq!(blank.session.source.row_count(), 0);
            assert_eq!(blank.session.result.row_count(), 0);
            assert!(!std::sync::Arc::ptr_eq(&blank.session.source.store, &store));
            assert!(blank.window_geometry.is_none());
        });
        assert_eq!(blank.app.update(blank.id, |_, cx| cx.viewport()), (1400., 900.));
        assert!(blank.grid_focused());
    }
    viewer.read(|view| {
        assert_eq!(view.session.source.row_count(), 2);
        assert_eq!(view.session.search_text, "north");
        assert!(view.session.dirty);
    });
}

#[test]
fn new_window_palette_command_opens_a_blank_window() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("new window");
    viewer.run_until_parked();
    viewer.render();
    let matches = viewer.read(|view| {
        crate::ui::command_palette::matching_commands(&super::active_commands(), &view.keymap, "new window")
    });
    // Drill down in new window matches too, after New window.
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].0.id, CommandId::NewWindow);
    assert_eq!(matches[0].1, "New window");
    assert_eq!(matches[0].2, "Ctrl+N");
    press(&viewer, "enter");
    assert!(!viewer.has_dialog());
    let others = viewer.others();
    assert_eq!(others.len(), 1);
    assert_eq!(others[0].read(|view| view.session.source.row_count()), 0);
}

#[test]
fn new_window_respects_disabled_and_remapped_shortcuts_and_dialogs() {
    use crate::application::keymap::{ActionId, DisabledDefault, KeyContext, UserBinding};
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let windows = || viewer.app.windows().len();
    viewer.update(|view, _| {
        view.keymap.disabled_defaults.push(DisabledDefault {
            action: ActionId::NewWindow,
            context: KeyContext::Application,
            keystroke: "ctrl-n".into(),
        });
        view.keymap.bindings.push(UserBinding {
            action: ActionId::NewWindow,
            context: KeyContext::Application,
            keystroke: "ctrl-shift-n".into(),
        });
    });
    press(&viewer, "ctrl-n");
    assert_eq!(windows(), 1);
    viewer.update(|view, cx| view.open_information(cx));
    press(&viewer, "ctrl-shift-n");
    assert_eq!(windows(), 1);
    viewer.close_dialog();
    viewer.update(|view, cx| view.open_keymap_editor(cx));
    press(&viewer, "ctrl-shift-n");
    assert_eq!(windows(), 1);
    viewer.update(|view, _| view.keymap_editor = None);
    press_in_grid(&viewer, "ctrl-shift-n");
    assert_eq!(windows(), 2);
}

#[test]
fn new_windows_navigate_independently_and_survive_closing_original() {
    use crate::infrastructure::source_database::LoadMode;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("other.csv");
    std::fs::write(&source, "name,amount\nFirst,30\nSecond,40\nThird,50\n").unwrap();
    let viewer = viewer_with_repository(repository(home.path()));
    viewer.update(|view, cx| view.dispatch_command(CommandId::NewWindow, cx));
    viewer.run_until_parked();
    let other = viewer.others().remove(0);
    other.update(|view, cx| view.load_source(source.clone(), false, LoadMode::Check, cx));
    other.settle();
    other.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.source.row_count(), 3);
    });
    viewer.read(|view| {
        assert_eq!(view.session.source.row_count(), 2);
        assert!(view.session.source_path.is_none());
    });
    press(&viewer, "ctrl-q");
    assert_eq!(other.app.windows(), vec![other.id]);
    press(&other, "ctrl-n");
    assert_eq!(other.app.windows().len(), 2);
    press(&other, "ctrl-q");
    let survivors = other.app.windows();
    assert_eq!(survivors.len(), 1);
    assert_eq!(other.window(survivors[0]).read(|view| view.session.source.row_count()), 0);
}

#[test]
fn new_window_closing_saves_preferences_without_changing_primary_placement() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    viewer.update(|view, cx| view.remember_window(cx));
    viewer.run_until_parked();
    let placement = repository.load_history().unwrap().primary_window;
    assert!(placement.is_some());
    for close_method in ["native", "shortcut", "release"] {
        viewer.update(|view, cx| view.open_blank_window(None, cx));
        viewer.run_until_parked();
        let other = viewer.others().remove(0);
        let source = home.path().join(format!("{close_method}.csv"));
        other.update(|view, _| {
            view.session.source_path = Some(source.to_string_lossy().into_owned());
            view.session.direct_table.source_path = source.to_string_lossy().into_owned();
            view.session.set_column_width("Row".into(), 180.);
        });
        match close_method {
            "native" => assert!(other.request_close()),
            // Painting would measure the column and replace the width set above.
            "shortcut" => other.press("ctrl-q"),
            _ => other.app.close(other.id),
        }
        viewer.run_until_parked();
        viewer.read(|view| view.table_writer.flush().unwrap());
        assert_eq!(repository.load_table_preferences(&source).unwrap().widths["Row"], 180.);
        assert_eq!(repository.load_history().unwrap().primary_window, placement);
        assert_eq!(viewer.app.windows(), vec![viewer.id]);
    }
}

#[test]
fn closing_window_flushes_latest_size_without_waiting_for_debounce() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer(&home);
    viewer.update(|view, cx| {
        view.repository = repository.clone();
        view.remember_window(cx);
    });
    viewer.resize(1100., 700.);
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.window_geometry.as_ref().unwrap().width), 1100.0);
    press(&viewer, "ctrl-q");
    viewer.run_until_parked();
    let saved = repository.load_history().unwrap().primary_window.unwrap();
    assert_eq!(saved.width, 1100.0);
    assert_eq!(saved.height, 700.0);
}

#[test]
fn startup_restores_saved_position_and_size() {
    use super::ViewerOptions;
    use crate::infrastructure::json_store::WindowGeometry;
    use crate::ui::platform::headless::TestApp;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let saved = WindowGeometry {
        x: 120.0,
        y: 60.0,
        width: 950.0,
        height: 650.0,
        maximized: false,
    };
    repository.save_window_geometry(&saved).unwrap();
    let app = TestApp::<super::DataFrameView>::new();
    let frame = FrameSnapshot::new(DuckDbStore::synthetic(10).unwrap(), 1);
    super::DataFrameView::open(
        app.runtime(),
        frame,
        None,
        ViewerOptions {
            repository,
            ..Default::default()
        },
    );
    app.run_until_parked();
    let id = app.windows()[0];
    let placement = app.update(id, |_, cx| cx.placement()).unwrap();
    assert_eq!(crate::ui::window_placement::geometry(placement), saved);
}

#[test]
fn info_shortcut_opens_dialog() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    press(&viewer, "i");
    viewer.render();
    assert!(viewer.has_dialog());
}

fn information<R>(viewer: &Viewer, f: impl FnOnce(&crate::ui::info::InformationPanel) -> R) -> R {
    viewer.read(|view| {
        let Some(Dialog::Information(panel)) = &view.dialog else {
            panic!("the information dialog is open")
        };
        f(panel)
    })
}

#[test]
fn info_profiles_every_source_row_while_search_narrows_the_grid() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.set_search_text("North".into(), cx));
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    viewer.update(|view, cx| view.open_information(cx));
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(
        information(&viewer, |panel| panel.row_text("region")).unwrap(),
        [
            "region",
            "VARCHAR",
            "0",
            "0.0%",
            "2",
            "North",
            "South",
            "North (1) · South (1)"
        ]
    );
    assert_eq!(
        information(&viewer, |panel| panel.row_text("sales")).unwrap()[2..7],
        ["0", "0.0%", "2", "10", "20"]
    );
}

fn help<R>(viewer: &Viewer, f: impl FnOnce(&crate::ui::help::HelpDialog) -> R) -> R {
    viewer.read(|view| {
        let Some(Dialog::Help(help)) = &view.dialog else {
            panic!("help is open")
        };
        f(help)
    })
}

fn find(viewer: &Viewer, hit: Hit) -> Rect {
    viewer.render();
    viewer.find(|found| *found == hit).unwrap_or_else(|| panic!("no {hit:?}"))
}

#[test]
fn help_font_changes_and_repeated_scrollbar_drags_keep_track_fixed() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.resize(800., 600.);
    viewer.update(|view, cx| view.open_help(cx));
    viewer.render();
    let height = help(&viewer, |help| help.content_height());
    viewer.update(|view, cx| view.change_font_size(9, cx));
    viewer.render();
    assert!(help(&viewer, |help| help.content_height()) > height);

    let track = find(&viewer, Hit::DialogScroll);
    let bar = find(&viewer, Hit::DialogBar);
    let mut from = point(bar.center().x, bar.y + 12.);
    for travel in [60., 60., -90.] {
        let before = help(&viewer, |help| help.scroll.offset);
        let to = point(from.x, from.y + travel);
        viewer.drag(from, to);
        let after = help(&viewer, |help| help.scroll.offset);
        assert!(
            if travel > 0. { after > before } else { after < before },
            "drag should follow the thumb"
        );
        assert_eq!(find(&viewer, Hit::DialogScroll), track);
        from = to;
    }
}

#[test]
fn help_columns_resize_align_scroll_and_dismiss() {
    use crate::ui::help::HelpLayout;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.resize(1920., 1080.);
    press(&viewer, "f1");
    viewer.render();
    assert!(viewer.has_dialog());

    for (width, height, columns) in [(1920., 1080., 3), (1200., 800., 2), (800., 600., 1)] {
        viewer.resize(width, height);
        let dialog = find(&viewer, Hit::Surface);
        assert_eq!(dialog.w, (width - 48.).min(1500.));
        assert!(dialog.x >= 24.);
        assert!(dialog.right() <= width - 24.);
        let close = find(&viewer, Hit::Button(Button::DialogClose));
        assert!(close.right() >= dialog.right() - 40.);
        assert!(close.right() < dialog.right());
        let body = find(&viewer, Hit::DialogScroll);
        assert!(body.h <= height - 140.);
        assert_eq!(HelpLayout::for_viewport(width).column_count, columns);
        if columns == 1 {
            let before = help(&viewer, |help| help.scroll.offset);
            viewer.app.scroll(viewer.id, body.center(), point(0., -10.));
            viewer.render();
            assert!(help(&viewer, |help| help.scroll.offset) > before);
        }
    }

    viewer.press("escape");
    assert!(!viewer.has_dialog());
    viewer.update(|view, cx| {
        view.change_font_size(9, cx);
        view.open_help(cx);
    });
    let body = find(&viewer, Hit::DialogScroll);
    assert!(body.h <= 460.);
    viewer.click(|hit| *hit == Hit::Button(Button::DialogClose));
    assert!(!viewer.has_dialog());
}

#[test]
fn information_sql_is_readonly_selectable_copyable_and_escape_restores_grid() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let sql = viewer.read(|view| view.session.sql.text());
    assert!(sql.contains("Result search and sort"));
    viewer.update(|view, cx| view.open_information(cx));
    viewer.run_until_parked();
    viewer.render();
    let (editor, shown) = information(&viewer, |panel| (panel.editor.id, panel.sql.clone()));
    viewer.focus(editor);
    viewer.press("ctrl-a");
    viewer.input("DELETE");
    viewer.press("backspace");
    assert_eq!(information(&viewer, |panel| panel.editor.text().to_owned()), shown);
    viewer.press("ctrl-c");
    assert_eq!(clipboard_text(), Some(shown.clone()));
    crate::ui::platform::system::write_clipboard("");
    viewer.click(|hit| *hit == Hit::Button(Button::CopySql));
    assert_eq!(clipboard_text(), Some(shown));
    viewer.press("escape");
    assert!(!viewer.has_dialog());
    assert!(viewer.grid_focused());
}

#[test]
fn pivot_sql_cache_and_errors_follow_the_published_result() {
    use crate::domain::documents::{Aggregation, AggregationOp, Condition, ConditionOp, ConditionValue, ScalarKey};
    use crate::domain::sql::SqlStatus;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| {
        view.session.start_query_draft("Pivot".into()).unwrap();
        let ViewKind::Query(config) = &mut view.session.working.as_mut().unwrap().view else {
            unreachable!()
        };
        config.group_by = vec!["sales".into()];
        config.pivot_by = vec!["region".into()];
        config.aggregations = vec![Aggregation {
            sql: None,
            source_column: "sales".into(),
            operation: AggregationOp::Sum,
            name: None,
            conditions: vec![Condition {
                column: "region".into(),
                operation: ConditionOp::Equal,
                values: vec![ConditionValue::Literal(ScalarKey::String("North".into()))],
            }],
            percentile: None,
            interpolation: None,
            ddof: None,
        }];
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.sql.displayed.statements.len(), 4);
    });
    viewer.update(|view, cx| view.set_search_text("10".into(), cx));
    viewer.run_until_parked();
    let previous = viewer.read(|view| {
        let statements = &view.session.sql.displayed.statements;
        assert_eq!(statements.len(), 4);
        assert!(statements[..3].iter().all(|statement| statement.status == SqlStatus::Cached));
        assert_eq!(statements[3].status, SqlStatus::Succeeded);
        view.session.result.result_revision
    });
    viewer.update(|view, cx| {
        let ViewKind::Query(config) = &mut view.session.working.as_mut().unwrap().view else {
            unreachable!()
        };
        config.pivot_by = vec!["missing".into()];
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_some());
        assert_eq!(view.session.result.result_revision, previous);
        assert_eq!(view.session.sql.displayed.statements.len(), 4);
        assert!(view.session.sql.failed.is_some());
    });
}

#[test]
fn header_keeps_only_contextual_clear_sort() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.render();
    // The header holds only search, the limit, the View options gear and
    // buttons that undo something.
    let buttons = viewer
        .app
        .hits(viewer.id)
        .into_iter()
        .filter(|region| matches!(region.hit, Hit::Button(_)) && region.hit != Hit::Button(Button::ViewOptions))
        .map(|region| region.hit)
        .collect::<Vec<_>>();
    assert!(buttons.is_empty(), "unexpected buttons {buttons:?}");
    viewer.update(|view, _| {
        view.session.set_sort_direction("sales".into(), true);
        let sort = view.session.sort.clone();
        view.grid.set_sort(sort);
    });
    assert!(has_button(&viewer, Button::ClearSort));
    viewer.click(|hit| *hit == Hit::Button(Button::ClearSort));
    assert!(viewer.read(|view| view.session.sort.is_empty()));
    assert!(!has_button(&viewer, Button::ClearSort));
}

#[test]
fn space_menu_is_scoped_to_grid_and_unknown_keys_leave_it_open() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.render();
    viewer.focus_search();
    viewer.press("space");
    assert!(!viewer.read(|view| view.space_menu_open));
    viewer.focus_grid();
    viewer.press("space");
    assert!(viewer.read(|view| view.space_menu_open));
    viewer.render();
    viewer.press("q");
    assert!(viewer.read(|view| view.space_menu_open));
    viewer.press("escape");
    assert!(!viewer.read(|view| view.space_menu_open));
}

fn frozen_count(viewer: &Viewer) -> usize {
    viewer.read(|view| view.session.effective_table_config().frozen_count)
}

#[test]
fn grid_copy_freeze_and_save_shortcuts_respect_focus() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("shortcuts.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let viewer = viewer_with_document(repository(home.path()), Some(source.to_string_lossy().into_owned()), None);
    viewer.render();
    viewer.focus_search();
    viewer.input("cf");
    assert_eq!(viewer.read(|view| view.search_input.text().to_owned()), "cf");
    assert_eq!(frozen_count(&viewer), 0);
    assert!(viewer.read(|view| view.copy_status.is_none()));
    viewer.update(|view, cx| view.set_search_text(String::new(), cx));
    viewer.run_until_parked();
    viewer.focus_grid();
    viewer.render();
    viewer.press("right c");
    viewer.run_until_parked();
    assert_eq!(clipboard_text(), Some("10".into()));
    viewer.press("f");
    assert_eq!(frozen_count(&viewer), 1);
    assert!(!viewer.read(|view| view.space_menu_open));
    viewer.press("f");
    assert_eq!(frozen_count(&viewer), 0);
    viewer.press("ctrl-s");
    assert!(viewer.has_dialog());
    assert!(has_button(&viewer, Button::DialogOk));
    viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
    assert!(!viewer.has_dialog(), "{:?}", viewer.read(|view| view.error.clone()));
    viewer.read(|view| {
        assert!(!view.session.dirty);
        assert_eq!(view.repository.load_views().0.len(), 1);
    });
}

#[test]
fn control_shift_digits_assign_all_favorite_slots() {
    use crate::application::state::ApplicationState;
    use crate::infrastructure::json_store::FavoriteTarget;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("favorite.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let repository = repository(home.path());
    let source_path = source.to_string_lossy().into_owned();
    let viewer = viewer_with_document(repository.clone(), Some(source_path.clone()), None);
    viewer.focus_grid();
    viewer.render();
    for digit in 1..=9 {
        viewer.press(&format!("ctrl-shift-{digit}"));
        viewer.read(|view| assert!(view.error.is_none(), "{:?}", view.error));
        assert_eq!(
            ApplicationState::load(repository.clone()).unwrap().settings.favorites[digit - 1],
            Some(FavoriteTarget::Source(source_path.clone()))
        );
    }
}

#[test]
fn changed_source_refreshes_all_open_windows_and_pivots() {
    use crate::domain::documents::{Dimension, QueryConfig};
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("sales.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let source_path = source.to_string_lossy().into_owned();
    let repository = repository(home.path());
    // The windows reread the file with the default engine.
    let mut settings = repository.load_settings().unwrap();
    settings.default_engine = crate::infrastructure::store::Engine::DuckDb;
    repository.save_settings(&settings).unwrap();
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let loaded = FrameSnapshot::new(database.load(&source, LoadMode::Check, true).unwrap(), 1);
    let table = viewer_with_document(repository.clone(), Some(source_path.clone()), None);
    let mut definition = DocumentDefinition::draft_table("By region".into(), source_path.clone());
    definition.view = ViewKind::Query(QueryConfig {
        group_by: vec!["region".into()],
        ..Default::default()
    });
    let query = Viewer::open_in(&table.app, sales_frame(), Some(source_path), Some(definition), repository);
    table.update(|view, cx| view.replace_dataframe(loaded.clone(), cx));
    query.update(|view, cx| view.replace_dataframe(loaded, cx));
    table.run_until_parked();
    assert_eq!(table.read(|view| view.session.result.row_count()), 2);
    assert_eq!(query.read(|view| view.session.result.row_count()), 2);
    std::fs::write(&source, "region,sales,extra\nEast,100,new\nEast,200,new\nWest,300,new\n").unwrap();
    table.advance_clock(Duration::from_secs(1));
    for viewer in [&table, &query] {
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.session.source.row_count(), 3);
            assert_eq!(view.session.source.columns.len(), 3);
            assert_eq!(view.session.source.store.lock().unwrap().duckdb().unwrap().table_sql(), "files.main.\"sales_csv\"");
        });
    }
    assert_eq!(table.read(|view| view.session.result.row_count()), 3);
    query.read(|view| {
        assert_eq!(view.session.result.row_count(), 2);
        let store = view.session.result.store.lock().unwrap();
        let rows = store.fetch_row_ids(&store.initial_row_ids().unwrap()).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.values[0].as_deref()).collect::<Vec<_>>(),
            [Some("East"), Some("West")]
        );
        assert!(
            matches!(&view.session.working.as_ref().unwrap().view, ViewKind::Query(config) if config.group_by == [Dimension::from("region")])
        );
    });
}

fn table_frozen(viewer: &Viewer) -> Option<usize> {
    viewer.read(|view| {
        view.session.working.as_ref().and_then(|definition| match &definition.view {
            ViewKind::Table(table) => Some(table.frozen_count),
            _ => None,
        })
    })
}

#[test]
fn space_menu_table_commands_sort_and_unfreeze() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    press_in_grid(&viewer, "space ]");
    assert!(!viewer.read(|view| view.space_menu_open));
    viewer.read(|view| {
        assert_eq!(view.session.sort[0].column, "region");
        assert!(view.session.sort[0].descending);
    });
    viewer.press("space f");
    assert_eq!(table_frozen(&viewer), Some(1));
    viewer.press("space shift-f");
    assert_eq!(table_frozen(&viewer), Some(0));
}

fn query_config(viewer: &Viewer) -> Option<crate::domain::documents::QueryConfig> {
    viewer.read(|view| {
        view.session.working.as_ref().and_then(|definition| match &definition.view {
            ViewKind::Query(config) => Some(config.clone()),
            _ => None,
        })
    })
}

#[test]
fn space_menu_opens_group_pivot_and_aggregation_workflows() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.focus_grid();
    viewer.render();
    viewer.press("space");
    assert!(viewer.read(|view| view.space_menu_open));
    viewer.render();
    viewer.press("g");
    assert!(!viewer.read(|view| view.space_menu_open));
    assert!(viewer.has_dialog());
    viewer.render();
    viewer.input("region");
    viewer.render();
    viewer.press("enter");
    viewer.run_until_parked();
    assert_eq!(query_config(&viewer).unwrap().group_by, vec!["region".into()]);
    assert!(viewer.read(|view| view.error.is_none()));
    assert!(viewer.has_dialog());
    viewer.close_dialog();

    viewer.focus_grid();
    viewer.press("p");
    viewer.render();
    viewer.input("sales");
    viewer.press("enter");
    viewer.run_until_parked();
    assert_eq!(query_config(&viewer).unwrap().pivot_by, vec!["sales".into()]);
    assert!(viewer.has_dialog());
    viewer.close_dialog();
    viewer.focus_grid();
    viewer.press("a");
    viewer.render();
    viewer.input("sales");
    viewer.press("enter");
    viewer.run_until_parked();
    let aggregations = query_config(&viewer).unwrap().aggregations;
    assert_eq!(aggregations.len(), 1);
    assert_eq!(aggregations[0].operation, crate::domain::documents::AggregationOp::Sum);
    assert_eq!(aggregations[0].name, None);
    assert!(viewer.has_dialog());
}

#[test]
fn group_right_pane_reorders_and_removes_selected_field() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    press_in_grid(&viewer, "g");
    press(&viewer, "enter");
    viewer.close_dialog();
    press_in_grid(&viewer, "g");
    viewer.input("sales");
    press(&viewer, "enter");
    assert!(viewer.has_dialog());
    viewer.close_dialog();
    press_in_grid(&viewer, "g");
    viewer.press("tab");
    press(&viewer, "alt-down");
    assert_eq!(query_config(&viewer).unwrap().group_by, vec!["sales".into(), "region".into()]);
    press(&viewer, "delete");
    assert_eq!(query_config(&viewer).unwrap().group_by, vec!["sales".into()]);
}

#[test]
fn aggregation_right_pane_edits_and_removes_selected_measure() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_aggregation_panel(cx));
    viewer.render();
    viewer.input("sales");
    press(&viewer, "enter");
    assert!(viewer.has_dialog());
    let aggregation = viewer.read(|view| view.session.query_config().aggregations[0].clone());
    assert_eq!(aggregation.operation, crate::domain::documents::AggregationOp::Sum);
    viewer.render();
    viewer.press("tab");
    viewer.render();
    viewer.press("enter");
    assert!(has_button(&viewer, Button::PanelApply));
    press(&viewer, "delete");
    viewer.read(|view| {
        assert!(view.session.query_config().aggregations.is_empty());
        assert!(view.is_table());
    });
    assert!(viewer.has_dialog());
}

type Focus = Option<crate::ui::widgets::WidgetId>;

/// Changes the open aggregation editor, then lets the view handle its events.
fn aggregation<R>(
    viewer: &Viewer,
    f: impl FnOnce(&mut crate::ui::aggregation_editor::AggregationPanel, &mut Focus) -> R,
) -> R {
    viewer.update(|view, _| {
        let mut focus = view.focus;
        let Some(Dialog::Aggregation(panel)) = &mut view.dialog else {
            panic!("the aggregation editor is open")
        };
        let result = f(panel, &mut focus);
        view.focus = focus;
        result
    })
}

fn aggregation_editor(home: &tempfile::TempDir) -> Viewer {
    let viewer = viewer(home);
    viewer.update(|view, cx| view.open_aggregation_panel(cx));
    viewer
}

/// Replaces the SQL silently, as code would, without an edit event.
fn set_aggregation_sql(viewer: &Viewer, sql: &str) {
    aggregation(viewer, |panel, _| panel.sql.set_text(sql));
}

fn aggregation_sql(viewer: &Viewer) -> String {
    aggregation(viewer, |panel, _| panel.sql.text().to_owned())
}

fn aggregation_error(viewer: &Viewer) -> Option<String> {
    aggregation(viewer, |panel, _| panel.error.clone())
}

fn aggregations(viewer: &Viewer) -> Vec<crate::domain::documents::Aggregation> {
    viewer.read(|view| view.session.query_config().aggregations)
}

#[test]
fn aggregation_function_reference_inserts_a_call_into_custom_sql() {
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    viewer.click(|hit| *hit == Hit::Button(Button::PanelShowFunctions));
    viewer.run_until_parked();
    viewer.render();
    // Descriptions are searchable as well as names.
    viewer.input("deviation");
    viewer.render();
    let sql_id = aggregation(&viewer, |panel, _| panel.sql.id);
    assert!(viewer.find(|hit| matches!(hit, Hit::Editor(id, _) if *id == sql_id)).is_none());
    viewer.run_until_parked();
    assert_eq!(aggregation_sql(&viewer), "");
    // Clear the query and look the function up by name; `median` comes
    // before `mad`, whose description mentions the median.
    for _ in 0.."deviation".len() {
        viewer.press("backspace");
    }
    viewer.input("median");
    viewer.run_until_parked();
    press(&viewer, "enter");
    assert_eq!(aggregation_sql(&viewer), "median()");
    // The cursor waits between the parentheses for a column.
    viewer.input("\"sales\"");
    assert_eq!(aggregation_sql(&viewer), "median(\"sales\")");
    press(&viewer, "ctrl-enter");
    let aggregations = aggregations(&viewer);
    assert_eq!(aggregations.len(), 1);
    assert_eq!(aggregations[0].sql.as_deref(), Some("median(\"sales\")"));
}

#[test]
fn aggregation_sql_fills_its_area_and_scrolls_multiline_content() {
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    let sql = (0..40).map(|_| "-- SQL line\n").collect::<String>() + "count(*)";
    aggregation(&viewer, |panel, focus| panel.start_custom(focus));
    let sql_id = aggregation(&viewer, |panel, _| panel.sql.id);
    for (width, height) in [(1400., 900.), (1000., 700.)] {
        viewer.resize(width, height);
        set_aggregation_sql(&viewer, &sql);
        viewer.render();
        let editor = viewer
            .app
            .hits(viewer.id)
            .into_iter()
            .filter(|region| matches!(region.hit, Hit::Editor(id, _) if id == sql_id))
            .map(|region| region.rect)
            .fold(None::<Rect>, |all, r| {
                Some(all.map_or(r, |all| {
                    let (x, y) = (all.x.min(r.x), all.y.min(r.y));
                    crate::ui::platform::rect(x, y, all.right().max(r.right()) - x, all.bottom().max(r.bottom()) - y)
                }))
            })
            .expect("the SQL editor is painted");
        let expected_height = (height - 560_f32).clamp(70., 220.);
        assert!((expected_height - editor.h).abs() <= 2., "{} vs {expected_height}", editor.h);
        let apply = find(&viewer, Hit::Button(Button::PanelApply));
        let before = aggregation(&viewer, |panel, _| panel.sql.scroll_offset());
        viewer.app.scroll(viewer.id, editor.center(), point(0., -5.));
        assert!(aggregation(&viewer, |panel, _| panel.sql.scroll_offset()) > before);
        assert_eq!(find(&viewer, Hit::Button(Button::PanelApply)), apply);
        assert_eq!(aggregation_sql(&viewer), sql);
    }
}

#[test]
fn aggregation_sql_applies_with_ctrl_enter_and_preserves_success_on_errors() {
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.render();
    viewer.input("count(*) AS Rows");
    press(&viewer, "ctrl-enter");
    let successful = viewer.read(|view| {
        let config = view.session.query_config();
        assert!(!config.aggregations.is_empty(), "view error: {:?}", view.error);
        assert_eq!(config.aggregations[0].sql.as_deref(), Some("count(*) AS Rows"));
        assert!(config.aggregations[0].source_column.is_empty());
        assert_eq!(view.session.result.columns[0].0, "Rows");
        assert_eq!(view.session.result.row_count(), 1);
        view.session.result.clone()
    });
    set_aggregation_sql(&viewer, "sum(\"missing\")");
    press(&viewer, "ctrl-enter");
    assert!(aggregation_error(&viewer).unwrap().contains("missing"));
    assert_eq!(aggregations(&viewer)[0].sql.as_deref(), Some("count(*) AS Rows"));
    assert!(std::sync::Arc::ptr_eq(
        &viewer.read(|view| view.session.result.store.clone()),
        &successful.store
    ));
    assert_eq!(aggregation_sql(&viewer), "sum(\"missing\")");
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.run_until_parked();
    assert!(aggregation_error(&viewer).is_some());
    assert_eq!(aggregations(&viewer).len(), 1);
    viewer.click(|hit| *hit == Hit::Button(Button::PanelRevert));
    assert_eq!(aggregation_sql(&viewer), "count(*) AS Rows");
    set_aggregation_sql(&viewer, "sum(\"sales\")");
    aggregation(&viewer, |panel, focus| panel.focus_sql(focus));
    viewer.render();
    viewer.press("escape");
    assert!(!viewer.has_dialog());
    viewer.update(|view, cx| view.open_aggregation_panel(cx));
    assert_eq!(aggregation_sql(&viewer), "count(*) AS Rows");
}

#[test]
fn aggregation_selection_applies_sql_aliases_and_reorders_them() {
    use crate::ui::widgets::list::Part;
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    viewer.render();
    viewer.input("sales");
    press(&viewer, "enter");
    assert_eq!(viewer.read(|view| view.session.result.columns[0].0.clone()), "sum(sales)");
    set_aggregation_sql(&viewer, "sum(\"sales\") AS Revenue");
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.run_until_parked();
    let saved = aggregations(&viewer)[0].clone();
    assert!(saved.name.is_none());
    assert_eq!(saved.sql.as_deref(), Some("sum(\"sales\") AS Revenue"));
    assert_eq!(viewer.read(|view| view.session.result.columns[0].0.clone()), "Revenue");
    set_aggregation_sql(&viewer, "count(*)");
    aggregation(&viewer, |panel, _| panel.apply());
    viewer.run_until_parked();
    assert_eq!(aggregations(&viewer).len(), 2);
    let list = aggregation(&viewer, |panel, _| panel.list.id);
    set_aggregation_sql(&viewer, "avg(\"sales\")");
    viewer.click(|hit| *hit == Hit::List(list, Part::Row(0)));
    viewer.run_until_parked();
    assert_eq!(aggregations(&viewer)[1].sql.as_deref(), Some("avg(\"sales\")"));
    assert_eq!(aggregation_sql(&viewer), "sum(\"sales\") AS Revenue");
    set_aggregation_sql(&viewer, "sum(\"missing\")");
    viewer.click(|hit| *hit == Hit::List(list, Part::Row(1)));
    viewer.run_until_parked();
    assert!(aggregation_error(&viewer).is_some());
    assert_eq!(aggregation_sql(&viewer), "sum(\"missing\")");
    aggregation(&viewer, |panel, focus| panel.revert(focus));
    press(&viewer, "alt-down");
    assert_eq!(aggregations(&viewer)[1].sql.as_deref(), Some("sum(\"sales\") AS Revenue"));
}

#[test]
fn aggregation_invalid_selection_and_stale_checks_cannot_change_applied_config() {
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    aggregation(&viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text("count(*)");
        panel.apply();
        // The text changes before the check reports.
        panel.sql.set_text("count(*) + 1");
    });
    viewer.run_until_parked();
    assert!(aggregations(&viewer).is_empty());
    assert!(aggregation_error(&viewer).unwrap().contains("changed"));
    aggregation(&viewer, |panel, focus| {
        panel.revert(focus);
        panel.start_custom(focus);
        panel.sql.set_text("count(*)");
        panel.apply();
        panel.revert(focus);
        panel.start_custom(focus);
        panel.sql.set_text("sum(\"sales\")");
        panel.apply();
    });
    viewer.run_until_parked();
    assert_eq!(aggregations(&viewer)[0].sql.as_deref(), Some("sum(\"sales\")"));
    assert!(aggregation_error(&viewer).is_none());
    set_aggregation_sql(&viewer, "sum(\"missing\")");
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.run_until_parked();
    assert!(aggregation_error(&viewer).is_some());
    assert_eq!(aggregation_sql(&viewer), "sum(\"missing\")");
    aggregation(&viewer, |panel, focus| {
        panel.revert(focus);
        panel.sql.set_text("count(*)");
        panel.apply();
        panel.close();
    });
    viewer.close_dialog();
    viewer.run_until_parked();
    assert_eq!(aggregations(&viewer)[0].sql.as_deref(), Some("sum(\"sales\")"));
}

#[test]
fn aggregation_column_addition_applies_pending_custom_sql_and_pivot_filter_is_inline() {
    use crate::domain::documents::{Aggregation, AggregationOp, QueryConfig};
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    aggregation(&viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text("count(*)");
        panel.focus_available(focus);
    });
    viewer.render();
    viewer.input("region");
    press(&viewer, "enter");
    let config = viewer.read(|view| view.session.query_config());
    assert_eq!(config.aggregations.len(), 2);
    assert_eq!(config.aggregations[0].sql.as_deref(), Some("count(*)"));
    assert_eq!(config.aggregations[1].operation, AggregationOp::NonNullCount);
    assert_eq!(aggregation_sql(&viewer), "count(\"region\")");
    assert!(viewer.has_dialog());
    viewer.close_dialog();
    viewer.update(|view, cx| {
        view.session
            .set_query_config(
                QueryConfig {
                    group_by: vec!["region".into()],
                    pivot_by: vec!["sales".into()],
                    aggregations: vec![Aggregation::sql_expression("count(*)".into(), None)],
                    ..Default::default()
                },
                "Pivot".into(),
            )
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.update(|view, cx| view.open_aggregation_panel(cx));
    aggregation(&viewer, |panel, _| {
        panel.sql.set_text("count(*) FILTER (WHERE true)");
        panel.apply();
    });
    assert!(aggregation_error(&viewer).unwrap().contains("DuckDB ignores FILTER"));
    assert_eq!(aggregations(&viewer)[0].sql.as_deref(), Some("count(*)"));
    assert!(has_button(&viewer, Button::PanelLimitations));
}

fn query_by_region(viewer: &Viewer) {
    use crate::domain::documents::QueryConfig;
    viewer.update(|view, cx| {
        view.session
            .set_query_config(
                QueryConfig {
                    group_by: vec!["region".into()],
                    ..Default::default()
                },
                "Query".into(),
            )
            .unwrap();
        view.session.dirty = false;
        view.schedule_search(String::new(), cx);
    });
}

fn limit_shown(viewer: &Viewer) -> bool {
    viewer.render();
    let limit = viewer.read(|view| view.limit_input.id);
    viewer.find(|hit| matches!(hit, Hit::Editor(id, _) if *id == limit)).is_some()
}

fn focus_limit(viewer: &Viewer) {
    let limit = viewer.read(|view| view.limit_input.id);
    viewer.focus(limit);
}

#[test]
fn query_limit_commits_globally_preserves_dirty_state_and_reuses_search_cache() {
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let first = viewer_with_repository(repository.clone());
    let second = Viewer::open_in(&first.app, sales_frame(), None, None, repository.clone());
    query_by_region(&first);
    query_by_region(&second);
    first.run_until_parked();
    assert!(limit_shown(&first));
    focus_limit(&first);
    first.press("ctrl-a");
    first.input("1");
    assert_eq!(first.read(|view| view.query_limit), 100000);
    press(&first, "enter");
    for viewer in [&first, &second] {
        viewer.render();
        viewer.run_until_parked();
        viewer.read(|view| {
            assert_eq!(view.query_limit, 1);
            assert_eq!(view.limit_input.text(), "1");
            assert_eq!(view.session.result.row_count(), 1);
            assert!(!view.session.dirty);
        });
    }
    assert_eq!(repository.load_settings().unwrap().query_limit, 1);
    let cached = first.read(|view| view.query_cache.as_ref().unwrap().frame.store.clone());
    first.update(|view, cx| view.set_search_text("South".into(), cx));
    first.run_until_parked();
    first.read(|view| {
        // The cached result stopped at the limit of 1, so the search reran
        // inside the query and found South past it; the cache is unchanged.
        assert_eq!(view.session.result.row_count(), 1);
        assert!(Arc::ptr_eq(&cached, &view.query_cache.as_ref().unwrap().frame.store));
        assert!(view.session.sql.displayed.statements[0].status == crate::domain::sql::SqlStatus::Cached);
    });
    first.render();
    focus_limit(&first);
    first.press("ctrl-a");
    first.input("2");
    first.render();
    first.run_until_parked();
    // Leaving the box commits the limit.
    first.focus_grid();
    first.render();
    first.run_until_parked();
    first.read(|view| {
        assert_eq!(view.query_limit, 2);
        assert_eq!(view.session.result.row_count(), 1);
        assert!(!Arc::ptr_eq(&cached, &view.query_cache.as_ref().unwrap().frame.store));
        assert!(!view.session.dirty);
    });
}

#[test]
fn query_limit_invalid_escape_failed_save_and_keyboard_focus() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let viewer = viewer_with_repository(repository.clone());
    query_by_region(&viewer);
    viewer.run_until_parked();
    viewer.render();
    let (search, limit, grid) = viewer.read(|view| (view.search_input.id, view.limit_input.id, view.grid.id));
    viewer.focus(search);
    viewer.press("tab");
    assert_eq!(viewer.focused(), Some(limit));
    // Typed keys reach the box the way the window delivers them.
    viewer.press("ctrl-a 2 5");
    assert_eq!(viewer.read(|view| view.limit_input.text().to_owned()), "25");
    viewer.press("backspace");
    assert_eq!(viewer.read(|view| view.limit_input.text().to_owned()), "2");
    viewer.press("ctrl-a");
    viewer.input("0");
    viewer.press("enter");
    assert_eq!(viewer.read(|view| view.query_limit), 100000);
    viewer.run_until_parked();
    viewer.render();
    assert!(viewer.read(|view| view.limit_error.is_some()));
    viewer.press("escape");
    viewer.read(|view| {
        assert_eq!(view.limit_input.text(), "100000");
        assert!(view.limit_error.is_none());
    });
    viewer.press("ctrl-a");
    viewer.input("gap i/ ");
    assert!(!viewer.has_dialog());
    assert!(!viewer.read(|view| view.space_menu_open));
    viewer.press("escape");
    viewer.press("shift-tab");
    assert_eq!(viewer.focused(), Some(search));
    viewer.press("tab tab");
    assert_eq!(viewer.focused(), Some(grid));
    viewer.press("shift-tab");
    assert_eq!(viewer.focused(), Some(limit));
    let blocked = home.path().join("blocked-config");
    std::fs::write(&blocked, "blocked").unwrap();
    viewer.update(|view, cx| {
        view.repository.paths.config = blocked;
        view.limit_input.set_text("1");
        view.commit_query_limit(cx);
    });
    viewer.read(|view| {
        assert_eq!(view.query_limit, 100000);
        assert!(view.limit_error.as_ref().unwrap().contains("Could not save"));
        assert_eq!(view.session.result.row_count(), 2);
    });
    assert_eq!(repository.load_settings().unwrap().query_limit, 100000);
}

#[test]
fn empty_query_and_pivot_setup_keep_table_controls_and_avoid_sql() {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::{QueryConfig, SlicerDefinition};
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    viewer.update(|view, cx| view.open_pivot_panel(PivotFieldKind::GroupBy, cx));
    assert!(viewer.read(|view| view.is_table()));
    viewer.close_dialog();
    viewer.update(|view, cx| {
        view.session
            .set_query_config(
                QueryConfig {
                    pivot_by: vec!["region".into()],
                    ..Default::default()
                },
                "Query".into(),
            )
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    assert!(limit_shown(&viewer));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(!view.busy);
        assert!(view.error.is_none());
    });
    viewer.update(|view, cx| {
        view.session.working.as_mut().unwrap().slicers = vec![SlicerDefinition {
            sql: false,
            condition: None,
            column: "region".into(),
            selected: vec![],
        }];
        view.session
            .set_query_config(QueryConfig::default(), "unused".into())
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    assert!(viewer.read(|view| view.is_table()));
    assert!(!limit_shown(&viewer));
    assert_eq!(picked_slicers(&viewer).len(), 1);
}

fn dimension<R>(viewer: &Viewer, f: impl FnOnce(&mut crate::ui::pivot_panel::PivotPanel, &mut Focus) -> R) -> R {
    viewer.update(|view, _| {
        let mut focus = view.focus;
        let Some(Dialog::Pivot(panel)) = &mut view.dialog else {
            panic!("a dimension editor is open")
        };
        let result = f(panel, &mut focus);
        view.focus = focus;
        result
    })
}

fn dimension_editor(home: &tempfile::TempDir, kind: crate::application::pivot_editor::PivotFieldKind) -> Viewer {
    let viewer = viewer(home);
    viewer.update(|view, cx| view.open_pivot_panel(kind, cx));
    viewer
}

fn dimension_sql(viewer: &Viewer) -> String {
    dimension(viewer, |panel, _| panel.sql.text().to_owned())
}

fn dimension_error(viewer: &Viewer) -> Option<String> {
    dimension(viewer, |panel, _| panel.error.clone())
}

/// Starts a custom dimension with `sql` and applies it.
fn apply_custom_dimension(viewer: &Viewer, sql: &str) {
    dimension(viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text(sql);
        panel.apply();
    });
}

#[test]
fn dimension_expressions_apply_in_both_managers_and_errors_preserve_results() {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.render();
    viewer.input("upper(region) AS RegionCode");
    press(&viewer, "ctrl-enter");
    assert!(viewer.has_dialog());
    assert!(dimension_error(&viewer).is_none(), "{:?}", dimension_error(&viewer));
    let successful = viewer.read(|view| {
        assert_eq!(
            view.session.query_config().group_by,
            [Dimension::expression("upper(region) AS RegionCode")]
        );
        assert_eq!(view.session.result.columns[0].0, "RegionCode");
        view.session.result.clone()
    });
    dimension(&viewer, |panel, _| panel.sql.set_text("upper(missing)"));
    press(&viewer, "ctrl-enter");
    assert!(dimension_error(&viewer).unwrap().contains("missing"));
    assert!(std::sync::Arc::ptr_eq(
        &viewer.read(|view| view.session.result.store.clone()),
        &successful.store
    ));
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.run_until_parked();
    assert!(dimension_error(&viewer).is_some());
    assert_eq!(dimension_sql(&viewer), "upper(missing)");
    dimension(&viewer, |panel, focus| panel.revert(focus));
    assert_eq!(dimension_sql(&viewer), "upper(region) AS RegionCode");
    viewer.close_dialog();
    viewer.update(|view, cx| view.open_pivot_panel(PivotFieldKind::PivotBy, cx));
    dimension(&viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text("sales / 10 AS Bucket");
    });
    press(&viewer, "ctrl-enter");
    assert!(dimension_error(&viewer).is_none(), "{:?}", dimension_error(&viewer));
    assert!(viewer.has_dialog());
    viewer.read(|view| {
        assert_eq!(
            view.session.query_config().pivot_by,
            [Dimension::expression("sales / 10 AS Bucket")]
        );
        assert_eq!(view.session.result.columns[0].0, "RegionCode");
        assert!(view.session.result.columns.len() > 1);
    });
}

#[test]
fn dimension_column_edits_aliases_reorder_focus_and_return_to_table() {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    use crate::ui::widgets::list::Part;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    viewer.render();
    viewer.input("region");
    press(&viewer, "enter");
    assert_eq!(dimension_sql(&viewer), "\"region\"");
    dimension(&viewer, |panel, _| panel.sql.set_text("upper(region) AS Code"));
    viewer.click(|hit| *hit == Hit::Button(Button::PanelCustom));
    viewer.run_until_parked();
    assert_eq!(
        viewer.read(|view| view.session.query_config().group_by),
        [Dimension::expression("upper(region) AS Code")]
    );
    dimension(&viewer, |panel, _| {
        panel.sql.set_text("sales AS Amount");
        panel.apply();
    });
    viewer.run_until_parked();
    let list = dimension(&viewer, |panel, _| panel.list.id);
    viewer.click(|hit| *hit == Hit::List(list, Part::Row(0)));
    viewer.run_until_parked();
    assert_eq!(dimension_sql(&viewer), "upper(region) AS Code");
    viewer.render();
    viewer.press("tab");
    let sql_id = dimension(&viewer, |panel, _| panel.sql.id);
    assert_eq!(viewer.focused(), Some(sql_id));
    viewer.press("shift-tab");
    press(&viewer, "alt-down");
    assert_eq!(
        viewer.read(|view| view.session.query_config().group_by),
        [
            Dimension::expression("sales AS Amount"),
            Dimension::expression("upper(region) AS Code")
        ]
    );
    press(&viewer, "delete");
    assert_eq!(viewer.read(|view| view.session.query_config().group_by.len()), 1);
    press(&viewer, "delete");
    viewer.read(|view| {
        assert!(view.is_table());
        assert!(view.session.query_config().group_by.is_empty());
    });
    assert!(viewer.has_dialog());
}

#[test]
fn dimension_pending_setup_validates_and_then_builds_a_pivot() {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::PivotBy);
    let previous = viewer.read(|view| view.session.result.clone());
    apply_custom_dimension(&viewer, "sum(sales) AS Bad");
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.session.query_config().pivot_by.is_empty()));
    assert!(dimension_error(&viewer).is_some());
    dimension(&viewer, |panel, _| {
        panel.sql.set_text("sales / 10 AS Bucket");
        panel.apply();
    });
    viewer.run_until_parked();
    assert!(dimension_error(&viewer).is_none(), "{:?}", dimension_error(&viewer));
    viewer.read(|view| {
        assert_eq!(
            view.session.query_config().pivot_by,
            [Dimension::expression("sales / 10 AS Bucket")]
        );
        assert!(std::sync::Arc::ptr_eq(&view.session.result.store, &previous.store));
        assert!(!view.busy);
    });
    viewer.close_dialog();
    viewer.update(|view, cx| view.open_pivot_panel(PivotFieldKind::GroupBy, cx));
    apply_custom_dimension(&viewer, "upper(region) AS Code");
    viewer.run_until_parked();
    assert!(dimension_error(&viewer).is_none(), "{:?}", dimension_error(&viewer));
    viewer.read(|view| {
        assert_eq!(view.session.result.columns[0].0, "Code");
        assert!(view.session.result.columns.len() > 1);
    });
}

#[test]
fn dimension_stale_and_closed_checks_cannot_commit() {
    use crate::application::pivot_editor::PivotFieldKind;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    dimension(&viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text("upper(region)");
        panel.apply();
        // The text changes before the check reports.
        panel.sql.set_text("lower(region)");
    });
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.session.query_config().group_by.is_empty()));
    assert!(dimension_error(&viewer).unwrap().contains("changed"));
    dimension(&viewer, |panel, focus| {
        panel.revert(focus);
        panel.start_custom(focus);
        panel.sql.set_text("upper(region)");
        panel.apply();
    });
    viewer.click(|hit| *hit == Hit::Button(Button::PanelClose));
    assert!(!viewer.has_dialog());
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.session.query_config().group_by.is_empty()));
}

#[test]
fn dimension_pending_column_addition_and_duplicate_output_names() {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    apply_custom_dimension(&viewer, "upper(region) AS Code");
    viewer.run_until_parked();
    apply_custom_dimension(&viewer, "lower(region) AS code");
    viewer.run_until_parked();
    assert!(dimension_error(&viewer).unwrap().contains("duplicate dimension output name"));
    assert_eq!(viewer.read(|view| view.session.query_config().group_by.len()), 1);
    dimension(&viewer, |panel, focus| {
        panel.revert(focus);
        panel.start_custom(focus);
        panel.sql.set_text("sales % 10 AS Bucket");
        panel.focus_available(focus);
    });
    viewer.render();
    viewer.input("region");
    press(&viewer, "enter");
    assert!(dimension_error(&viewer).is_none(), "{:?}", dimension_error(&viewer));
    assert_eq!(
        viewer.read(|view| view.session.query_config().group_by),
        [
            Dimension::expression("upper(region) AS Code"),
            Dimension::expression("sales % 10 AS Bucket"),
            Dimension::from("region")
        ]
    );
    assert_eq!(dimension_sql(&viewer), "\"region\"");
    assert!(viewer.has_dialog());
}

#[test]
fn expression_group_can_be_removed_from_its_result_column() {
    use crate::application::pivot_editor::PivotFieldKind;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    apply_custom_dimension(&viewer, "upper(region) AS Code");
    viewer.run_until_parked();
    press(&viewer, "escape");
    viewer.select(0, 0);
    press(&viewer, "delete");
    viewer.read(|view| {
        assert!(view.session.query_config().group_by.is_empty());
        assert!(view.is_table());
    });
}

#[test]
fn dimension_check_rejects_session_changes() {
    use crate::application::pivot_editor::PivotFieldKind;
    let home = tempfile::tempdir().unwrap();
    let viewer = dimension_editor(&home, PivotFieldKind::GroupBy);
    apply_custom_dimension(&viewer, "upper(region) AS Code");
    viewer.update(|view, cx| view.schedule_search("10".into(), cx));
    viewer.run_until_parked();
    assert!(viewer.read(|view| view.session.query_config().group_by.is_empty()));
    assert!(dimension_error(&viewer).unwrap().contains("changed"));
}

/// The three-card window: region values, a sales condition and flag values.
fn slicer_cards(home: &tempfile::TempDir) -> Viewer {
    let viewer = viewer(home);
    viewer.resize(1400., 900.);
    let source = DuckDbStore::from_rows(
        &[
            ("region".into(), "VARCHAR".into()),
            ("sales".into(), "BIGINT".into()),
            ("flag".into(), "BOOLEAN".into()),
        ],
        [
            vec![Value::Text("North".into()), Value::BigInt(10), Value::Boolean(true)],
            vec![Value::Text("South".into()), Value::BigInt(20), Value::Boolean(false)],
        ],
    )
    .unwrap();
    viewer.update(|view, cx| {
        view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
        view.add_slicer("region".into(), cx);
        view.add_condition_slicer("sales".into(), cx);
        view.add_slicer("flag".into(), cx);
    });
    viewer.settle();
    viewer
}

fn slicer_value_shown(viewer: &Viewer, column: &str, index: usize) -> bool {
    viewer.render();
    viewer
        .find(|hit| {
            *hit == Hit::Slicer(SlicerHit::Value {
                column: column.into(),
                index,
            })
        })
        .is_some()
}

#[test]
fn ctrl_l_cycles_slicer_inputs_and_ctrl_d_returns_to_the_grid() {
    let home = tempfile::tempdir().unwrap();
    let viewer = slicer_cards(&home);
    // From the grid, the first card's value filter opens and takes the typing.
    viewer.focus_grid();
    press(&viewer, "ctrl-l");
    viewer.input("No");
    assert!(slicer_value_shown(&viewer, "region", 0));
    assert!(!slicer_value_shown(&viewer, "region", 1));
    // The condition card is next; moving on commits its text.
    press(&viewer, "ctrl-l");
    viewer.input(">15");
    press(&viewer, "ctrl-l");
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 1);
    // After the last card, focus wraps to the first, whose filter text stays.
    press(&viewer, "ctrl-l");
    assert!(slicer_value_shown(&viewer, "region", 0));
    assert!(!slicer_value_shown(&viewer, "region", 1));
    viewer.press("ctrl-d");
    assert!(viewer.grid_focused());
}

#[test]
fn ctrl_l_without_slicers_reports_how_to_add_one() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    press(&viewer, "ctrl-l");
    assert!(viewer.grid_focused());
    assert_eq!(
        viewer.read(|view| view.copy_status.clone()).as_deref(),
        Some("No slicers to focus. Use Add slicer to add one")
    );
}

#[test]
fn undo_steps_back_slicer_selections_and_cards_but_not_search_box_typing() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let source = DuckDbStore::from_rows(
        &[("region".into(), "VARCHAR".into())],
        [vec![Value::Text("North".into())], vec![Value::Text("South".into())]],
    )
    .unwrap();
    viewer.update(|view, cx| view.replace_dataframe(FrameSnapshot::new(source, 2), cx));
    viewer.settle();
    viewer.update(|view, cx| view.add_slicer("region".into(), cx));
    viewer.settle();
    viewer.click_at(slicer_value(&viewer, "region", 1).center());
    viewer.settle();
    let state = || {
        viewer.read(|view| {
            let cards = view
                .session
                .working
                .as_ref()
                .map_or(0, |definition| definition.slicers.len());
            (cards, view.session.result.row_count())
        })
    };
    assert_eq!(state(), (1, 1));
    // In the search box, ctrl-z undoes typing rather than the view.
    press(&viewer, "/");
    viewer.input("x");
    viewer.press("ctrl-z");
    viewer.settle();
    assert_eq!(viewer.read(|view| view.session.search_text.clone()), "");
    assert_eq!(state(), (1, 1));
    let undo = || {
        viewer.press("ctrl-d");
        viewer.render();
        viewer.press("ctrl-z");
        viewer.settle();
    };
    undo();
    assert_eq!(state(), (1, 2));
    undo();
    assert_eq!(state(), (0, 2));
}

#[test]
fn ctrl_f_focuses_search_from_the_grid_and_from_slicer_inputs() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(&home);
    let source =
        DuckDbStore::from_rows(&[("sales".into(), "BIGINT".into())], [vec![Value::BigInt(10)], vec![Value::BigInt(20)]])
            .unwrap();
    viewer.update(|view, cx| view.replace_dataframe(FrameSnapshot::new(source, 2), cx));
    viewer.settle();
    viewer.update(|view, cx| view.add_condition_slicer("sales".into(), cx));
    viewer.settle();
    viewer.focus_grid();
    press(&viewer, "ctrl-f");
    assert!(viewer.search_focused());
    press(&viewer, "ctrl-l");
    assert!(!viewer.search_focused());
    viewer.press("ctrl-f");
    assert!(viewer.search_focused());
}

#[test]
fn aggregation_sql_editor_lists_columns_after_a_quote_above_the_dialog() {
    let home = tempfile::tempdir().unwrap();
    let viewer = aggregation_editor(&home);
    aggregation(&viewer, |panel, focus| {
        panel.start_custom(focus);
        panel.sql.set_text("sum(");
        panel.sql.set_selection(4..4);
        panel.focus_sql(focus);
    });
    let menu = || {
        aggregation(&viewer, |panel, _| {
            panel
                .sql
                .completion
                .as_ref()
                .map(|menu| menu.items.iter().map(|item| item.label.clone()).collect::<Vec<_>>())
        })
    };
    let input = |text: &str| {
        viewer.render();
        viewer.input(text);
        viewer.run_until_parked();
    };
    input("\"");
    assert_eq!(menu(), Some(vec!["region".into(), "sales".into()]));
    // The list is drawn above the dialog, where it can be clicked.
    viewer.render();
    let sql_id = aggregation(&viewer, |panel, _| panel.sql.id);
    let surface = viewer
        .app
        .hits(viewer.id)
        .iter()
        .rposition(|region| region.hit == Hit::Surface)
        .unwrap();
    let menu_region = viewer
        .app
        .hits(viewer.id)
        .iter()
        .rposition(|region| matches!(region.hit, Hit::Editor(id, _) if id == sql_id))
        .unwrap();
    assert!(menu_region > surface);
    // Down then Enter picks the second column; the dialog stays open.
    press(&viewer, "down");
    press(&viewer, "enter");
    assert_eq!(aggregation_sql(&viewer), "sum(\"sales\"");
    assert!(menu().is_none());
    assert!(viewer.has_dialog());
    // Escape closes the list but not the dialog; Tab accepts.
    input(" + \"re");
    assert_eq!(menu(), Some(vec!["region".into()]));
    press(&viewer, "escape");
    assert!(menu().is_none());
    assert!(viewer.has_dialog());
    input("g");
    press(&viewer, "tab");
    assert_eq!(aggregation_sql(&viewer), "sum(\"sales\" + \"region\"");
    // A typed word lists what fits there: keywords that start an expression too.
    input(" * CA");
    assert!(menu().is_some_and(|labels| labels.contains(&"CASE".to_string())), "{:?}", menu());
    // Columns come first, then the aggregate functions the engine has.
    input("SE WHEN re");
    let labels = menu().unwrap();
    assert_eq!(labels[0], "region");
    assert!(labels.contains(&"regr_count".to_string()), "{labels:?}");
}
