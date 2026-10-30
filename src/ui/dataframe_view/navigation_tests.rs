use super::FileEntry;
use super::test_support::{Viewer, empty_frame, repository, set_clipboard};
use crate::application::commands::CommandId;
use crate::application::session::SessionIdentity;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, QueryConfig, ScalarKey, SlicerDefinition, SortKey,
    ViewKind,
};
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::hit::{Button, Hit};

fn viewer(repository: ViewRepository) -> Viewer {
    Viewer::open(empty_frame(), None, None, repository)
}

fn open(viewer: &Viewer, entry: FileEntry) {
    viewer.update(|view, cx| view.navigate_to_file(entry, false, false, false, cx));
    viewer.run_until_parked();
}

fn travel(viewer: &Viewer, forward: bool) {
    viewer.update(|view, cx| view.navigate_history(forward, cx));
    viewer.run_until_parked();
}

/// Presses each key in turn, painting first like a window would.
fn press_each(viewer: &Viewer, keys: &[&str]) {
    for key in keys {
        viewer.render();
        viewer.press(key);
        viewer.run_until_parked();
    }
}

#[test]
fn database_tables_open_in_table_mode_sort_and_refresh() {
    use crate::domain::documents::{DatabaseView, RelationKind};
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 3;
    repository.save_settings(&settings).unwrap();
    let viewer = viewer(repository);
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("CREATE TABLE main.sales AS SELECT range AS n FROM range(100)")
            .unwrap()
    });
    let table = DatabaseView {
        kind: RelationKind::Table,
        ..DatabaseView::view("main", "sales")
    };
    assert!(viewer.read(|view| view.database.relations().unwrap()).contains(&table));
    open(&viewer, FileEntry::View(DocumentDefinition::draft_database_view(table)));
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.identity, SessionIdentity::DraftTable);
        // Table mode reads every row; the query limit does not apply.
        assert_eq!(view.session.result.row_count(), 100);
    });
    viewer.update(|view, cx| {
        view.session.set_sort_direction("n".into(), true);
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.result.fetch_rows(0, 1).unwrap()[0].values[0], Some("99".into()));
    });
    viewer.update(|view, cx| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("INSERT INTO main.sales VALUES (500)")
            .unwrap();
        view.database.invalidate();
        view.check_source(cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 101);
        assert_eq!(view.session.result.fetch_rows(0, 1).unwrap()[0].values[0], Some("500".into()));
    });
    // A query on the table uses the limit, and clearing it returns to table mode.
    viewer.update(|view, cx| {
        view.session.start_query_draft("sales".into()).unwrap();
        view.session
            .set_query_config(
                QueryConfig {
                    group_by: vec!["n".into()],
                    ..Default::default()
                },
                "sales".into(),
            )
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
    });
    viewer.update(|view, cx| {
        view.session
            .set_query_config(QueryConfig::default(), "sales".into())
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 101);
    });
}

#[test]
fn database_views_use_query_mode_limits_full_aggregations_and_persist() {
    use crate::domain::documents::DatabaseView;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 3;
    repository.save_settings(&settings).unwrap();
    let viewer = viewer(repository.clone());
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("CREATE VIEW main.sales AS SELECT range AS n, range % 2 AS category FROM range(100)")
            .unwrap()
    });
    viewer.update(|view, cx| view.open_file_palette(cx));
    viewer.render();
    viewer.input("main.sales");
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.file_groups[4].len(), 1);
        assert!(!view.file_groups[4][0].is_saved_view());
    });
    viewer.render();
    viewer.press("enter");
    viewer.run_until_parked();
    viewer.render();
    let limit = viewer.read(|view| view.limit_input.id);
    assert!(viewer.find(|hit| matches!(hit, Hit::Editor(id, _) if *id == limit)).is_some());
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(view.session.result.row_ids.to_vec(), [0, 1, 2]);
        assert_eq!(view.session.result.fetch_rows(0, 3).unwrap()[2].values[0], Some("2".into()));
        assert!(view.session.source_path.is_none());
    });
    viewer.update(|view, cx| {
        view.session
            .set_query_config(
                QueryConfig {
                    group_by: vec!["category".into()],
                    aggregations: vec![Aggregation::sql_expression("count(*)".into(), Some("Rows".into()))],
                    ..Default::default()
                },
                "sales".into(),
            )
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 2);
        assert_eq!(view.session.result.fetch_rows(0, 2).unwrap()[0].values[1], Some("50".into()));
    });
    viewer.update(|view, cx| {
        view.session
            .set_query_config(QueryConfig::default(), "sales".into())
            .unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    let saved = viewer.update(|view, _| {
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
        view.session
            .save_as("Saved database view".into(), &view.repository)
            .unwrap();
        view.session.working.clone().unwrap()
    });
    assert_eq!(saved.database_view, Some(DatabaseView::view("main", "sales")));
    assert_eq!(repository.load_views().0, [saved.clone()]);
    let path = home.path().join("file.csv");
    std::fs::write(&path, "n\n1\n2\n3\n4\n").unwrap();
    open(&viewer, FileEntry::Source(path.clone()));
    assert!(viewer.read(|view| view.is_table()));
    travel(&viewer, false);
    viewer.read(|view| {
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(view.session.working.as_ref().unwrap().database_view, saved.database_view);
    });
    open(&viewer, FileEntry::View(saved.clone()));
    viewer.update(|view, cx| {
        view.query_limit = 5;
        view.reload(false, cx);
    });
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 5);
    viewer.update(|view, cx| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("CREATE OR REPLACE VIEW main.sales AS SELECT 999 AS n, 1 AS category")
            .unwrap();
        view.database.invalidate();
        view.check_source(cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.session.result.fetch_rows(0, 1).unwrap()[0].values[0], Some("999".into()));
    });
    viewer.update(|view, cx| view.archive_view(&saved, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(!view.is_table());
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.database.views().unwrap(), [saved.database_view.clone().unwrap()]);
    });
    open(&viewer, FileEntry::Source(path));
    let previous = viewer.read(|view| view.session.navigation_entry().unwrap());
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("DROP VIEW main.sales")
            .unwrap()
    });
    open(&viewer, FileEntry::View(saved));
    viewer.read(|view| {
        assert_eq!(view.session.navigation_entry().unwrap(), previous);
        assert!(view.error.as_ref().unwrap().contains("no longer exists"));
    });
}

#[test]
fn favorites_reopen_saved_tables_queries_and_sql_by_id() {
    use crate::infrastructure::json_store::FavoriteTarget;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let viewer = viewer(repository.clone());
    for kind in [
        ViewKind::Table(Default::default()),
        ViewKind::Query(QueryConfig {
            group_by: vec![Dimension::from("region")],
            ..Default::default()
        }),
        ViewKind::Sql(crate::domain::documents::SqlConfig {
            text: "SELECT 123".into(),
            ..Default::default()
        }),
    ] {
        let mut saved = DocumentDefinition::draft_table("Favorite view".into(), path.to_string_lossy().into());
        saved.view = kind;
        repository.save_view(&mut saved).unwrap();
        open(&viewer, FileEntry::View(saved.clone()));
        viewer.update(|view, cx| view.favorite(0, true, cx));
        assert_eq!(
            repository.load_settings().unwrap().favorites[0],
            Some(FavoriteTarget::View {
                view_id: saved.id.clone()
            })
        );
        saved.name = "Renamed favorite".into();
        repository.save_view(&mut saved).unwrap();
        open(&viewer, FileEntry::Source(path.clone()));
        viewer.update(|view, cx| view.favorite(0, false, cx));
        viewer.run_until_parked();
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let reopened = view.session.working.as_ref().unwrap();
            assert_eq!(reopened.id, saved.id);
            assert_eq!(reopened.name, saved.name);
            assert_eq!(std::mem::discriminant(&reopened.view), std::mem::discriminant(&saved.view));
            if !matches!(saved.view, ViewKind::Table(_)) {
                assert_eq!(reopened.view, saved.view);
            }
            if view.session.is_sql() {
                assert_eq!(view.session.result.row_count(), 0);
            }
        });
    }
    let saved = viewer.read(|view| view.session.working.clone().unwrap());
    repository.archive(&saved.id).unwrap();
    viewer.update(|view, cx| view.favorite(0, false, cx));
    assert!(viewer.read(|view| view.error.as_ref().unwrap().contains("no longer saved")));
}

#[test]
fn history_restores_unsaved_definition_search_and_layout_without_saving() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nNorth,30\nSouth,20\n").unwrap();
    let mut saved = DocumentDefinition::draft_table("Saved".into(), path.to_string_lossy().into());
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::View(saved.clone()));
    viewer.update(|view, cx| {
        view.session.rename_working("Unsaved name".into()).unwrap();
        view.session
            .edit_definition(|definition| {
                definition.slicers = vec![SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "region".into(),
                    selected: vec![ScalarKey::String("North".into())],
                }];
                let ViewKind::Table(table) = &mut definition.view else {
                    unreachable!()
                };
                table.column_order = vec!["n".into(), "region".into()];
                table.frozen_count = 1;
            })
            .unwrap();
        view.session.set_column_width("n".into(), 123.);
        view.session.toggle_sort("n".into());
        view.set_search_text("North".into(), cx);
    });
    viewer.run_until_parked();
    let expected = viewer.read(|view| view.session.navigation_entry().unwrap());
    open(&viewer, FileEntry::Source(path.clone()));
    travel(&viewer, false);
    viewer.read(|view| {
        assert_eq!(view.session.navigation_entry().unwrap(), expected);
        assert_eq!(view.search_input.text(), "North");
        assert_eq!(view.session.result.row_count(), 2);
        assert_eq!(view.session.result.columns[0].0, "n");
        assert_eq!(view.grid.column_width("n"), 123.);
        assert_eq!(view.grid.selection().unwrap().active.row, 0);
        assert!(!view.loading && !view.busy);
        assert!(view.navigation_backup.is_none());
        assert!(view.navigation_history.target(true).is_some());
        view.history_writer.flush();
    });
    assert_eq!(repository.load_views().0, vec![saved]);
    travel(&viewer, true);
    viewer.read(|view| {
        assert_eq!(view.session.identity, SessionIdentity::DraftTable);
        assert!(view.session.search_text.is_empty());
    });
}

#[test]
fn history_restores_direct_preferences_and_draft_pivot() {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nNorth,30\nSouth,20\n").unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::Source(path.clone()));
    viewer.update(|view, cx| {
        view.session.set_column_width("n".into(), 177.);
        let mut config = view.session.effective_table_config();
        config.column_order = vec!["n".into(), "region".into()];
        config.frozen_count = 1;
        view.session.set_table_layout(config).unwrap();
        view.autosave_table_preferences(cx);
        view.table_writer.flush().unwrap();
    });
    let direct = viewer.read(|view| view.session.navigation_entry().unwrap());
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        aggregations: vec![Aggregation {
            source_column: "n".into(),
            operation: AggregationOp::Sum,
            name: Some("total".into()),
            sql: None,
            conditions: Vec::new(),
            percentile: None,
            interpolation: None,
            ddof: None,
        }],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(pivot));
    viewer.update(|view, _| {
        view.session.identity = SessionIdentity::DraftQuery;
        view.session.rename_working("Draft pivot".into()).unwrap();
    });
    let query = viewer.read(|view| view.session.navigation_entry().unwrap());
    repository
        .update_table_preferences(
            &path,
            &TablePreferencesPatch {
                widths: [("n".into(), Some(299.))].into(),
                ..Default::default()
            },
        )
        .unwrap();
    travel(&viewer, false);
    viewer.read(|view| {
        assert_eq!(view.session.navigation_entry().unwrap(), direct);
        assert_eq!(view.grid.column_width("n"), 177.);
        assert!(view.session.is_direct_table());
    });
    travel(&viewer, true);
    viewer.read(|view| {
        assert_eq!(view.session.navigation_entry().unwrap(), query);
        assert_eq!(view.session.result.row_count(), 2);
    });
}

#[test]
fn history_missing_source_and_failed_recomputation_preserve_cursor() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    let b = home.path().join("b.csv");
    std::fs::write(&a, "region,n\nNorth,10\nSouth,20\n").unwrap();
    std::fs::write(&b, "region,n\nEast,40\n").unwrap();
    let mut definition = DocumentDefinition::draft_table("A".into(), a.to_string_lossy().into());
    definition.slicers = vec![SlicerDefinition {
        sql: false,
        condition: None,
        column: "region".into(),
        selected: vec![ScalarKey::String("North".into())],
    }];
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::View(definition));
    open(&viewer, FileEntry::Source(b));
    let (current, target, result) = viewer.read(|view| {
        (
            view.session.navigation_entry(),
            view.navigation_history.target(false),
            view.session.result.clone(),
        )
    });
    std::fs::remove_file(&a).unwrap();
    travel(&viewer, false);
    viewer.read(|view| {
        assert!(view.error.is_some());
        assert_eq!(view.session.navigation_entry(), current);
        assert_eq!(view.navigation_history.target(false), target);
        assert!(view.navigation_history.target(true).is_none());
    });
    std::fs::write(&a, "different\nchanged schema\n").unwrap();
    travel(&viewer, false);
    viewer.read(|view| {
        assert!(view.error.as_ref().unwrap().contains("region"));
        assert_eq!(view.session.navigation_entry(), current);
        assert_eq!(view.navigation_history.target(false), target);
        assert!(std::sync::Arc::ptr_eq(&view.session.result.store, &result.store));
        assert!(std::sync::Arc::ptr_eq(&view.grid.result().store, &result.store));
        assert!(!view.loading && !view.busy);
        assert!(view.navigation_backup.is_none());
    });
}

#[test]
fn history_branches_and_rejects_superseded_loads() {
    let home = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|name| {
            let path = home.path().join(format!("{name}.csv"));
            std::fs::write(&path, "n\n1\n").unwrap();
            path
        })
        .collect();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(paths[0].clone()));
    open(&viewer, FileEntry::Source(paths[1].clone()));
    viewer.update(|view, cx| {
        view.navigate_history(false, cx);
        // There is no forward target until Back commits. This must be a no-op.
        view.navigate_history(true, cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.source_path.as_deref(), Some(paths[0].to_str().unwrap()));
        assert!(view.navigation_history.target(false).is_none());
        assert!(view.navigation_history.target(true).is_some());
    });
    travel(&viewer, true);
    viewer.update(|view, cx| {
        view.navigate_history(false, cx);
        view.navigate_to_file(FileEntry::Source(paths[2].clone()), false, false, false, cx);
    });
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.session.source_path.as_deref(), Some(paths[2].to_str().unwrap()));
        assert_eq!(
            view.navigation_history
                .target(false)
                .unwrap()
                .1
                .source_path
                .as_deref(),
            Some(paths[1].to_str().unwrap())
        );
    });
    travel(&viewer, false);
    open(&viewer, FileEntry::Source(paths[0].clone()));
    viewer.read(|view| {
        assert!(view.navigation_history.target(true).is_none());
        assert_eq!(
            view.navigation_history
                .target(false)
                .unwrap()
                .1
                .source_path
                .as_deref(),
            Some(paths[1].to_str().unwrap())
        );
    });
}

#[test]
fn history_sql_restores_text_and_search_without_results_or_execution() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("data.csv");
    std::fs::write(&path, "n\n1\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path.clone()));
    viewer.update(|view, cx| view.new_sql(false, cx));
    viewer.run_until_parked();
    let text = "CREATE TABLE history_marker AS SELECT 42 AS n; SELECT n FROM history_marker;";
    viewer.update(|view, cx| {
        view.sql_panel.as_mut().unwrap().editor.set_text(text);
        view.session.set_sql_text(text.into());
        view.run_sql(true, cx);
    });
    viewer.run_until_parked();
    viewer.update(|view, cx| {
        assert_eq!(view.session.result.row_count(), 1);
        view.set_search_text("42".into(), cx);
        view.session.sort = vec![SortKey {
            column: "n".into(),
            descending: true,
        }];
        if let Some(definition) = &mut view.session.working {
            let ViewKind::Sql(sql) = &mut definition.view else {
                unreachable!()
            };
            sql.result_sort = view.session.sort.clone();
        }
    });
    viewer.run_until_parked();
    let expected = viewer.read(|view| view.session.navigation_entry().unwrap());
    open(&viewer, FileEntry::Source(path));
    travel(&viewer, false);
    viewer.read(|view| {
        assert_eq!(view.session.navigation_entry().unwrap(), expected);
        assert_eq!(view.session.identity, SessionIdentity::DraftSql);
        assert_eq!(view.session.result.row_count(), 0);
        assert_eq!(view.session.source.row_count(), 0);
        assert!(!view.sql_running && !view.busy && !view.loading);
        assert!(view.sql_panel.as_ref().unwrap().error.is_none());
        assert!(view.query_cache.is_none() && view.table_query.is_none());
        assert!(view.navigation_backup.is_none());
        assert!(view.navigation_history.target(true).is_some());
    });
}

#[test]
fn history_return_reloads_changed_source_data() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    let b = home.path().join("b.csv");
    std::fs::write(&a, "n\n1\n").unwrap();
    std::fs::write(&b, "n\n9\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(a.clone()));
    open(&viewer, FileEntry::Source(b));
    std::fs::write(&a, "n\n1\n2\n3\n").unwrap();
    travel(&viewer, false);
    viewer.read(|view| {
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(view.session.source.row_count(), 3);
        assert!(view.navigation_backup.is_none());
    });
}

#[test]
fn history_keys_only_navigate_from_grid_and_commands_are_available() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    let b = home.path().join("b.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    std::fs::write(&b, "word\nur\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(a.clone()));
    open(&viewer, FileEntry::Source(b.clone()));
    let source = |viewer: &Viewer| viewer.read(|view| view.session.source_path.clone());
    viewer.focus_search();
    viewer.render();
    viewer.input("ur");
    assert_eq!(source(&viewer).as_deref(), Some(b.to_str().unwrap()));
    assert_eq!(viewer.read(|view| view.search_input.text().to_owned()), "ur");
    viewer.run_until_parked();
    viewer.focus_grid();
    press_each(&viewer, &["u"]);
    assert_eq!(source(&viewer).as_deref(), Some(a.to_str().unwrap()));
    press_each(&viewer, &["r"]);
    assert_eq!(source(&viewer).as_deref(), Some(b.to_str().unwrap()));
    assert_eq!(viewer.read(|view| view.session.search_text.clone()), "ur");
    viewer.update(|view, cx| view.open_command_palette(cx));
    viewer.render();
    viewer.input("Back");
    press_each(&viewer, &["enter"]);
    assert_eq!(source(&viewer).as_deref(), Some(a.to_str().unwrap()));
    viewer.update(|view, cx| view.dispatch_command(CommandId::Forward, cx));
    viewer.run_until_parked();
    assert_eq!(source(&viewer).as_deref(), Some(b.to_str().unwrap()));
}

fn column_names(viewer: &Viewer) -> Vec<String> {
    viewer.read(|view| {
        let result = &view.session.result;
        result
            .column_indices
            .iter()
            .map(|&index| result.columns[index].0.clone())
            .collect()
    })
}

#[test]
fn ctrl_v_in_the_grid_opens_pasted_text_as_a_table_and_back_returns() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(a.clone()));
    set_clipboard("city\tsales\r\nOslo\t10\r\n\"Rome, IT\"\t20\r\n");
    viewer.focus_grid();
    press_each(&viewer, &["ctrl-v"]);
    let pasted = viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 2);
        assert!(
            view.copy_status
                .as_deref()
                .is_some_and(|status| status.starts_with("Pasted table saved to"))
        );
        std::path::PathBuf::from(view.session.source_path.clone().unwrap())
    });
    let pasted_folder = crate::infrastructure::source_database::absolute_source_path(&home.path().join("pasted"));
    assert_eq!(pasted.parent().unwrap(), pasted_folder);
    assert_eq!(pasted.extension().unwrap(), "tsv");
    assert_eq!(column_names(&viewer), ["city", "sales"]);
    let rows = viewer.read(|view| {
        view.session
            .result
            .fetch_rows(0, 2)
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        rows,
        [
            vec![Some("Oslo".to_string()), Some("10".to_string())],
            vec![Some("Rome, IT".to_string()), Some("20".to_string())],
        ]
    );
    travel(&viewer, false);
    assert_eq!(
        viewer.read(|view| view.session.source_path.clone()).as_deref(),
        Some(a.to_str().unwrap())
    );
}

#[test]
fn pasting_without_text_reports_an_error_and_keeps_the_table() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(a.clone()));
    set_clipboard("  \r\n");
    viewer.update(|view, cx| view.dispatch_command(CommandId::PasteTable, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert_eq!(view.error.as_deref(), Some("The clipboard has no text to open as a table"));
        assert_eq!(view.session.source_path.as_deref(), Some(a.to_str().unwrap()));
    });
    assert!(!home.path().join("pasted").exists());
}

#[test]
fn dropped_files_open_as_tables_without_writing_to_the_database() {
    use crate::application::state::ApplicationState;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let a = home.path().join("a.csv");
    let b = home.path().join("b.tsv");
    std::fs::write(&a, "word\nur\n").unwrap();
    std::fs::write(&b, "x\ty\n1\t2\n").unwrap();
    let viewer = viewer(repository.clone());
    let windows = viewer.app.windows().len();
    viewer.drop_files(vec![a.clone(), b.clone()]);
    let database = viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        // Dropped files open as tables, read into memory by Polars.
        assert_eq!(view.session.source_path.as_deref(), Some(a.to_str().unwrap()));
        assert_eq!(view.query_file(), None);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.session.source.store.lock().unwrap().engine(),
            crate::infrastructure::store::Engine::Polars
        );
        view.database.clone()
    });
    assert_eq!(viewer.app.windows().len(), windows + 1);
    assert!(
        !database
            .relations()
            .unwrap()
            .iter()
            .any(|relation| relation.name.contains("csv"))
    );
    crate::application::history::HistoryWriter::flush_profiles();
    let mut state = ApplicationState::load(repository.clone()).unwrap();
    assert!(
        state
            .history
            .recent_sources
            .iter()
            .any(|path| path.ends_with("a.csv"))
    );

    // A watched file opens the same way.
    state.add_watched_files(std::slice::from_ref(&a)).unwrap();
    viewer.drop_files(vec![a.clone()]);
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.source_path.as_deref(), Some(a.to_str().unwrap()));
        assert!(view.is_table());
    });
}

#[test]
fn n_opens_an_overlay_that_watches_dropped_files() {
    use crate::application::state::ApplicationState;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    let folder = home.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let viewer = viewer(repository.clone());
    let press = |key: &str| {
        viewer.focus_grid();
        press_each(&viewer, &[key]);
    };
    let open_overlay = || {
        viewer.update(|view, cx| view.dispatch_command(CommandId::CacheFiles, cx));
        assert!(viewer.read(|view| view.cache_drop_open));
    };

    // Folders are rejected; only files can be added.
    open_overlay();
    viewer.drop_files(vec![folder.clone()]);
    viewer.read(|view| {
        assert!(!view.cache_drop_open);
        assert!(view.error.as_deref().unwrap().contains("is a folder"));
    });

    open_overlay();
    viewer.drop_files(vec![a.clone()]);
    let database = viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.copy_status.as_deref(), Some("Watching a.csv"));
        // The overlay watches the file without opening or reading it.
        assert_eq!(view.session.source_path, None);
        view.database.clone()
    });
    assert!(ApplicationState::load(repository).unwrap().is_watched(&a));
    assert_eq!(database.loaded_table(&a), None);

    // N opens the overlay from the grid and Escape closes it.
    press("n");
    assert!(viewer.read(|view| view.cache_drop_open));
    press("escape");
    assert!(!viewer.read(|view| view.cache_drop_open));
}

#[test]
fn dropped_databases_attach_read_only_and_show_in_the_tree() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    // A DuckDB file with a `.db` extension is recognized by its header.
    let shop = home.path().join("shop.db");
    duckdb::Connection::open(&shop)
        .unwrap()
        .execute_batch("CREATE TABLE orders AS SELECT 1 AS id")
        .unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(a.clone()));
    let windows = viewer.app.windows().len();
    viewer.drop_files(vec![shop.clone()]);
    assert_eq!(viewer.app.windows().len(), windows);
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.source_path.as_deref(), Some(a.to_str().unwrap()));
        let status = view.copy_status.as_deref().unwrap();
        assert_eq!(status, "Attached shop read-only; to write, DETACH shop and ATTACH it again in SQL");
        assert!(view.database_tree_open);
        assert_eq!(view.database_tree.selected_id().as_deref(), Some("db:shop"));
        assert!(
            view.database_tree
                .relation_labels()
                .contains(&"shop.main.orders".to_string())
        );
        let written = view
            .database
            .connect()
            .unwrap()
            .execute_batch("INSERT INTO shop.orders VALUES (2)");
        assert!(written.is_err());
    });
    // Selecting cells replaces the message with the selection's statistics.
    viewer.update(|view, cx| view.focus_grid(cx));
    press_each(&viewer, &["down"]);
    assert!(viewer.read(|view| view.copy_status.is_none()));
}

#[test]
fn database_tree_toggles_filters_by_typing_and_opens_sources_and_metadata() {
    use crate::domain::documents::RelationKind;
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("data");
    std::fs::create_dir_all(folder.join("2024")).unwrap();
    std::fs::write(folder.join("2024").join("q1.csv"), "region,n\nNorth,1\nSouth,2\n").unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.watched_folders = vec![folder.to_string_lossy().into_owned()];
    repository.save_settings(&settings).unwrap();
    let viewer = viewer(repository.clone());
    press_each(&viewer, &["ctrl-b"]);
    viewer.read(|view| {
        assert!(view.database_tree_open);
        assert!(view.focus.is_some_and(|id| view.database_tree.contains(id)));
    });
    assert!(repository.load_settings().unwrap().database_tree_open);
    // Typing in the tree starts the filter; Enter opens the first match here.
    press_each(&viewer, &["q", "1", "enter"]);
    viewer.read(|view| {
        assert!(
            view.session
                .source_path
                .as_deref()
                .is_some_and(|path| path.ends_with("q1.csv")),
            "{:?}",
            view.session.source_path
        );
        assert_eq!(view.session.result.row_count(), 2);
    });
    assert!(viewer.grid_focused());
    // A metadata function opens as a read-only database relation. Typing in
    // the tree replaces the old filter and moves to the box; Enter there opens.
    viewer.update(|view, _| {
        let mut focus = view.focus;
        view.database_tree.focus(&mut focus);
        view.focus = focus;
    });
    press_each(&viewer, &["d"]);
    viewer.input("uckdb_settings");
    viewer.run_until_parked();
    press_each(&viewer, &["enter"]);
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let relation = view
            .session
            .working
            .as_ref()
            .and_then(|definition| definition.database_view.clone())
            .unwrap_or_else(|| {
                panic!(
                    "no relation: {:?} {:?} selected={:?}",
                    view.session.working.as_ref().map(|definition| &definition.name),
                    view.session.source_path,
                    view.database_tree.selected_id()
                )
            });
        assert_eq!(relation.kind, RelationKind::Function);
        assert_eq!(relation.name, "duckdb_settings");
        assert!(view.session.result.row_count() > 0);
    });
    // Ctrl+B from the grid hides the tree and remembers that.
    press_each(&viewer, &["ctrl-b"]);
    assert!(!viewer.read(|view| view.database_tree_open));
    assert!(!repository.load_settings().unwrap().database_tree_open);
}

#[test]
fn delete_in_the_database_tree_removes_what_each_item_stands_for() {
    use crate::application::state::ApplicationState;
    use crate::infrastructure::source_database::SourceDatabase;
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("data");
    std::fs::create_dir_all(folder.join("2024")).unwrap();
    let q1 = folder.join("2024").join("q1.csv");
    std::fs::write(&q1, "n\n1\n").unwrap();
    let single = home.path().join("single.csv");
    std::fs::write(&single, "n\n1\n").unwrap();
    let recent = home.path().join("recent.csv");
    std::fs::write(&recent, "n\n1\n").unwrap();
    let repository = repository(home.path());
    let mut state = ApplicationState::load(repository.clone()).unwrap();
    let mut settings = state.settings.clone();
    settings.watched_folders = vec![folder.to_string_lossy().into_owned()];
    repository.save_settings(&settings).unwrap();
    state.settings = settings;
    state.add_watched_files(std::slice::from_ref(&single)).unwrap();
    state.record_source(&recent).unwrap();
    let mut saved = DocumentDefinition::draft_table("Old view".into(), recent.to_string_lossy().into());
    repository.save_view(&mut saved).unwrap();
    let viewer = viewer(repository.clone());
    let database = viewer.read(|view| view.database.clone());
    database
        .connect()
        .unwrap()
        .execute_batch("CREATE TABLE main.notes AS SELECT 1 AS n")
        .unwrap();
    let tables = |database: &std::sync::Arc<SourceDatabase>| {
        database
            .relations()
            .unwrap()
            .into_iter()
            .map(|relation| relation.label())
            .collect::<Vec<_>>()
    };
    press_each(&viewer, &["ctrl-b"]);
    let delete = |id: &str| {
        viewer.update(|view, _| {
            view.database_tree.select(id);
            let mut focus = view.focus;
            view.database_tree.focus(&mut focus);
            view.focus = focus;
        });
        press_each(&viewer, &["delete"]);
    };
    let confirm = || {
        assert!(viewer.has_dialog());
        viewer.click(|hit| *hit == Hit::Button(Button::DialogOk));
        viewer.run_until_parked();
    };
    let status = || {
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            view.copy_status.clone().unwrap_or_default()
        })
    };

    // Groups, schema folders and metadata functions have nothing to delete.
    delete("group:sources");
    delete("db:profile.main");
    assert!(!viewer.has_dialog());

    // The profile database explains why it stays.
    delete("db:profile");
    assert!(status().contains("cannot be detached"));

    // A recent file is forgotten.
    let recorded = state.history.recent_sources[0].clone();
    delete(&format!("recent:{recorded}"));
    let state = ApplicationState::load(repository.clone()).unwrap();
    assert!(
        !state
            .history
            .recent_sources
            .iter()
            .any(|path| path.ends_with("recent.csv"))
    );

    // A watched file stops being watched.
    delete(&format!("watched:{}", state.settings.watched_files[0]));
    status();
    assert!(!ApplicationState::load(repository.clone()).unwrap().is_watched(&single));

    // A file in a watched folder asks before removing the whole folder.
    delete(&format!("src:{}", q1.display()));
    assert!(ApplicationState::load(repository.clone()).unwrap().is_watched(&q1));
    confirm();
    status();
    let state = ApplicationState::load(repository.clone()).unwrap();
    assert!(state.settings.watched_folders.is_empty());

    // A table of the profile database is dropped once confirmed.
    delete("db:main.notes");
    assert!(tables(&database).contains(&"main.notes".to_owned()));
    confirm();
    status();
    assert!(!tables(&database).contains(&"main.notes".to_owned()));

    // A saved view is archived.
    delete(&format!("view:{}", saved.id));
    assert!(status().starts_with("Archived Old view"));
    assert!(repository.load_views().0.is_empty());
}

#[test]
fn database_tree_refresh_button_lists_new_tables() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()));
    let labels = || viewer.read(|view| view.database_tree.relation_labels());
    press_each(&viewer, &["ctrl-b"]);
    assert!(!labels().contains(&"main.added_later".to_owned()));
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("CREATE TABLE main.added_later AS SELECT 1 AS n")
            .unwrap();
    });
    viewer.click(|hit| *hit == Hit::Button(Button::TreeRefresh));
    viewer.run_until_parked();
    assert!(labels().contains(&"main.added_later".to_owned()));
}

#[test]
fn database_tree_reads_again_only_when_what_it_lists_may_have_changed() {
    use crate::application::state::ApplicationState;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let folder = home.path().join("watched");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("nested/first.csv"), "n\n1\n").unwrap();
    ApplicationState::load(repository.clone())
        .unwrap()
        .add_watched_folder(&folder)
        .unwrap();
    let viewer = viewer(repository);
    press_each(&viewer, &["ctrl-b"]);
    let check = || {
        viewer.update(|view, cx| view.check_database_tree(cx));
        viewer.run_until_parked();
        viewer.read(|view| view.database_tree.refreshes())
    };
    let files = || viewer.read(|view| view.database_tree.folder_files());
    // Opening the tree saved a setting, which may cost one more read.
    let settled = check();
    assert_eq!(check(), settled);

    // Saving a listed file in place lists nothing new.
    std::fs::write(folder.join("nested/first.csv"), "n\n1\n2\n").unwrap();
    assert_eq!(check(), settled);

    // A new file in a subfolder is listed.
    std::fs::write(folder.join("nested/second.csv"), "n\n1\n").unwrap();
    assert_eq!(check(), settled + 1);
    assert!(files().contains(&std::path::PathBuf::from("nested").join("second.csv")));
    assert_eq!(check(), settled + 1);

    // So is a change to the database.
    viewer.read(|view| view.database.invalidate());
    assert_eq!(check(), settled + 2);

    // The monitor checks only while the window has the keyboard.
    std::fs::write(folder.join("third.csv"), "n\n1\n").unwrap();
    viewer.update(|view, _| view.window_active = false);
    viewer.advance_clock(std::time::Duration::from_secs(1));
    assert_eq!(viewer.read(|view| view.database_tree.refreshes()), settled + 2);
    viewer.update(|view, _| view.window_active = true);
    viewer.advance_clock(std::time::Duration::from_secs(1));
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.database_tree.refreshes()), settled + 3);
    assert!(files().contains(&std::path::PathBuf::from("third.csv")));
}

#[test]
fn source_checks_skip_unchanged_files_but_still_see_edits() {
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()));
    let path = home.path().join("watched.csv");
    std::fs::write(&path, "n\n1\n2\n").unwrap();
    open(&viewer, FileEntry::Source(path.clone()));
    let check = || {
        viewer.update(|view, cx| view.check_source(cx));
        viewer.run_until_parked();
    };
    let rows = || {
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            view.session.result.row_count()
        })
    };
    check();
    assert!(viewer.read(|view| view.source_probe.is_some()));
    check();
    assert_eq!(rows(), 2);

    std::fs::write(&path, "n\n1\n2\n3\n").unwrap();
    check();
    assert_eq!(rows(), 3);
    check();
    assert!(viewer.read(|view| view.source_probe.is_some()));

    // A database change forces the full check, which still reuses the read.
    let store = viewer.read(|view| view.session.source.store.clone());
    viewer.read(|view| view.database.invalidate());
    check();
    assert_eq!(rows(), 3);
    assert!(viewer.read(|view| std::sync::Arc::ptr_eq(&view.session.source.store, &store)));
}

#[test]
fn read_mode_scans_a_file_in_place_and_reads_it_into_memory_again() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,n
North,1
South,2
North,3
").unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::Source(path.clone()));
    let scanned = |viewer: &Viewer| viewer.read(|view| view.session.source.store.lock().unwrap().is_database_view());
    assert!(!scanned(&viewer));
    viewer.view_option(super::view_options::ViewOption::ReadMode);
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.dialog.is_none());
        assert_eq!(view.query_file().as_deref(), Some(path.to_string_lossy().as_ref()));
        assert_eq!(view.session.result.row_count(), 3);
    });
    assert!(scanned(&viewer));
    // The choice is the file's, so opening it again scans it.
    viewer.update(|view, _| view.table_writer.flush().unwrap());
    assert!(repository.load_table_preferences(&path).unwrap().file.scan);
    open(&viewer, FileEntry::Source(path.clone()));
    assert!(scanned(&viewer));
    // The command switches back, and the file is read into memory.
    viewer.update(|view, cx| view.dispatch_command(CommandId::ToggleScan, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.query_file().is_none());
        assert_eq!(view.session.result.row_count(), 3);
    });
    assert!(!scanned(&viewer));
    viewer.update(|view, _| view.table_writer.flush().unwrap());
    assert!(!repository.load_table_preferences(&path).unwrap().file.scan);
}

#[test]
fn a_file_read_into_memory_stops_at_the_limit_in_either_engine() {
    use crate::infrastructure::store::Engine;
    for engine in Engine::ALL {
        // A profile each, since a window takes the limit its profile last set.
        let home = tempfile::tempdir().unwrap();
        let repository = repository(home.path());
        let path = home.path().join("years.csv");
        std::fs::write(&path, "year,n
2000,1
2004,2
2008,3
").unwrap();
        repository
            .save_settings(&crate::infrastructure::json_store::Settings {
                default_engine: engine,
                query_limit: 2,
                ..Default::default()
            })
            .unwrap();
        let viewer = viewer(repository.clone());
        open(&viewer, FileEntry::Source(path.clone()));
        viewer.render();
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(view.is_table());
            assert_eq!(view.session.source.row_count(), 2, "{engine:?}");
            assert_eq!(view.session.source.cut_at, Some(2));
            // The limit shows in table mode, red, since it cut the file.
            assert!(view.limit_applies());
            assert!(view.limit_reached());
        });
        // Exporting asks first, since the rest of the file was never read.
        viewer.update(|view, cx| view.export_result(cx));
        viewer.read(|view| match &view.dialog {
            Some(super::dialogs::Dialog::Confirm(confirm)) => {
                let (title, message, _) = confirm.text();
                assert!(title.contains("rows that were read"), "{title}");
                assert!(message.contains("first 2 rows"), "{message}");
            }
            _ => panic!("expected the export to ask first"),
        });
        viewer.update(|view, _| view.dialog = None);
        // Raising the limit reads the file again, whole this time.
        let limit = viewer.read(|view| view.limit_input.id);
        viewer.focus(limit);
        viewer.press("ctrl-a 5 enter");
        viewer.run_until_parked();
        viewer.render();
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.query_limit, 5);
            assert_eq!(view.session.source.row_count(), 3, "{engine:?}");
            assert_eq!(view.session.source.cut_at, None);
            assert!(view.limit_applies());
            assert!(!view.limit_reached());
        });
        // A file scanned in place is not read into memory, so in table mode
        // the limit does not apply to it.
        viewer.update(|view, cx| view.load_database_view(crate::domain::documents::DatabaseView::file(path.to_string_lossy()), true, cx));
        viewer.run_until_parked();
        assert!(!viewer.read(|view| view.is_table() && view.limit_applies()));
    }
}

fn engine(viewer: &Viewer) -> crate::infrastructure::store::Engine {
    viewer.read(|view| view.session.source.store.lock().unwrap().engine())
}

#[test]
fn the_engine_button_switches_a_file_between_engines_and_keeps_the_view() {
    use crate::infrastructure::store::Engine;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,n\nNorth,1\nSouth,2\nNorth,3\n").unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::Source(path.clone()));
    assert_eq!(engine(&viewer), Engine::Polars);
    viewer.update(|view, cx| {
        view.search_input.set_text("north");
        view.schedule_search("north".into(), cx);
    });
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.row_count()), 2);
    // The gear's View options switch the engine in place.
    viewer.view_option(super::view_options::ViewOption::Engine);
    assert!(viewer.has_dialog());
    viewer.close_dialog();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.dialog.is_none());
        assert_eq!(view.session.search_text, "north");
        assert_eq!(view.session.result.row_count(), 2);
    });
    assert_eq!(engine(&viewer), Engine::DuckDb);
    // The choice is the file's, so opening it again keeps DuckDB.
    viewer.update(|view, _| view.table_writer.flush().unwrap());
    let preferences = repository.load_table_preferences(&path).unwrap();
    assert_eq!(preferences.file.engine, Some(Engine::DuckDb));
    open(&viewer, FileEntry::Source(path.clone()));
    assert_eq!(engine(&viewer), Engine::DuckDb);
    // Preserve order shows for DuckDB and reads the file again.
    viewer.view_option(super::view_options::ViewOption::PreserveOrder);
    viewer.close_dialog();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.file_settings.preserve_order);
        assert_eq!(view.session.result.row_count(), 3);
    });
    viewer.update(|view, cx| view.dispatch_command(CommandId::QueryEngine, cx));
    press_each(&viewer, &["enter"]);
    viewer.run_until_parked();
    assert_eq!(engine(&viewer), Engine::Polars);
    // Preserve order does not apply to Polars.
    viewer.update(|view, cx| view.dispatch_command(CommandId::TogglePreserveOrder, cx));
    assert!(viewer.read(|view| view.copy_status.as_deref().unwrap().contains("DuckDB")));
}

#[test]
fn settings_switch_the_default_engine_for_files_without_their_own() {
    use crate::infrastructure::store::Engine;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "n\n1\n").unwrap();
    let viewer = viewer(repository.clone());
    press_each(&viewer, &["ctrl-,"]);
    assert!(viewer.has_dialog());
    // The first setting is the default engine; Enter switches it in place.
    press_each(&viewer, &["enter"]);
    assert!(viewer.has_dialog());
    assert_eq!(repository.load_settings().unwrap().default_engine, Engine::DuckDb);
    press_each(&viewer, &["escape"]);
    open(&viewer, FileEntry::Source(path.clone()));
    assert_eq!(engine(&viewer), Engine::DuckDb);
}

#[test]
fn slicer_options_are_kept_per_file_without_reading_it_again() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region\nNorth\n").unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::Source(path.clone()));
    let store = viewer.read(|view| view.session.source.store.clone());
    viewer.update(|view, cx| view.dispatch_command(CommandId::ToggleCrossFilter, cx));
    viewer.update(|view, cx| view.dispatch_command(CommandId::ToggleAvailableFirst, cx));
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(!view.file_settings.slicers.cross_filter);
        assert!(view.file_settings.slicers.available_first);
        assert!(std::sync::Arc::ptr_eq(&view.session.source.store, &store));
    });
    viewer.update(|view, _| view.table_writer.flush().unwrap());
    let saved = repository.load_table_preferences(&path).unwrap().file.slicers;
    assert!(!saved.cross_filter && saved.available_first);
    // Opening the file again restores them.
    let other = Viewer::open_in(&viewer.app, empty_frame(), None, None, repository);
    open(&other, FileEntry::Source(path));
    assert!(!other.read(|view| view.file_settings.slicers.cross_filter));
}

#[test]
fn the_aggregation_editor_speaks_the_files_engine() {
    use crate::infrastructure::store::Engine;
    use crate::ui::dataframe_view::dialogs::Dialog;
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,n\nNorth,1\nSouth,2\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    let population_deviation = Aggregation {
        sql: None,
        source_column: "n".into(),
        operation: AggregationOp::StandardDeviation,
        name: None,
        conditions: Vec::new(),
        percentile: None,
        interpolation: None,
        ddof: Some(0),
    };
    viewer.update(|view, _| {
        view.session.start_query_draft("By region".into()).unwrap();
        view.session
            .edit_definition(|definition| {
                definition.view = ViewKind::Query(QueryConfig {
                    group_by: vec!["region".into()],
                    aggregations: vec![population_deviation.clone()],
                    ..Default::default()
                })
            })
            .unwrap();
    });
    let inspect = |viewer: &Viewer| {
        viewer.update(|view, cx| view.open_aggregation_panel(cx));
        viewer.run_until_parked();
        viewer.update(|view, _| {
            let Some(Dialog::Aggregation(panel)) = &mut view.dialog else {
                panic!("the aggregation editor is open")
            };
            panel.select(0);
            let functions = panel.functions.list.items().iter().map(|item| item.label.clone()).collect::<Vec<_>>();
            (functions, panel.sql.text().to_owned())
        })
    };
    let (functions, sql) = inspect(&viewer);
    assert_eq!(engine(&viewer), Engine::Polars);
    assert!(functions.iter().any(|name| name.starts_with("total")), "{functions:?}");
    assert!(!functions.iter().any(|name| name.starts_with("arg_max")), "{functions:?}");
    // Polars SQL has only the sample statistic, scaled to the population.
    assert!(sql.contains("stddev_samp") && sql.contains("sqrt"), "{sql}");
    press_each(&viewer, &["escape"]);
    viewer.update(|view, cx| view.set_file_engine(Engine::DuckDb, cx));
    viewer.run_until_parked();
    let (functions, sql) = inspect(&viewer);
    assert!(functions.iter().any(|name| name.starts_with("arg_max")), "{functions:?}");
    assert!(sql.starts_with("stddev_pop"), "{sql}");
}

#[test]
fn auto_save_keeps_saved_table_and_query_views_up_to_date() {
    use super::view_options::ViewOption;
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,units\nnorth,3\nsouth,5\n").unwrap();
    let mut table = DocumentDefinition::draft_table("Sales table".into(), path.to_string_lossy().into_owned());
    repository.save_view(&mut table).unwrap();
    let mut query = DocumentDefinition::draft_table("Sales query".into(), path.to_string_lossy().into_owned());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec!["region".into()],
        ..Default::default()
    });
    repository.save_view(&mut query).unwrap();
    let stored = |id: &str| repository.load_views().0.into_iter().find(|view| view.id == id).unwrap();
    let viewer = viewer(repository.clone());
    for saved in [table, query] {
        open(&viewer, FileEntry::View(saved.clone()));
        viewer.view_option(ViewOption::AutoSave);
        viewer.close_dialog();
        assert!(stored(&saved.id).auto_save);
        viewer.update(|view, cx| {
            view.session.set_sort_direction("region".into(), true);
            view.schedule_search(String::new(), cx);
        });
        viewer.run_until_parked();
        assert!(viewer.read(|view| view.session.dirty));
        viewer.advance_clock(std::time::Duration::from_millis(2000));
        let sort = match stored(&saved.id).view {
            ViewKind::Table(config) => config.sort,
            ViewKind::Query(config) => config.result_sort,
            ViewKind::Sql(_) => unreachable!(),
        };
        assert_eq!(sort, [SortKey { column: "region".into(), descending: true }]);
        assert!(!viewer.read(|view| view.session.dirty));
    }
}
