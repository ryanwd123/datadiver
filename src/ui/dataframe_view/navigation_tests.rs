use super::{DataFrameView, FileEntry};
use crate::application::commands::CommandId;
use crate::application::session::SessionIdentity;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, QueryConfig, ScalarKey,
    SlicerDefinition, SortKey, ViewKind,
};
use crate::domain::frame::FrameSnapshot;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, Focusable, TestAppContext, WindowHandle, px, size};

fn viewer(
    cx: &mut TestAppContext,
    repository: ViewRepository,
) -> (WindowHandle<Root>, Entity<DataFrameView>) {
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
    (handle, entity.unwrap())
}

fn open(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entity: &Entity<DataFrameView>,
    entry: FileEntry,
) {
    cx.update_window(handle.into(), |_, _, cx| {
        entity.update(cx, |view, cx| {
            view.navigate_to_file(entry, false, false, false, handle.into(), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
}

fn travel(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entity: &Entity<DataFrameView>,
    forward: bool,
) {
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.navigate_history(forward, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn database_views_use_query_mode_limits_full_aggregations_and_persist(cx: &mut TestAppContext) {
    use crate::domain::documents::DatabaseView;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 3;
    repository.save_settings(&settings).unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
    cx.update(|cx| {
        entity.read(cx).database.connect().unwrap().execute_batch(
        "CREATE VIEW main.sales AS SELECT range AS n, range % 2 AS category FROM range(100)"
    ).unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.open_file_palette(window, cx));
        window.render_frame(cx);
        window.input("main.sales", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(entity.read(cx).file_groups[4].len(), 1);
        assert!(!entity.read(cx).file_groups[4][0].is_saved_view());
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert!(window.try_find("query-limit").is_some());
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(view.session.result.row_ids.as_ref(), &[0, 1, 2]);
        assert_eq!(
            view.session.result.fetch_rows(0, 3).unwrap()[2].values[0],
            Some("2".into())
        );
        assert!(view.session.source_path.is_none());
    })
    .unwrap();
    cx.update_window(handle.into(), |_, _, cx| {
        entity.update(cx, |view, cx| {
            view.session
                .set_query_config(
                    QueryConfig {
                        group_by: vec!["category".into()],
                        aggregations: vec![Aggregation::sql_expression(
                            "count(*)".into(),
                            Some("Rows".into()),
                        )],
                        ..Default::default()
                    },
                    "sales".into(),
                )
                .unwrap();
            view.schedule_search(String::new(), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 2);
        assert_eq!(
            view.session.result.fetch_rows(0, 2).unwrap()[0].values[1],
            Some("50".into())
        );
    });
    cx.update_window(handle.into(), |_, _, cx| {
        entity.update(cx, |view, cx| {
            view.session
                .set_query_config(QueryConfig::default(), "sales".into())
                .unwrap();
            view.schedule_search(String::new(), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let saved = cx.update(|cx| {
        entity.update(cx, |view, _| {
            assert!(!view.is_table());
            assert_eq!(view.session.result.row_count(), 3);
            view.session
                .save_as("Saved database view".into(), &view.repository)
                .unwrap();
            view.session.working.clone().unwrap()
        })
    });
    assert_eq!(
        saved.database_view,
        Some(DatabaseView::view("main", "sales"))
    );
    assert_eq!(repository.load_views().0, [saved.clone()]);
    let path = home.path().join("file.csv");
    std::fs::write(&path, "n\n1\n2\n3\n4\n").unwrap();
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    cx.update(|cx| assert!(entity.read(cx).is_table()));
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(
            view.session.working.as_ref().unwrap().database_view,
            saved.database_view
        );
    });
    open(cx, handle, &entity, FileEntry::View(saved.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.query_limit = 5;
            view.reload(false, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(entity.read(cx).session.result.row_count(), 5));
    cx.update_window(handle.into(), |_, _, cx| {
        entity.update(cx, |view, cx| {
            view.database
                .connect()
                .unwrap()
                .execute_batch(
                    "CREATE OR REPLACE VIEW main.sales AS SELECT 999 AS n, 1 AS category",
                )
                .unwrap();
            view.database.invalidate();
            view.check_source(cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.session.result.fetch_rows(0, 1).unwrap()[0].values[0],
            Some("999".into())
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.archive_view(&saved, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(!view.is_table());
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.database.views().unwrap(),
            [saved.database_view.clone().unwrap()]
        );
    });
    open(cx, handle, &entity, FileEntry::Source(path));
    let previous = cx.update(|cx| entity.read(cx).session.navigation_entry().unwrap());
    cx.update(|cx| {
        entity
            .read(cx)
            .database
            .connect()
            .unwrap()
            .execute_batch("DROP VIEW main.sales")
            .unwrap()
    });
    open(cx, handle, &entity, FileEntry::View(saved));
    cx.update(|cx| {
        assert_eq!(
            entity.read(cx).session.navigation_entry().unwrap(),
            previous
        );
        assert!(
            entity
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("no longer exists")
        );
    });
}

#[gpui_kit::test]
fn favorites_reopen_saved_tables_queries_and_sql_by_id(cx: &mut TestAppContext) {
    use crate::infrastructure::json_store::FavoriteTarget;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
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
        let mut saved =
            DocumentDefinition::draft_table("Favorite view".into(), path.to_string_lossy().into());
        saved.view = kind;
        repository.save_view(&mut saved).unwrap();
        open(cx, handle, &entity, FileEntry::View(saved.clone()));
        cx.update_window(handle.into(), |_, window, cx| {
            entity.update(cx, |view, cx| view.favorite(0, true, window, cx));
        })
        .unwrap();
        assert_eq!(
            repository.load_settings().unwrap().favorites[0],
            Some(FavoriteTarget::View {
                view_id: saved.id.clone()
            })
        );
        saved.name = "Renamed favorite".into();
        repository.save_view(&mut saved).unwrap();
        open(cx, handle, &entity, FileEntry::Source(path.clone()));
        cx.update_window(handle.into(), |_, window, cx| {
            entity.update(cx, |view, cx| view.favorite(0, false, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            let reopened = view.session.working.as_ref().unwrap();
            assert_eq!(reopened.id, saved.id);
            assert_eq!(reopened.name, saved.name);
            assert_eq!(
                std::mem::discriminant(&reopened.view),
                std::mem::discriminant(&saved.view)
            );
            if !matches!(saved.view, ViewKind::Table(_)) {
                assert_eq!(reopened.view, saved.view);
            }
            if view.session.is_sql() {
                assert_eq!(view.session.result.row_count(), 0);
            }
        });
    }
    let saved = cx.update(|cx| entity.read(cx).session.working.clone().unwrap());
    repository.archive(&saved.id).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.favorite(0, false, window, cx));
    })
    .unwrap();
    cx.update(|cx| {
        assert!(
            entity
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("no longer saved")
        )
    });
}

#[gpui_kit::test]
fn history_restores_unsaved_definition_search_and_layout_without_saving(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nNorth,30\nSouth,20\n").unwrap();
    let mut saved = DocumentDefinition::draft_table("Saved".into(), path.to_string_lossy().into());
    repository.save_view(&mut saved).unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
    open(cx, handle, &entity, FileEntry::View(saved.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
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
            view.set_search_text("North".into(), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let expected = cx.update(|cx| entity.read(cx).session.navigation_entry().unwrap());
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.navigation_entry().unwrap(), expected);
        assert_eq!(view.search_input.read(cx).value().as_ref(), "North");
        assert_eq!(view.session.result.row_count(), 2);
        assert_eq!(view.session.result.columns[0].0, "n");
        assert_eq!(view.grid.read(cx).column_width("n"), 123.);
        assert_eq!(view.grid.read(cx).selection().unwrap().active.row, 0);
        assert!(!view.loading && !view.busy);
        assert!(view.navigation_backup.is_none());
        assert!(view.navigation_history.target(true).is_some());
        view.history_writer.flush();
    });
    assert_eq!(repository.load_views().0, vec![saved]);
    travel(cx, handle, &entity, true);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.identity, SessionIdentity::DraftTable);
        assert!(view.session.search_text.is_empty());
    });
}

#[gpui_kit::test]
fn history_restores_direct_preferences_and_draft_pivot(cx: &mut TestAppContext) {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nNorth,30\nSouth,20\n").unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    cx.update(|cx| {
        entity.update(cx, |view, cx| {
            view.session.set_column_width("n".into(), 177.);
            let mut config = view.session.effective_table_config();
            config.column_order = vec!["n".into(), "region".into()];
            config.frozen_count = 1;
            view.session.set_table_layout(config).unwrap();
            view.autosave_table_preferences(cx);
            view.table_writer.flush().unwrap();
        })
    });
    let direct = cx.update(|cx| entity.read(cx).session.navigation_entry().unwrap());
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
    open(cx, handle, &entity, FileEntry::View(pivot));
    cx.update(|cx| {
        entity.update(cx, |view, _| {
            view.session.identity = SessionIdentity::DraftQuery;
            view.session.rename_working("Draft pivot".into()).unwrap();
        })
    });
    let query = cx.update(|cx| entity.read(cx).session.navigation_entry().unwrap());
    repository
        .update_table_preferences(
            &path,
            &TablePreferencesPatch {
                widths: [("n".into(), Some(299.))].into(),
                ..Default::default()
            },
        )
        .unwrap();
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.navigation_entry().unwrap(), direct);
        assert_eq!(view.grid.read(cx).column_width("n"), 177.);
        assert!(view.session.is_direct_table());
    });
    travel(cx, handle, &entity, true);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.navigation_entry().unwrap(), query);
        assert_eq!(view.session.result.row_count(), 2);
    });
}

#[gpui_kit::test]
fn history_missing_source_and_failed_recomputation_preserve_cursor(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
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
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::View(definition));
    open(cx, handle, &entity, FileEntry::Source(b));
    let (current, target, result) = cx.update(|cx| {
        let view = entity.read(cx);
        (
            view.session.navigation_entry(),
            view.navigation_history.target(false),
            view.session.result.clone(),
        )
    });
    std::fs::remove_file(&a).unwrap();
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_some());
        assert_eq!(view.session.navigation_entry(), current);
        assert_eq!(view.navigation_history.target(false), target);
        assert!(view.navigation_history.target(true).is_none());
    });
    std::fs::write(&a, "different\nchanged schema\n").unwrap();
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.as_ref().unwrap().contains("region"));
        assert_eq!(view.session.navigation_entry(), current);
        assert_eq!(view.navigation_history.target(false), target);
        assert!(std::sync::Arc::ptr_eq(
            &view.session.result.store,
            &result.store
        ));
        assert!(std::sync::Arc::ptr_eq(
            &view.grid.read(cx).result().store,
            &result.store
        ));
        assert!(!view.loading && !view.busy);
        assert!(view.navigation_backup.is_none());
    });
}

#[gpui_kit::test]
fn history_branches_and_rejects_superseded_loads(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let paths: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|name| {
            let path = home.path().join(format!("{name}.csv"));
            std::fs::write(&path, "n\n1\n").unwrap();
            path
        })
        .collect();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(paths[0].clone()));
    open(cx, handle, &entity, FileEntry::Source(paths[1].clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.navigate_history(false, window, cx);
            // There is no forward target until Back commits. This must be a no-op.
            view.navigate_history(true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(
            view.session.source_path.as_deref(),
            Some(paths[0].to_str().unwrap())
        );
        assert!(view.navigation_history.target(false).is_none());
        assert!(view.navigation_history.target(true).is_some());
    });
    travel(cx, handle, &entity, true);
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.navigate_history(false, window, cx);
            view.navigate_to_file(
                FileEntry::Source(paths[2].clone()),
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
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(
            view.session.source_path.as_deref(),
            Some(paths[2].to_str().unwrap())
        );
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
    travel(cx, handle, &entity, false);
    open(cx, handle, &entity, FileEntry::Source(paths[0].clone()));
    cx.update(|cx| {
        let view = entity.read(cx);
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

#[gpui_kit::test]
fn history_sql_restores_text_and_search_without_results_or_execution(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("data.csv");
    std::fs::write(&path, "n\n1\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.open_sql_editor(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let text = "CREATE TABLE history_marker AS SELECT 42 AS n; SELECT n FROM history_marker;";
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            let editor = view
                .sql_panel
                .as_ref()
                .unwrap()
                .read(cx)
                .editor
                .read(cx)
                .state()
                .clone();
            editor.update(cx, |input, cx| input.set_value(text, window, cx));
            view.session.set_sql_text(text.into());
            view.run_sql(true, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            assert_eq!(view.session.result.row_count(), 1);
            view.set_search_text("42".into(), window, cx);
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
    })
    .unwrap();
    cx.run_until_parked();
    let expected = cx.update(|cx| entity.read(cx).session.navigation_entry().unwrap());
    open(cx, handle, &entity, FileEntry::Source(path));
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.navigation_entry().unwrap(), expected);
        assert_eq!(view.session.identity, SessionIdentity::DraftSql);
        assert_eq!(view.session.result.row_count(), 0);
        assert_eq!(view.session.source.row_count(), 0);
        assert!(!view.sql_running && !view.busy && !view.loading);
        assert!(view.sql_panel.as_ref().unwrap().read(cx).error.is_none());
        assert!(view.query_cache.is_none() && view.table_query.is_none());
        assert!(view.navigation_backup.is_none());
        assert!(view.navigation_history.target(true).is_some());
    });
}

#[gpui_kit::test]
fn history_return_reloads_changed_source_data(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let a = home.path().join("a.csv");
    let b = home.path().join("b.csv");
    std::fs::write(&a, "n\n1\n").unwrap();
    std::fs::write(&b, "n\n9\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(a.clone()));
    open(cx, handle, &entity, FileEntry::Source(b));
    std::fs::write(&a, "n\n1\n2\n3\n").unwrap();
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(view.session.result.row_count(), 3);
        assert_eq!(view.session.source.row_count(), 3);
        assert!(view.navigation_backup.is_none());
    });
}

#[gpui_kit::test]
fn history_keys_only_navigate_from_grid_and_commands_are_available(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let a = home.path().join("a.csv");
    let b = home.path().join("b.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    std::fs::write(&b, "word\nur\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(a.clone()));
    open(cx, handle, &entity, FileEntry::Source(b.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        let input = entity.read(cx).search_input.clone();
        window.focus(&input.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.input("ur", cx);
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(b.to_str().unwrap())
        );
        assert_eq!(input.read(cx).value().as_ref(), "ur");
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.press("u", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(a.to_str().unwrap())
        );
        window.render_frame(cx);
        window.press("r", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(b.to_str().unwrap())
        );
        assert_eq!(entity.read(cx).session.search_text, "ur");
        entity.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("Back", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(a.to_str().unwrap())
        )
    });
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::Forward, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(b.to_str().unwrap())
        )
    });
}

fn column_names(entity: &Entity<DataFrameView>, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let result = &entity.read(cx).session.result;
        result
            .column_indices
            .iter()
            .map(|&index| result.columns[index].0.clone())
            .collect()
    })
}

#[gpui_kit::test]
fn ctrl_v_in_the_grid_opens_pasted_text_as_a_table_and_back_returns(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(a.clone()));
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
        "city\tsales\r\nOslo\t10\r\n\"Rome, IT\"\t20\r\n".into(),
    ));
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.press("ctrl-v", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let pasted = cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 2);
        assert!(
            view.copy_status
                .as_deref()
                .is_some_and(|status| status.starts_with("Pasted table saved to"))
        );
        std::path::PathBuf::from(view.session.source_path.clone().unwrap())
    });
    assert_eq!(pasted.parent().unwrap(), home.path().join("pasted"));
    assert_eq!(pasted.extension().unwrap(), "tsv");
    assert_eq!(column_names(&entity, cx), ["city", "sales"]);
    let rows = cx.update(|cx| {
        let result = &entity.read(cx).session.result;
        result
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
    travel(cx, handle, &entity, false);
    cx.update(|cx| {
        assert_eq!(
            entity.read(cx).session.source_path.as_deref(),
            Some(a.to_str().unwrap())
        )
    });
}

#[gpui_kit::test]
fn pasting_without_text_reports_an_error_and_keeps_the_table(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let a = home.path().join("a.csv");
    std::fs::write(&a, "word\nur\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(a.clone()));
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("  \r\n".into()));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::PasteTable, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(
            view.error.as_deref(),
            Some("The clipboard has no text to open as a table")
        );
        assert_eq!(
            view.session.source_path.as_deref(),
            Some(a.to_str().unwrap())
        );
    });
    assert!(!home.path().join("pasted").exists());
}

#[gpui_kit::test]
fn dropped_files_open_the_first_here_and_the_rest_in_new_windows(cx: &mut TestAppContext) {
    use gpui_kit::{ExternalPaths, FileDropEvent, PlatformInput, point};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let a = home.path().join("a.csv");
    let b = home.path().join("b.tsv");
    std::fs::write(&a, "word\nur\n").unwrap();
    std::fs::write(&b, "x\ty\n1\t2\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    let windows = cx.update(|cx| cx.windows().len());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let position = point(px(400.), px(300.));
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Entered {
                position,
                paths: ExternalPaths([a.clone(), b.clone()].into_iter().collect()),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Submit { position }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.session.source_path.as_deref(),
            Some(a.to_str().unwrap())
        );
        assert_eq!(cx.windows().len(), windows + 1);
    });
}

#[gpui_kit::test]
fn database_tree_toggles_filters_by_typing_and_opens_sources_and_metadata(cx: &mut TestAppContext) {
    use crate::domain::documents::RelationKind;
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("data");
    std::fs::create_dir_all(folder.join("2024")).unwrap();
    std::fs::write(
        folder.join("2024").join("q1.csv"),
        "region,n\nNorth,1\nSouth,2\n",
    )
    .unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.watched_folders = vec![folder.to_string_lossy().into_owned()];
    repository.save_settings(&settings).unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
    let press = |cx: &mut TestAppContext, keys: &[&str]| {
        for key in keys {
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.press(key, cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    press(cx, &["ctrl-b"]);
    cx.update_window(handle.into(), |_, window, cx| {
        let view = entity.read(cx);
        assert!(view.database_tree_open);
        assert!(view.database_tree.read(cx).contains_focus(window, cx));
    })
    .unwrap();
    assert!(repository.load_settings().unwrap().database_tree_open);
    // Typing in the tree starts the filter; Enter opens the first match here.
    press(cx, &["q", "1", "enter"]);
    cx.update_window(handle.into(), |_, window, cx| {
        let view = entity.read(cx);
        assert!(
            view.session
                .source_path
                .as_deref()
                .is_some_and(|path| path.ends_with("q1.csv")),
            "{:?}",
            view.session.source_path
        );
        assert_eq!(view.session.result.row_count(), 2);
        assert!(view.grid.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
    // A metadata function opens as a read-only database relation. Typing in
    // the tree replaces the old filter and moves to the box; Enter there opens.
    cx.update_window(handle.into(), |_, window, cx| {
        let tree = entity.read(cx).database_tree.clone();
        tree.update(cx, |tree, cx| tree.focus(window, cx));
    })
    .unwrap();
    press(cx, &["d"]);
    cx.update_window(handle.into(), |_, window, cx| {
        window.input("uckdb_settings", cx)
    })
    .unwrap();
    cx.run_until_parked();
    press(cx, &["enter"]);
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        let relation = view
            .session
            .working
            .as_ref()
            .and_then(|definition| definition.database_view.clone())
            .unwrap_or_else(|| {
                panic!(
                    "no relation: {:?} {:?} selected={:?}",
                    view.session.working.as_ref().map(|d| &d.name),
                    view.session.source_path,
                    view.database_tree
                        .read(cx)
                        .tree
                        .read(cx)
                        .selected_item()
                        .map(|i| i.id.clone())
                )
            });
        assert_eq!(relation.kind, RelationKind::Function);
        assert_eq!(relation.name, "duckdb_settings");
        assert!(view.session.result.row_count() > 0);
    });
    // Ctrl+B from the grid hides the tree and remembers that.
    press(cx, &["ctrl-b"]);
    cx.update(|cx| assert!(!entity.read(cx).database_tree_open));
    assert!(!repository.load_settings().unwrap().database_tree_open);
}

#[gpui_kit::test]
fn database_tree_refresh_button_lists_new_tables(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, entity) = viewer(cx, repository);
    let labels = |cx: &mut TestAppContext| {
        cx.update(|cx| entity.read(cx).database_tree.read(cx).relation_labels())
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-b", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!labels(cx).contains(&"main.added_later".to_owned()));
    cx.update(|cx| {
        entity
            .read(cx)
            .database
            .connect()
            .unwrap()
            .execute_batch("CREATE TABLE main.added_later AS SELECT 1 AS n")
            .unwrap();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("database-tree-refresh", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(labels(cx).contains(&"main.added_later".to_owned()));
}

#[gpui_kit::test]
fn source_checks_skip_unchanged_files_but_still_see_edits(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, entity) = viewer(cx, repository);
    let path = home.path().join("watched.csv");
    std::fs::write(&path, "n\n1\n2\n").unwrap();
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let check = |cx: &mut TestAppContext| {
        cx.update(|cx| entity.update(cx, |view, cx| view.check_source(cx)));
        cx.run_until_parked();
    };
    let rows = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            view.session.result.row_count()
        })
    };
    check(cx);
    cx.update(|cx| assert!(entity.read(cx).source_probe.is_some()));
    check(cx);
    assert_eq!(rows(cx), 2);

    std::fs::write(&path, "n\n1\n2\n3\n").unwrap();
    check(cx);
    assert_eq!(rows(cx), 3);
    check(cx);
    cx.update(|cx| assert!(entity.read(cx).source_probe.is_some()));

    // A database change, such as SQL dropping the table, forces the full check.
    cx.update(|cx| {
        let view = entity.read(cx);
        view.database
            .connect()
            .unwrap()
            .execute_batch("DROP TABLE files.watched_csv")
            .unwrap();
        view.database.invalidate();
    });
    check(cx);
    assert_eq!(rows(cx), 3);
    let restored: i64 = cx.update(|cx| {
        entity
            .read(cx)
            .database
            .connect()
            .unwrap()
            .query_row("SELECT count(*) FROM files.watched_csv", [], |row| {
                row.get(0)
            })
            .unwrap()
    });
    assert_eq!(restored, 3);
}
