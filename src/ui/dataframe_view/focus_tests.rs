use super::DataFrameView;
use crate::domain::documents::ViewKind;
use crate::domain::frame::FrameSnapshot;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use duckdb::types::Value;
use gpui_kit::component::{Root, WindowExt};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Focusable, TestAppContext, px, size};

#[gpui_kit::test]
fn numeric_shortcuts_follow_active_columns_and_restore_from_settings(cx: &mut TestAppContext) {
    use crate::domain::formatting::NumericFormat;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let source = home
        .path()
        .join("source.csv")
        .to_string_lossy()
        .into_owned();
    let (handle, view) = viewer_with_document(cx, repository.clone(), Some(source.clone()), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid = view.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.press(",", cx);
        window.press(">", cx);
        assert!(
            repository
                .load_settings()
                .unwrap()
                .column_formats
                .is_empty()
        );
        window.press("right", cx);
        window.press(",", cx);
        window.press("shift-.", cx);
        window.press(">", cx);
        window.press("shift-,", cx);
        assert_eq!(
            grid.read(cx).numeric_format("sales"),
            Some(NumericFormat {
                precision: 1,
                grouping: false
            })
        );
        window.press("alt-left", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid = view.read(cx).grid.clone();
        assert_eq!(view.read(cx).session.result.columns[0].0, "sales");
        assert_eq!(
            grid.read(cx).numeric_format("sales"),
            Some(NumericFormat {
                precision: 1,
                grouping: false
            })
        );
        assert_eq!(grid.read(cx).numeric_format("region"), None);
        assert!(!view.read(cx).session.dirty);
        let input = view.read(cx).search_input.clone();
        window.focus(&input.read(cx).focus_handle(cx), cx);
        window.input(",<>", cx);
        assert_eq!(
            grid.read(cx).numeric_format("sales"),
            Some(NumericFormat {
                precision: 1,
                grouping: false
            })
        );
    })
    .unwrap();
    let persisted = repository.load_settings().unwrap();
    assert_eq!(
        persisted.column_formats[&format!("table:{source}")]["sales"],
        NumericFormat {
            precision: 1,
            grouping: false
        }
    );
    cx.update(|cx| {
        cx.global_mut::<super::ColumnFormatRuntime>()
            .profiles
            .remove(&repository.paths.config);
    });
    let (handle, reopened) = viewer_with_document(cx, repository, Some(source), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            reopened.read(cx).grid.read(cx).numeric_format("sales"),
            Some(NumericFormat {
                precision: 1,
                grouping: false
            })
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn saved_queries_autosave_formats_without_saving_query_edits(cx: &mut TestAppContext) {
    use crate::domain::documents::{Aggregation, DocumentDefinition, QueryConfig};
    use crate::domain::formatting::NumericFormat;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let source = home
        .path()
        .join("source.csv")
        .to_string_lossy()
        .into_owned();
    let mut definition = DocumentDefinition::draft_table("Totals".into(), source.clone());
    definition.view = ViewKind::Query(QueryConfig {
        group_by: vec!["region".into()],
        aggregations: vec![Aggregation::sql_expression(
            "sum(sales)".into(),
            Some("Total".into()),
        )],
        ..Default::default()
    });
    repository.save_view(&mut definition).unwrap();
    let (handle, view) = viewer_with_document(
        cx,
        repository.clone(),
        Some(source.clone()),
        Some(definition.clone()),
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid = view.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.press("right", cx);
        assert_eq!(grid.read(cx).numeric_format("Total").unwrap().precision, 2);
        window.press(",", cx);
        window.press(">", cx);
        assert_eq!(grid.read(cx).numeric_format("Total").unwrap().precision, 3);
        window.press("<", cx);
        assert_eq!(grid.read(cx).numeric_format("Total").unwrap().precision, 2);
        window.press("<", cx);
        window.press("<", cx);
        window.press("<", cx);
        assert_eq!(
            grid.read(cx).numeric_format("Total"),
            Some(NumericFormat {
                precision: 0,
                grouping: false
            })
        );
        assert!(!view.read(cx).session.dirty);
        view.update(cx, |view, _| {
            view.session.dirty = true;
        });
        window.press(">", cx);
        assert!(view.read(cx).session.dirty);
    })
    .unwrap();
    let settings = repository.load_settings().unwrap();
    let format = NumericFormat {
        precision: 1,
        grouping: false,
    };
    assert_eq!(
        settings.column_formats[&format!("view:{}", definition.id)]["Total"],
        format
    );
    assert_eq!(repository.load_views().0, vec![definition.clone()]);
    let (handle, reopened) = viewer_with_document(cx, repository, Some(source), Some(definition));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            reopened.read(cx).grid.read(cx).numeric_format("Total"),
            Some(format)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn query_configuration_changes_refit_existing_columns(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository);
    cx.run_until_parked();
    for (shortcut, field) in [("g", "region"), ("p", "sales"), ("a", "sales")] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let grid = view.read(cx).grid.clone();
            window.focus(&grid.read(cx).focus_handle(cx), cx);
            for _ in 0..25 {
                window.press(")", cx);
            }
            assert!(grid.read(cx).column_width("region") > 250.);
            window.press(shortcut, cx);
            window.render_frame(cx);
            window.input(field, cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let viewer = view.read(cx);
            assert!(viewer.error.is_none(), "{:?}", viewer.error);
            assert!(viewer.grid.read(cx).widths_ready());
            assert!(viewer.grid.read(cx).column_width("region") < 250.);
            window.close_dialog(cx);
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid = view.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.press(")", cx);
        let manual = grid.read(cx).column_width("region");
        view.update(cx, |view, cx| view.schedule_search("North".into(), cx));
        assert_eq!(grid.read(cx).column_width("region"), manual);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn copy_as_search_is_focused_from_shortcut_and_command(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, entity) = viewer_with_repository(cx, repository);
    cx.run_until_parked();
    for shortcut in [true, false] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.focus(&entity.read(cx).grid.read(cx).focus_handle(cx), cx);
            if shortcut {
                window.press("ctrl-shift-c", cx);
            } else {
                entity.update(cx, |view, cx| {
                    view.dispatch_command(
                        crate::application::commands::CommandId::CopyAs,
                        window,
                        cx,
                    )
                });
            }
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.input("json records", cx);
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
            assert!(!window.has_active_dialog(cx));
            let text = cx.read_from_clipboard().unwrap().text().unwrap();
            let copied: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(copied[0]["region"], "North");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn cached_direct_widths_skip_sampling_and_shortcuts_respect_focus(cx: &mut TestAppContext) {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
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
            },
        )
        .unwrap();
    let (handle, entity) = viewer_with_document(
        cx,
        repository.clone(),
        Some(source.to_string_lossy().into_owned()),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.render_frame(cx);
        let view = entity.read(cx);
        assert_eq!(grid.read(cx).column_width("region"), 150.);
        assert_eq!(
            view.session
                .source
                .store
                .lock()
                .unwrap()
                .width_sampling_calls(),
            0
        );
        assert_eq!(view.session.result.columns[0].0, "sales");
        assert_eq!(view.session.effective_table_config().frozen_count, 1);
        assert!(
            view.session.working.as_ref().unwrap().slicers[0]
                .selected
                .is_empty()
        );
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.press("shift-0", cx);
        assert_eq!(grid.read(cx).column_width("sales"), 100.);
        assert_eq!(grid.read(cx).column_width("region"), 150.);
        window.press("shift-9", cx);
        assert_eq!(grid.read(cx).column_width("sales"), 90.);
        window.press(")", cx);
        assert_eq!(grid.read(cx).column_width("sales"), 100.);
        entity.update(cx, |view, cx| view.change_font_size(1, cx));
        window.render_frame(cx);
        assert_eq!(grid.read(cx).column_width("sales"), 100.);
        assert_eq!(grid.read(cx).column_width("region"), 150.);
        assert_eq!(
            entity
                .read(cx)
                .session
                .source
                .store
                .lock()
                .unwrap()
                .width_sampling_calls(),
            0
        );
        let input = entity.read(cx).search_input.clone();
        window.focus(&input.read(cx).focus_handle(cx), cx);
        window.input("()", cx);
        assert_eq!(grid.read(cx).column_width("sales"), 100.);
        entity.update(cx, |view, cx| view.open_information(window, cx));
        window.press(")", cx);
        assert_eq!(grid.read(cx).column_width("sales"), 100.);
        window.close_dialog(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(!view.session.dirty);
        view.table_writer.flush().unwrap();
    });
    assert_eq!(
        repository.load_table_preferences(&source).unwrap().widths["sales"],
        100.
    );
}

#[gpui_kit::test]
fn missing_widths_infer_once_and_manual_widths_survive_pending_inference(cx: &mut TestAppContext) {
    use crate::domain::table_preferences::TablePreferencesPatch;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
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
    let (handle, entity) = viewer_with_document(
        cx,
        repository.clone(),
        Some(source.to_string_lossy().into_owned()),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        // Sampling has returned candidates, but they have not been measured yet.
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.press(")", cx);
        let manual = grid.read(cx).column_width("region");
        window.render_frame(cx);
        assert_eq!(grid.read(cx).column_width("region"), manual);
        assert_eq!(grid.read(cx).column_width("sales"), 175.);
        assert!(grid.read(cx).widths_ready());
        assert_eq!(
            entity
                .read(cx)
                .session
                .source
                .store
                .lock()
                .unwrap()
                .width_sampling_calls(),
            1
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.schedule_search("North".into(), cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert_eq!(
            view.session
                .source
                .store
                .lock()
                .unwrap()
                .width_sampling_calls(),
            1
        );
        view.table_writer.flush().unwrap();
    });
    assert_eq!(
        repository.load_table_preferences(&source).unwrap().widths["sales"],
        175.
    );
}

#[gpui_kit::test]
fn programmatic_updates_submit_once_and_supersede_a_typing_debounce(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (handle, view) =
        viewer_with_repository(cx, ViewRepository::new(StorePaths::for_home(home.path())));
    cx.run_until_parked();
    let before = cx.update(|cx| view.read(cx).session.token());
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_search_text("north".into(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(!view.busy);
        assert!(!view.debouncing);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.session.token().request_revision,
            before.request_revision + 1
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        let input = view.read(cx).search_input.clone();
        window.render_frame(cx);
        window.focus(&input.read(cx).focus_handle(cx), cx);
        window.press("ctrl-a", cx);
        window.input("south", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(view.read(cx).debouncing));
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_search_text(String::new(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let immediate = cx.update(|cx| {
        assert_eq!(view.read(cx).session.result.row_count(), 2);
        assert!(!view.read(cx).busy);
        view.read(cx).session.token()
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(view.read(cx).session.token(), immediate);
        assert_eq!(view.read(cx).session.result.row_count(), 2);
    });
}

#[gpui_kit::test]
fn same_source_navigation_shares_rows_but_requeries_changed_sort_and_filter(
    cx: &mut TestAppContext,
) {
    use super::FileEntry;
    use crate::domain::documents::{DocumentDefinition, ScalarKey, SlicerDefinition, SortKey};
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("source.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let mut base =
        DocumentDefinition::draft_table("Base".into(), path.to_string_lossy().into_owned());
    repository.save_view(&mut base).unwrap();
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    let navigate = |cx: &mut TestAppContext, definition| {
        cx.update_window(handle.into(), |_, _, cx| {
            view.update(cx, |view, cx| {
                view.navigate_to_file(
                    FileEntry::View(definition),
                    false,
                    false,
                    false,
                    handle.into(),
                    cx,
                )
            });
        })
        .unwrap();
        cx.run_until_parked();
    };
    navigate(cx, base.clone());
    let (source, first) = cx.update(|cx| {
        let view = view.read(cx);
        assert!(!view.busy);
        (view.session.source.clone(), view.session.result.clone())
    });
    let mut reordered = base.clone();
    if let ViewKind::Table(table) = &mut reordered.view {
        table.column_order = vec!["n".into(), "region".into()];
        table.frozen_count = 1;
    }
    navigate(cx, reordered);
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(Arc::ptr_eq(&source.store, &view.session.source.store));
        assert_eq!(source.source_revision, view.session.source.source_revision);
        let result = &view.session.result;
        assert!(Arc::ptr_eq(&first.row_ids, &result.row_ids));
        assert!(Arc::ptr_eq(
            first.row_map.as_ref().unwrap(),
            result.row_map.as_ref().unwrap()
        ));
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
    navigate(cx, sorted);
    cx.update(|cx| {
        let result = &view.read(cx).session.result;
        assert!(!Arc::ptr_eq(&first.row_ids, &result.row_ids));
        assert_eq!(&*result.row_ids, &[1, 0]);
    });
    let mut sliced = base;
    sliced.slicers = vec![SlicerDefinition {
        sql: false,
        condition: None,
        column: "region".into(),
        selected: vec![ScalarKey::String("North".into())],
    }];
    navigate(cx, sliced);
    cx.update(|cx| assert_eq!(&*view.read(cx).session.result.row_ids, &[0]));
    let writer = cx.update(|cx| view.read(cx).history_writer.clone());
    writer.flush();
}

#[gpui_kit::test]
fn failed_destination_query_restores_the_unsaved_document(cx: &mut TestAppContext) {
    use super::FileEntry;
    use crate::domain::documents::{DocumentDefinition, ScalarKey, SlicerDefinition};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("source.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\n").unwrap();
    let (handle, view) = viewer_with_repository(cx, repository);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.session.start_table_draft("Draft".into()).unwrap();
            view.session.rename_working("Unsaved".into()).unwrap();
            view.set_search_text("north".into(), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            let mut definition = DocumentDefinition::draft_table(
                "Invalid".into(),
                path.to_string_lossy().into_owned(),
            );
            definition.slicers = vec![SlicerDefinition {
                sql: false,
                condition: None,
                column: "missing".into(),
                selected: vec![ScalarKey::String("North".into())],
            }];
            view.navigate_to_file(
                FileEntry::View(definition),
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
        let view = view.read(cx);
        assert!(view.error.as_ref().unwrap().contains("missing"));
        assert_eq!(view.session.working.as_ref().unwrap().name, "Unsaved");
        assert!(view.session.dirty);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(view.session.search_text, "north");
        assert_eq!(view.search_input.read(cx).value().as_ref(), "north");
        assert!(!view.busy);
        assert!(view.navigation_backup.is_none());
    });
}

#[gpui_kit::test]
fn rapid_navigation_rejects_old_loads_and_carries_or_clears_search(cx: &mut TestAppContext) {
    use super::FileEntry;
    use crate::domain::documents::{DocumentDefinition, SortKey};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
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
    let (handle, view) = viewer_with_repository(cx, repository);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_search_text("north".into(), window, cx);
            view.navigate_to_file(
                FileEntry::View(base.clone()),
                true,
                false,
                false,
                handle.into(),
                cx,
            );
            view.navigate_to_file(
                FileEntry::View(sorted.clone()),
                true,
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
        let view = view.read(cx);
        assert_eq!(view.session.working.as_ref().unwrap().name, "Latest");
        assert_eq!(view.session.search_text, "north");
        assert_eq!(view.session.sort[0].column, "n");
        assert_eq!(&*view.session.result.row_ids, &[0]);
        assert!(!view.loading && !view.busy);
    });
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            view.navigate_to_file(
                FileEntry::View(sorted),
                false,
                false,
                false,
                handle.into(),
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.session.search_text.is_empty());
        assert_eq!(&*view.session.result.row_ids, &[1, 0]);
    });
    let writer = cx.update(|cx| view.read(cx).history_writer.clone());
    writer.flush();
}

#[gpui_kit::test]
fn two_windows_follow_reloads_and_report_missing_sources(cx: &mut TestAppContext) {
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let source = home.path().join("shared.csv");
    std::fs::write(&source, "n\n1\n2\n").unwrap();
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let first = database.load(&source, LoadMode::Check).unwrap();
    let second = database.load(&source, LoadMode::Check).unwrap();
    let (first_window, first_view) = viewer_with_repository(cx, repository.clone());
    let (second_window, second_view) = viewer_with_repository(cx, repository);
    cx.update_window(first_window.into(), |_, _, cx| {
        first_view.update(cx, |view, cx| {
            view.session.source_path = Some(source.to_string_lossy().into_owned());
            view.replace_dataframe(FrameSnapshot::new(first, 2), cx);
        });
    })
    .unwrap();
    cx.update_window(second_window.into(), |_, _, cx| {
        second_view.update(cx, |view, cx| {
            view.session.source_path = Some(source.to_string_lossy().into_owned());
            view.replace_dataframe(FrameSnapshot::new(second, 2), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
    std::fs::write(&source, "n\n300\n").unwrap();
    cx.update_window(first_window.into(), |_, window, cx| {
        first_view.update(cx, |view, cx| view.reload(false, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(std::sync::Arc::ptr_eq(
            &first_view.read(cx).database,
            &second_view.read(cx).database
        ));
        assert_eq!(first_view.read(cx).session.result.row_count(), 1);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = second_view.read(cx);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.session
                .result
                .store
                .lock()
                .unwrap()
                .fetch_row_ids(&[0])
                .unwrap()[0]
                .values[0],
            Some("300".into())
        );
    });
    std::fs::remove_file(&source).unwrap();
    cx.update_window(second_window.into(), |_, window, cx| {
        second_view.update(cx, |view, cx| view.reload(false, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = second_view.read(cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("cannot access source")
        );
        assert!(
            view.session
                .result
                .store
                .lock()
                .unwrap()
                .fetch_row_ids(&[0])
                .is_err()
        );
    });
}

#[gpui_kit::test]
fn source_monitor_reports_missing_files_and_recovers_when_recreated(cx: &mut TestAppContext) {
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let frame = FrameSnapshot::new(database.load(&source, LoadMode::Check).unwrap(), 1);
    let (_, view) = viewer_with_document(
        cx,
        repository,
        Some(source.to_string_lossy().into_owned()),
        None,
    );
    cx.update(|cx| view.update(cx, |view, cx| view.replace_dataframe(frame, cx)));
    cx.run_until_parked();
    std::fs::remove_file(&source).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            view.read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("Cannot refresh source")
        )
    });
    std::fs::write(&source, "region,sales\nEast,300\n").unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.result.row_count(), 1);
        assert_eq!(
            view.session
                .result
                .store
                .lock()
                .unwrap()
                .fetch_row_ids(&[0])
                .unwrap()[0]
                .values[0],
            Some("East".into())
        );
    });
}

fn viewer(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<DataFrameView>,
) {
    viewer_with_repository(
        cx,
        ViewRepository::new(StorePaths::for_home(
            &std::env::temp_dir().join("quick-pivot-focus-tests"),
        )),
    )
}

fn finish_slicer_work(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn slicers_stay_left_of_the_grid_and_use_two_columns_when_the_window_fits(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.simulate_window_resize(handle.into(), size(px(1400.), px(900.)));
    cx.update_window(handle.into(), |_, _, cx| {
        let source = DuckDbStore::from_rows(
            &[
                ("region".into(), "VARCHAR".into()),
                ("sales".into(), "BIGINT".into()),
                ("flag".into(), "BOOLEAN".into()),
            ],
            [
                vec![
                    Value::Text("North".into()),
                    Value::BigInt(10),
                    Value::Boolean(true),
                ],
                vec![
                    Value::Text("South".into()),
                    Value::BigInt(20),
                    Value::Boolean(false),
                ],
            ],
        )
        .unwrap();
        view.update(cx, |view, cx| {
            view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
            for column in ["region", "sales", "flag"] {
                view.add_slicer(column.into(), cx);
            }
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let region = window.find("slicer-value:region:0").bounds();
        let sales = window.find("slicer-value:sales:0").bounds();
        let flag = window.find("slicer-value:flag:0").bounds();
        assert!(sales.origin.x > region.origin.x);
        assert_eq!(region.origin.y, sales.origin.y);
        assert_eq!(flag.origin.x, region.origin.x);
        assert!(flag.origin.y > region.origin.y);
        assert!(sales.origin.x + sales.size.width <= px(480.));
        window.click("slicer-value:sales:1", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| assert_eq!(view.read(cx).session.result.row_count(), 1));
    cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let region = window.find("slicer-value:region:0").bounds();
        let sales = window.find("slicer-value:sales:0").bounds();
        assert_eq!(region.origin.x, sales.origin.x);
        assert!(sales.origin.y > region.origin.y);
        assert!(region.origin.x + region.size.width <= px(240.));
        window.click("slicer-clear:sales", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| assert_eq!(view.read(cx).session.result.row_count(), 2));
}

#[gpui_kit::test]
fn overflowing_slicer_scrollbar_works_without_hover_and_keeps_other_cards_in_place(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, point};
    let (handle, view) = viewer(cx);
    cx.simulate_window_resize(handle.into(), size(px(1200.), px(600.)));
    cx.update_window(handle.into(), |_, _, cx| {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE qp_data AS SELECT range AS n, range AS m FROM range(1000)")
            .unwrap();
        let source = DuckDbStore::from_connection(connection).unwrap();
        view.update(cx, |view, cx| {
            view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
            for column in ["n", "m"] {
                view.add_slicer(column.into(), cx);
            }
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let first = window.find("slicer-value:n:0").bounds();
        let first_y = first.origin.y;
        let other_y = window.find("slicer-value:m:0").bounds().origin.y;
        let position = point(first.right() + px(9.), first.origin.y + px(200.));
        window.dispatch_event(
            MouseDownEvent {
                position,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                position,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        assert!(window.find("slicer-value:n:0").bounds().origin.y < first_y);
        assert_eq!(window.find("slicer-value:m:0").bounds().origin.y, other_y);
        assert_eq!(view.read(cx).session.result.row_count(), 1_000);
        assert!(
            view.read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .iter()
                .all(|card| card.selected.is_empty())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn slicer_picker_filters_by_identity_and_cards_filter_clear_remove(cx: &mut TestAppContext) {
    use crate::domain::documents::ScalarKey;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("space", cx);
        window.render_frame(cx);
        window.press("z", cx);
        window.render_frame(cx);
        window.input("sales", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        window.press("enter", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!window.has_active_dialog(cx));
        let definition = view.read(cx).session.working.as_ref().unwrap();
        assert_eq!(definition.slicers[0].column, "sales");
        assert!(!view.read(cx).session.dirty);
        assert!(repository.load_views().0.is_empty());
        assert!(window.try_find("slicer-remove:sales").is_some());
        window.click("slicer-value:sales:0", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.result.row_count(), 1);
        assert_eq!(
            view.read(cx).session.working.as_ref().unwrap().slicers[0].selected,
            [ScalarKey::Signed("10".into())]
        );
        window.click("slicer-clear:sales", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.result.row_count(), 2);
        window.drag_to("slicer-value:sales:0", "slicer-value:sales:1", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        use gpui_kit::{InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
        window.render_frame(cx);
        assert_eq!(
            view.read(cx).session.working.as_ref().unwrap().slicers[0]
                .selected
                .len(),
            2
        );
        let position = window.find("slicer-value:sales:0").bounds().center();
        let modifiers = Modifiers {
            control: true,
            ..Default::default()
        };
        window.dispatch_event(
            MouseDownEvent {
                position,
                modifiers,
                button: MouseButton::Left,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                position,
                modifiers,
                button: MouseButton::Left,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.result.row_count(), 1);
        assert_eq!(
            view.read(cx).session.working.as_ref().unwrap().slicers[0].selected,
            [ScalarKey::Signed("20".into())]
        );
        use gpui_kit::{
            InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        };
        let from = window.find("slicer-value:sales:1").bounds().center();
        let to = window.find("slicer-value:sales:0").bounds().center();
        let modifiers = Modifiers {
            control: true,
            ..Default::default()
        };
        window.dispatch_event(
            MouseDownEvent {
                position: from,
                modifiers,
                button: MouseButton::Left,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseMoveEvent {
                position: to,
                modifiers,
                pressed_button: Some(MouseButton::Left),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                position: to,
                modifiers,
                button: MouseButton::Left,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.result.row_count(), 2);
        assert_eq!(
            view.read(cx).session.working.as_ref().unwrap().slicers[0]
                .selected
                .len(),
            2
        );
        window.press("tab", cx);
        let search_focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&search_focus));
        window.click("slicer-remove:sales", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("slicer-remove:sales").is_none());
        assert!(
            view.read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .is_empty()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn slicer_availability_ignores_search_and_picker_excludes_existing_cards(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            view.add_slicer("region".into(), cx);
            view.add_slicer("sales".into(), cx);
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-value:region:0", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    let catalog = cx.update(|cx| view.read(cx).slicer_stage.as_ref().unwrap().catalog.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_search_text("South".into(), window, cx)
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        let stage = view.read(cx).slicer_stage.as_ref().unwrap();
        assert!(std::sync::Arc::ptr_eq(&stage.catalog, &catalog));
        assert!(stage.cards[0].values.iter().all(|value| value.available));
        assert!(stage.cards[1].values[0].available);
        assert!(!stage.cards[1].values[1].available);
        assert_eq!(view.read(cx).session.result.row_count(), 0);
        view.update(cx, |view, cx| view.open_slicer_picker(window, cx));
        window.render_frame(cx);
        window.input("region", cx);
        window.press("enter", cx);
        assert!(window.has_active_dialog(cx));
        assert_eq!(
            view.read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .len(),
            2
        );
        window.press("escape", cx);
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn slicer_picker_offers_cards_without_values_before_discovery_finishes(cx: &mut TestAppContext) {
    use crate::ui::slicer_sidebar::SlicerKind;
    let (handle, view) = viewer(cx);
    let kinds = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let picker = view.read(cx).slicer_picker.clone().unwrap();
            let picker = picker.read(cx);
            let kinds = picker
                .choices()
                .iter()
                .map(|choice| choice.kind)
                .collect::<Vec<_>>();
            (picker.loading, kinds)
        })
    };
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.open_slicer_picker(window, cx));
    })
    .unwrap();
    // Discovery has not run yet, but SQL and condition cards are listed.
    let (loading, before) = kinds(cx);
    assert!(loading);
    assert_eq!(before[0], SlicerKind::Sql);
    assert!(before.len() > 1);
    assert!(before[1..].iter().all(|kind| *kind == SlicerKind::Condition));
    finish_slicer_work(cx);
    let (loading, after) = kinds(cx);
    assert!(!loading);
    assert!(after.contains(&SlicerKind::Values));
    // Closing the picker stops discovering every column.
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        let view = view.read(cx);
        assert!(view.slicer_picker.is_none());
        assert!(view.slicer_discovery.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn slicer_reload_errors_keep_last_result_and_missing_selected_values_remain_removable(
    cx: &mut TestAppContext,
) {
    use crate::domain::documents::ScalarKey;
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            view.add_slicer("region".into(), cx);
            let mut definition = view.session.working.clone().unwrap();
            definition.id = "named-table".into();
            view.session.saved(definition);
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-value:region:0", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        assert_eq!(view.read(cx).session.result.row_count(), 1);
        let frame = FrameSnapshot::new(
            DuckDbStore::from_rows(
                &[("region".into(), "VARCHAR".into())],
                [vec![Value::Text("South".into())]],
            )
            .unwrap(),
            2,
        );
        view.update(cx, |view, cx| view.replace_dataframe(frame, cx));
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).session.result.row_count(), 0);
        assert_eq!(
            view.read(cx).session.working.as_ref().unwrap().slicers[0].selected,
            [ScalarKey::String("North".into())]
        );
        window.click("slicer-value:region:1", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        assert_eq!(view.read(cx).session.result.row_count(), 1);
        let frame = FrameSnapshot::new(DuckDbStore::synthetic(100).unwrap(), 3);
        view.update(cx, |view, cx| view.replace_dataframe(frame, cx));
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let view = view.read(cx);
        assert_eq!(view.session.result.row_count(), 1);
        assert!(view.error.as_ref().unwrap().contains("missing"));
        assert!(view.slicer_stage.as_ref().unwrap().cards[0].error.is_some());
        window.click("slicer-remove:region", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| assert_eq!(view.read(cx).session.result.row_count(), 100));
}

fn viewer_with_repository(
    cx: &mut TestAppContext,
    repository: ViewRepository,
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<DataFrameView>,
) {
    viewer_with_document(cx, repository, None, None)
}

fn viewer_with_document(
    cx: &mut TestAppContext,
    repository: ViewRepository,
    source_path: Option<String>,
    document: Option<crate::domain::documents::DocumentDefinition>,
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<DataFrameView>,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
    });
    let mut viewer = None;
    let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
        let frame = DuckDbStore::from_rows(
            &[
                ("region".into(), "VARCHAR".into()),
                ("sales".into(), "BIGINT".into()),
            ],
            [
                vec![Value::Text("North".into()), Value::BigInt(10)],
                vec![Value::Text("South".into()), Value::BigInt(20)],
            ],
        )
        .unwrap();
        let view = cx.new(|cx| {
            DataFrameView::new(
                FrameSnapshot::new(frame, 1),
                source_path,
                String::new(),
                document,
                repository,
                window,
                cx,
            )
        });
        viewer = Some(view.clone());
        Root::new(view, window, cx)
    });
    (handle, viewer.unwrap())
}

#[gpui_kit::test]
fn theme_editor_command_saves_colors_across_windows_and_restarts(cx: &mut TestAppContext) {
    use crate::application::theme::{ThemeColor, ThemeSettings};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    let (_, other_view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("editor theme", cx);
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
        assert!(window.try_find("theme-save").is_some());
        window.press("ctrl-a", cx);
        window.input("#AABBCC", cx);
        assert_eq!(view.read(cx).theme, ThemeSettings::default());
        assert!(!repository.paths.config.join("theme.json").exists());
        window.render_frame(cx);
        window.click("theme-save", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    cx.run_until_parked();
    let saved = repository.load_theme().unwrap();
    assert_eq!(saved.header_background, ThemeColor::rgb(0xaabbcc));
    cx.update(|cx| {
        assert_eq!(view.read(cx).theme, saved);
        assert_eq!(other_view.read(cx).theme, saved);
        assert!(!view.read(cx).session.dirty);
    });
    // Drop the shared runtime so a new viewer must load the file again.
    cx.update(|cx| cx.set_global(super::ThemeRuntime::default()));
    let (_, restarted) = viewer_with_repository(cx, repository);
    cx.update(|cx| assert_eq!(restarted.read(cx).theme, saved));
}

#[gpui_kit::test]
fn theme_editor_cancel_validation_reset_and_failed_save_keep_the_draft(cx: &mut TestAppContext) {
    use crate::application::theme::{ThemeColor, ThemeSettings};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let initial = ThemeSettings {
        header_background: ThemeColor::rgb(0xaabbcc),
        ..Default::default()
    };
    repository.save_theme(&initial).unwrap();
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_theme_editor(window, cx));
        window.render_frame(cx);
        window.press("ctrl-a", cx);
        window.input("#bad", cx);
        window.click("theme-save", cx);
        assert!(window.has_active_dialog(cx));
        assert_eq!(repository.load_theme().unwrap(), initial);
        assert_eq!(view.read(cx).theme, initial);
        window.render_frame(cx);
        window.click("theme-reset", cx);
        window.render_frame(cx);
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        assert_eq!(repository.load_theme().unwrap(), initial);
        assert_eq!(view.read(cx).theme, initial);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let form = view.update(cx, |view, cx| view.open_theme_editor(window, cx));
        window.render_frame(cx);
        window.click("theme-reset", cx);
        window.render_frame(cx);
        assert_eq!(form.read(cx).build(cx).unwrap(), ThemeSettings::default());
        // A file in place of the config directory forces a write failure.
        let blocked = home.path().join("blocked");
        std::fs::write(&blocked, "blocked").unwrap();
        view.update(cx, |view, _| view.repository.paths.config = blocked);
        window.click("theme-save", cx);
        assert!(window.has_active_dialog(cx));
        assert_eq!(form.read(cx).build(cx).unwrap(), ThemeSettings::default());
        assert_eq!(view.read(cx).theme, initial);
        assert_eq!(repository.load_theme().unwrap(), initial);
        view.update(cx, |view, _| view.repository = repository.clone());
        window.render_frame(cx);
        window.click("theme-save", cx);
        assert!(!window.has_active_dialog(cx));
        assert_eq!(repository.load_theme().unwrap(), ThemeSettings::default());
    })
    .unwrap();
}

#[gpui_kit::test]
fn theme_editor_saves_with_enter_and_preserves_typed_alpha(cx: &mut TestAppContext) {
    use crate::application::theme::ThemeColor;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_theme_editor(window, cx));
        window.render_frame(cx);
        window.press("ctrl-a", cx);
        window.input("#12345680", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        assert!(!window.has_active_dialog(cx));
        assert_eq!(
            repository.load_theme().unwrap().header_background,
            ThemeColor::parse("#12345680").unwrap()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn palettes_focus_their_query(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("info", cx);
        window.press("escape", cx);
        assert!(
            window.has_active_dialog(cx),
            "Escape should clear the query first"
        );
        window.close_dialog(cx);

        view.update(cx, |view, cx| view.open_file_palette(window, cx));
        window.render_frame(cx);
        window.input("report", cx);
        window.press("escape", cx);
        assert!(
            window.has_active_dialog(cx),
            "Escape should clear the query first"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn font_picker_and_shortcuts_persist_share_and_clamp(cx: &mut TestAppContext) {
    use super::{CommandId, FontRuntime};
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.font_family = Some("Existing font".into());
    repository.save_settings(&settings).unwrap();
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    let (_, other) = viewer_with_repository(cx, repository.clone());
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("ctrl-=", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).font.size, 14);
        assert_eq!(window.rem_size(), px(14.));
        window.press("ctrl--", cx);
        assert_eq!(view.read(cx).font.size, 13);
        let search_focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&search_focus, cx);
        window.press("ctrl-=", cx);
        assert_eq!(view.read(cx).font.size, 14);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("picker font", cx);
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
        assert!(window.has_active_dialog(cx));
        window.input("default", cx);
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
        assert!(!window.has_active_dialog(cx));
        assert!(view.read(cx).font.family.is_none());
        view.update(cx, |view, cx| {
            for _ in 0..50 {
                view.dispatch_command(CommandId::IncreaseFontSize, window, cx);
            }
            assert_eq!(view.font.size, 40);
        });
        window.render_frame(cx);
        assert!(window.find("grid-cell:0:0").bounds().size.height > px(40.));
        view.update(cx, |view, cx| {
            for _ in 0..50 {
                view.dispatch_command(CommandId::DecreaseFontSize, window, cx);
            }
            assert_eq!(view.font.size, 7);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let saved = repository.load_settings().unwrap();
    assert_eq!(saved.font_size, 7);
    assert!(saved.font_family.is_none());
    cx.update(|cx| {
        assert_eq!(view.read(cx).font, other.read(cx).font);
        assert!(!view.read(cx).session.dirty);
        cx.set_global(FontRuntime::default());
    });
    let (_, restarted) = viewer_with_repository(cx, repository);
    cx.update(|cx| assert_eq!(restarted.read(cx).font.size, 7));
}

#[gpui_kit::test]
fn editor_font_follows_its_picker_and_the_grid_size(cx: &mut TestAppContext) {
    use super::{FontRuntime, FontTarget};
    use gpui_kit::component::ActiveTheme;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.run_until_parked();
    let default_mono = cx.update(|cx| cx.theme().mono_font_family.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            let mut font = view.font.clone();
            font.editor_family = Some("Test Mono".into());
            font.size = 17;
            view.save_font(font, cx);
        });
        // Every code editor reads the theme's monospace font.
        assert_eq!(cx.theme().mono_font_family.as_ref(), "Test Mono");
        assert_eq!(cx.theme().mono_font_size, px(17.));
        assert!(view.read(cx).font.family.is_none());
        view.update(cx, |view, cx| {
            view.open_font_picker(FontTarget::Editor, window, cx)
        });
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        window.input("default", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(view.read(cx).font.editor_family.is_none());
        assert_eq!(cx.theme().mono_font_family, default_mono);
    });
    let saved = repository.load_settings().unwrap();
    assert_eq!(
        (saved.font_size, saved.editor_font_family.clone()),
        (17, None)
    );

    let mut settings = saved;
    settings.editor_font_family = Some("Saved Mono".into());
    repository.save_settings(&settings).unwrap();
    cx.update(|cx| cx.set_global(FontRuntime::default()));
    let (_, restarted) = viewer_with_repository(cx, repository);
    cx.update(|cx| {
        assert_eq!(
            restarted.read(cx).font.editor_family.as_deref(),
            Some("Saved Mono")
        );
        assert_eq!(cx.theme().mono_font_family.as_ref(), "Saved Mono");
    });
}

#[gpui_kit::test]
fn clear_filters_button_clears_search_and_all_cards_preserving_sort(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("clear-filters").is_none());
        view.update(cx, |view, cx| {
            view.add_slicer("region".into(), cx);
            view.add_slicer("sales".into(), cx);
            view.session.set_sort_direction("sales".into(), true);
            view.set_search_text("north".into(), window, cx);
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-value:region:0", cx);
        window.render_frame(cx);
        window.click("slicer-value:sales:0", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .iter()
                .all(|card| !card.selected.is_empty())
        );
        window.click("clear-filters", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("clear-filters").is_none());
        let session = &view.read(cx).session;
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
        assert_eq!(&*session.result.row_ids, &[1, 0]);
    })
    .unwrap();
}

#[gpui_kit::test]
fn file_picker_matches_terms_across_name_and_path_and_archives_selected_identity(
    cx: &mut TestAppContext,
) {
    use crate::domain::documents::DocumentDefinition;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut first = DocumentDefinition::draft_table("First report".into(), "east.csv".into());
    let mut second = DocumentDefinition::draft_table("Second report".into(), "west.csv".into());
    repository.save_view(&mut first).unwrap();
    repository.save_view(&mut second).unwrap();
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.update(|cx| {
        cx.bind_keys([gpui_kit::KeyBinding::new(
            "ctrl-delete",
            super::ArchiveSelectedView,
            Some("Command"),
        )])
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_file_palette(window, cx));
        window.render_frame(cx);
        window.input("  WEST\tSECOND  ", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let rows = window.within("command");
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(0))
                .is_some()
        );
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(1))
                .is_none()
        );
        window.press("ctrl-delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(repository.load_views().0.len(), 1);
    assert_eq!(repository.load_views().0[0].id, first.id);
    assert_eq!(repository.load_archives().0[0].1.view.id, second.id);
}

#[gpui_kit::test]
fn file_and_font_pickers_match_command_palette_width(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    for font in [false, true] {
        cx.simulate_window_resize(handle.into(), size(px(1400.), px(900.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            view.update(cx, |view, cx| {
                if font {
                    view.open_font_picker(super::FontTarget::Grid, window, cx);
                } else {
                    view.open_file_palette(window, cx);
                }
            });
            window.render_frame(cx);
            assert_eq!(
                window.within("dialog").find(0usize).bounds().size.width,
                px(896.)
            );
        })
        .unwrap();
        cx.simulate_window_resize(handle.into(), size(px(700.), px(600.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let bounds = window.within("dialog").find(0usize).bounds();
            assert!(bounds.size.width < px(700.));
            assert!(bounds.origin.x >= px(0.));
            assert!(bounds.right() <= px(700.));
            window.close_dialog(cx);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn file_picker_empty_query_escape_and_filtered_source_confirmation(cx: &mut TestAppContext) {
    use crate::application::state::ApplicationState;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("West report.csv");
    std::fs::write(&path, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let other_path = home.path().join("East report.csv");
    std::fs::write(&other_path, "region,sales\nNorth,30\n").unwrap();
    let mut app = ApplicationState::load(repository.clone()).unwrap();
    app.record_source(&path).unwrap();
    app.record_source(&other_path).unwrap();
    let (handle, view) = viewer_with_repository(cx, repository);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_file_palette(window, cx));
        window.render_frame(cx);
        window.input("west nonexistent", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(0).section(2))
                .is_none()
        );
        window.press("enter", cx);
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(1).section(2))
                .is_some()
        );
        window.input("SOURCE west REPORT", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let index = gpui_kit::component::IndexPath::new(0).section(2);
        assert!(window.within("command").try_find(index).is_some());
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(1).section(2))
                .is_none()
        );
        window.within("command").click(index, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!window.has_active_dialog(cx));
        let session = &view.read(cx).session;
        assert!(
            session
                .source_path
                .as_ref()
                .unwrap()
                .ends_with("West report.csv")
        );
        assert_eq!(session.result.row_count(), 2);
        assert!(matches!(
            session.identity,
            crate::application::session::SessionIdentity::DraftTable
        ));
    })
    .unwrap();
}

#[gpui_kit::test]
fn save_dialog_focuses_name_for_shortcut_and_palette(cx: &mut TestAppContext) {
    for from_palette in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let source = home.path().join("source.csv");
        std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
        let (handle, view) = viewer_with_document(
            cx,
            repository.clone(),
            Some(source.to_string_lossy().into_owned()),
            None,
        );
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let focus = if from_palette {
                view.read(cx).grid.read(cx).focus_handle(cx)
            } else {
                view.read(cx).search_input.read(cx).focus_handle(cx)
            };
            window.focus(&focus, cx);
            if from_palette {
                window.press("ctrl-shift-p", cx);
                window.render_frame(cx);
                window.input("save view", cx);
            } else {
                window.press("ctrl-s", cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            if from_palette {
                window.press("enter", cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.press("ctrl-a", cx);
            window.input("Keyboard saved view", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                !window.has_active_dialog(cx),
                "from_palette={from_palette}, saved={:?}, error={:?}",
                repository.load_views().0,
                view.read(cx).error
            );
            assert!(view.read(cx).session.search_text.is_empty());
            assert!(!view.read(cx).session.dirty);
            let (saved, issues) = repository.load_views();
            assert!(issues.is_empty());
            assert_eq!(saved.len(), 1);
            assert_eq!(saved[0].name, "Keyboard saved view");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn settings_directory_command_reports_an_unusable_active_profile(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository);
    let blocked = home.path().join("blocked");
    std::fs::write(&blocked, "blocked").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, _| view.repository.paths.config = blocked.clone());
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("settings directory", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let rows = window.within("command");
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(0))
                .is_some()
        );
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(1))
                .is_none()
        );
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!window.has_active_dialog(cx));
        let error = view.read(cx).error.as_deref().unwrap();
        assert!(error.starts_with("Cannot open settings directory"));
        assert!(error.contains(&blocked.display().to_string()));
        assert_eq!(std::fs::read_to_string(&blocked).unwrap(), "blocked");
    })
    .unwrap();
}

#[gpui_kit::test]
fn command_palette_add_folder_opens_directory_picker_and_saves_selection(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("add folder", cx);
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
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories);
        assert!(!options.files);
        Some(vec![folder.path().to_path_buf()])
    });
    cx.run_until_parked();
    assert_eq!(
        repository.load_settings().unwrap().watched_folders,
        [folder
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()]
    );
}

#[gpui_kit::test]
fn command_palette_filters_all_terms_and_confirms_the_filtered_command(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("SHIFT keymap", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let rows = window.within("command");
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(0))
                .is_some()
        );
        assert!(
            rows.try_find(gpui_kit::component::IndexPath::new(1))
                .is_none()
        );
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).keymap_editor.is_some());
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn command_palette_empty_matches_and_escape_restore_rows(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("keymap nonexistent", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(0))
                .is_none()
        );
        window.press("enter", cx);
        assert!(window.has_active_dialog(cx));
        assert!(view.read(cx).keymap_editor.is_none());
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("command")
                .try_find(gpui_kit::component::IndexPath::new(1))
                .is_some()
        );
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn command_palette_doubles_width_and_fits_smaller_windows(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.simulate_window_resize(handle.into(), size(px(1400.), px(900.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        assert_eq!(
            window.within("dialog").find(0usize).bounds().size.width,
            px(896.)
        );
    })
    .unwrap();
    cx.simulate_window_resize(handle.into(), size(px(700.), px(600.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.within("dialog").find(0usize).bounds();
        assert!(bounds.size.width < px(700.));
        assert!(bounds.origin.x >= px(0.));
        assert!(bounds.origin.x + bounds.size.width <= px(700.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn search_down_and_tab_focus_grid(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        let search_focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&grid_focus));
        window.focus(&grid_focus, cx);
        window.press("/", cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&search_focus));
        window.press("down", cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&grid_focus));
        window.focus(&search_focus, cx);
        window.press("tab", cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&grid_focus));
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_clears_active_search_without_leaving_search(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            view.set_search_text("North".into(), window, cx);
        });
        let search_focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&search_focus, cx);
        window.press("escape", cx);

        assert!(view.read(cx).search_input.read(cx).value().is_empty());
        assert!(view.read(cx).search_text().is_empty());
        assert_eq!(window.focused(cx).as_ref(), Some(&search_focus));
    })
    .unwrap();
}

#[gpui_kit::test]
fn typing_a_quote_in_search_offers_columns_and_tab_or_enter_inserts_them(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    let step = |cx: &mut TestAppContext,
                action: &dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App)| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            action(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            (
                view.search_input.read(cx).value().to_string(),
                view.search_completion.read(cx).items(),
            )
        })
    };
    let search = view.clone();
    step(cx, &|window, cx| {
        let focus = search.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    let (_, items) = step(cx, &|window, cx| window.input("\"", cx));
    assert_eq!(items, Some(vec!["region".into(), "sales".into()]));
    let (_, items) = step(cx, &|window, cx| window.input("S", cx));
    assert_eq!(items, Some(vec!["sales".into()]));
    let (text, items) = step(cx, &|window, cx| window.press("tab", cx));
    assert_eq!(text, "\"sales\"");
    assert!(items.is_none());
    cx.update_window(handle.into(), |_, window, cx| {
        let focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&focus));
    })
    .unwrap();
    // After an operator a quote names the column to compare against.
    step(cx, &|window, cx| window.input(">\"", cx));
    step(cx, &|window, cx| window.press("down", cx));
    let (text, _) = step(cx, &|window, cx| window.press("enter", cx));
    assert_eq!(text, "\"sales\">\"sales\"");
    // Escape closes the list first and keeps the text.
    let (_, items) = step(cx, &|window, cx| window.input(" \"", cx));
    assert!(items.is_some());
    let (text, items) = step(cx, &|window, cx| window.press("escape", cx));
    assert!(items.is_none());
    assert!(text.starts_with("\"sales\">\"sales\" \""), "{text}");
}

#[gpui_kit::test]
fn right_clicking_a_sorted_header_clears_only_that_sort(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.session.set_sort_direction("region".into(), false);
            view.session.set_sort_direction("sales".into(), true);
            let sort = view.session.sort.clone();
            view.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
        });
        window.render_frame(cx);
        window.right_click("grid-header:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        assert_eq!(
            view.read(cx)
                .session
                .sort
                .iter()
                .map(|key| key.column.as_str())
                .collect::<Vec<_>>(),
            ["sales"]
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_q_closes_the_focused_window(cx: &mut TestAppContext) {
    let (handle, _) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-q", cx);
    })
    .unwrap();
    assert!(cx.update_window(handle.into(), |_, _, _| {}).is_err());
}

fn window_view(
    cx: &mut TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
) -> gpui_kit::Entity<DataFrameView> {
    cx.update_window(handle, |root, _, cx| {
        root.downcast::<Root>()
            .ok()
            .expect("window root")
            .read(cx)
            .view()
            .clone()
            .downcast::<DataFrameView>()
            .ok()
            .expect("dataframe view")
    })
    .unwrap()
}

#[gpui_kit::test]
fn new_window_shortcut_opens_blank_windows_from_grid_and_search(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (original, view) = viewer_with_repository(cx, repository.clone());
    cx.run_until_parked();
    for search_focused in [false, true] {
        cx.update_window(original.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.session.set_sort_direction("sales".into(), true);
                view.set_search_text("north".into(), window, cx);
                view.session.dirty = true;
            });
            let focus = if search_focused {
                view.read(cx).search_input.read(cx).focus_handle(cx)
            } else {
                view.read(cx).grid.read(cx).focus_handle(cx)
            };
            window.render_frame(cx);
            window.focus(&focus, cx);
            window.press("ctrl-n", cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    let windows = cx.windows();
    assert_eq!(windows.len(), 3);
    for handle in windows
        .into_iter()
        .filter(|handle| *handle != original.into())
    {
        let blank = window_view(cx, handle);
        cx.update(|cx| {
            let blank = blank.read(cx);
            assert_eq!(blank.repository.paths.config, repository.paths.config);
            assert_eq!(blank.repository.paths.data, repository.paths.data);
            assert!(blank.session.source_path.is_none());
            assert!(blank.session.working.is_none());
            assert!(!blank.session.dirty);
            assert!(blank.session.search_text.is_empty());
            assert!(blank.session.sort.is_empty());
            assert_eq!(blank.session.source.row_count(), 0);
            assert_eq!(blank.session.result.row_count(), 0);
            assert!(!std::sync::Arc::ptr_eq(
                &blank.session.source.store,
                &view.read(cx).session.source.store,
            ));
            assert!(blank.window_geometry.is_none());
        });
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.viewport_size(), size(px(1400.), px(900.)));
            assert_eq!(
                window.focused(cx),
                Some(blank.read(cx).grid.read(cx).focus_handle(cx))
            );
        })
        .unwrap();
    }
    cx.update(|cx| {
        assert_eq!(view.read(cx).session.source.row_count(), 2);
        assert_eq!(view.read(cx).session.search_text, "north");
        assert!(view.read(cx).session.dirty);
    });
}

#[gpui_kit::test]
fn new_window_palette_command_opens_a_blank_window(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let (original, view) =
        viewer_with_repository(cx, ViewRepository::new(StorePaths::for_home(home.path())));
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.open_command_palette(window, cx));
        window.render_frame(cx);
        window.input("new window", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(original.into(), |_, window, cx| {
        window.render_frame(cx);
        let matches = super::matching_commands(
            &super::active_commands(),
            &view.read(cx).keymap,
            "new window",
        );
        // Drill down in new window matches too, after New window.
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].0.id, super::CommandId::NewWindow);
        assert_eq!(matches[0].1, "New window · ctrl-n");
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(original.into(), |_, window, cx| {
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    assert_eq!(cx.windows().len(), 2);
    let new_handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != original.into())
        .unwrap();
    let blank = window_view(cx, new_handle);
    cx.update(|cx| assert_eq!(blank.read(cx).session.source.row_count(), 0));
}

#[gpui_kit::test]
fn new_window_respects_disabled_and_remapped_shortcuts_and_dialogs(cx: &mut TestAppContext) {
    use crate::application::keymap::{ActionId, DisabledDefault, KeyContext, UserBinding};
    let home = tempfile::tempdir().unwrap();
    let (original, view) =
        viewer_with_repository(cx, ViewRepository::new(StorePaths::for_home(home.path())));
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, _| {
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
        window.render_frame(cx);
        window.press("ctrl-n", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.open_information(window, cx));
        window.render_frame(cx);
        window.press("ctrl-shift-n", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    cx.update_window(original.into(), |_, window, cx| {
        window.close_dialog(cx);
        view.update(cx, |view, cx| view.open_keymap_editor(window, cx));
        window.render_frame(cx);
        window.press("ctrl-shift-n", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, _| view.keymap_editor = None);
        let focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        window.render_frame(cx);
        window.press("ctrl-shift-n", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
}

#[gpui_kit::test]
fn new_windows_navigate_independently_and_survive_closing_original(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let source = home.path().join("other.csv");
    std::fs::write(&source, "name,amount\nFirst,30\nSecond,40\nThird,50\n").unwrap();
    let (original, view) = viewer_with_repository(cx, repository);
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.dispatch_command(super::CommandId::NewWindow, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let other_handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != original.into())
        .unwrap();
    let other = window_view(cx, other_handle);
    cx.update_window(other_handle, |_, window, cx| {
        other.update(cx, |view, cx| {
            view.load_source(
                source.clone(),
                false,
                super::LoadMode::Check,
                window.window_handle(),
                cx,
            );
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| {
        assert!(other.read(cx).error.is_none(), "{:?}", other.read(cx).error);
        assert_eq!(other.read(cx).session.source.row_count(), 3);
        assert_eq!(view.read(cx).session.source.row_count(), 2);
        assert!(view.read(cx).session.source_path.is_none());
    });
    cx.update_window(original.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-q", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows(), vec![other_handle]);
    cx.update_window(other_handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-n", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    cx.update_window(other_handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-q", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    let survivor = window_view(cx, cx.windows()[0]);
    cx.update(|cx| assert_eq!(survivor.read(cx).session.source.row_count(), 0));
}

#[gpui_kit::test]
fn new_window_closing_saves_preferences_without_changing_primary_placement(
    cx: &mut TestAppContext,
) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (original, view) = viewer_with_repository(cx, repository.clone());
    cx.update_window(original.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.remember_window(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let placement = repository.load_history().unwrap().primary_window;
    for close_method in ["native", "shortcut", "release"] {
        cx.update(|cx| view.update(cx, |view, cx| view.open_blank_window(None, cx)));
        cx.run_until_parked();
        let other_handle = cx
            .windows()
            .into_iter()
            .find(|handle| *handle != original.into())
            .unwrap();
        let other = window_view(cx, other_handle);
        let source = home.path().join(format!("{close_method}.csv"));
        cx.update(|cx| {
            other.update(cx, |view, _| {
                view.session.source_path = Some(source.to_string_lossy().into_owned());
                view.session.direct_table.source_path = source.to_string_lossy().into_owned();
                view.session.set_column_width("Row".into(), 180.);
            });
        });
        if close_method == "native" {
            let mut visual = gpui_kit::VisualTestContext::from_window(other_handle, cx);
            assert!(visual.simulate_close());
        }
        // Release coverage must not retain the view outside its window.
        drop(other);
        cx.update_window(other_handle, |_, window, cx| {
            if close_method == "shortcut" {
                window.render_frame(cx);
                window.press("ctrl-q", cx);
            } else {
                window.remove_window();
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| view.read(cx).table_writer.flush().unwrap());
        assert_eq!(
            repository.load_table_preferences(&source).unwrap().widths["Row"],
            180.
        );
        assert_eq!(repository.load_history().unwrap().primary_window, placement);
        assert_eq!(cx.windows(), vec![original.into()]);
    }
}

#[gpui_kit::test]
fn closing_window_flushes_latest_size_without_waiting_for_debounce(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.repository = repository.clone();
            view.remember_window(window, cx);
        });
    })
    .unwrap();
    cx.simulate_window_resize(handle.into(), size(px(1100.), px(700.)));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).window_geometry.as_ref().unwrap().width,
            1100.0
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-q", cx);
    })
    .unwrap();
    drop(view);
    cx.run_until_parked();
    let saved = repository.load_history().unwrap().primary_window.unwrap();
    assert_eq!(saved.width, 1100.0);
    assert_eq!(saved.height, 700.0);
}

#[gpui_kit::test]
fn startup_restores_saved_position_and_size(cx: &mut TestAppContext) {
    use super::ViewerOptions;
    use crate::infrastructure::json_store::WindowGeometry;
    cx.update(gpui_kit::init);
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let saved = WindowGeometry {
        x: 120.0,
        y: 60.0,
        width: 950.0,
        height: 650.0,
        maximized: false,
    };
    repository.save_window_geometry(&saved).unwrap();
    let frame = FrameSnapshot::new(DuckDbStore::synthetic(10).unwrap(), 1);
    cx.update(|cx| {
        DataFrameView::open(
            frame,
            None,
            ViewerOptions {
                repository: repository.clone(),
                ..Default::default()
            },
            cx,
        )
    });
    cx.run_until_parked();
    let handle = cx.update(|cx| cx.windows()[0]);
    cx.update_window(handle, |_, window, _| {
        assert_eq!(
            super::window_placement::geometry(window.window_bounds()),
            saved
        );
        window.remove_window();
    })
    .unwrap();
}

#[gpui_kit::test]
fn info_shortcut_opens_dialog(cx: &mut TestAppContext) {
    let (handle, _) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("i", cx);
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn info_profiles_every_source_row_while_search_narrows_the_grid(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_search_text("North".into(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(view.read(cx).session.result.row_count(), 1);
        view.update(cx, |view, cx| view.open_information(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = view
            .read(cx)
            .information_panel
            .as_ref()
            .and_then(|panel| panel.upgrade())
            .expect("information panel");
        let panel = panel.read(cx);
        assert_eq!(
            panel.row_text("region", cx).unwrap(),
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
            panel.row_text("sales", cx).unwrap()[2..7],
            ["0", "0.0%", "2", "10", "20"]
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn help_font_changes_and_repeated_scrollbar_drags_keep_track_fixed(cx: &mut TestAppContext) {
    use gpui_kit::{
        InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        point,
    };
    let home = tempfile::tempdir().unwrap();
    let (handle, view) =
        viewer_with_repository(cx, ViewRepository::new(StorePaths::for_home(home.path())));
    cx.simulate_window_resize(handle.into(), size(px(800.), px(600.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_help(window, cx));
        window.render_frame(cx);
        let key_height = window.find("help-key-Navigation-0").bounds().size.height;
        let description_height = window
            .find("help-description-Navigation-0")
            .bounds()
            .size
            .height;
        view.update(cx, |view, cx| view.change_font_size(9, cx));
        window.render_frame(cx);
        assert!(window.find("help-key-Navigation-0").bounds().size.height > key_height);
        assert!(
            window
                .find("help-description-Navigation-0")
                .bounds()
                .size
                .height
                > description_height
        );

        let track = window.find("help-viewport").bounds();
        let mut from = point(track.right() - px(5.), track.top() + px(12.));
        for travel in [60., 60., -90.] {
            let before = window.find("help-section-Navigation").bounds().origin.y;
            window.dispatch_event(
                MouseDownEvent {
                    position: from,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            let to = point(from.x, from.y + px(travel));
            window.dispatch_event(
                MouseMoveEvent {
                    position: to,
                    modifiers: Modifiers::default(),
                    pressed_button: Some(MouseButton::Left),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseUpEvent {
                    position: to,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            let after = window.find("help-section-Navigation").bounds().origin.y;
            assert!(
                if travel > 0. {
                    after < before
                } else {
                    after > before
                },
                "drag should follow the thumb"
            );
            assert_eq!(window.find("help-viewport").bounds(), track);
            from = to;
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn help_columns_resize_align_scroll_and_dismiss(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, point};

    let home = tempfile::tempdir().unwrap();
    let (handle, view) =
        viewer_with_repository(cx, ViewRepository::new(StorePaths::for_home(home.path())));
    cx.simulate_window_resize(handle.into(), size(px(1920.), px(1080.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("f1", cx);
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();

    for (width, height, columns) in [(1920., 1080., 3), (1200., 800., 2), (800., 600., 1)] {
        cx.simulate_window_resize(handle.into(), size(px(width), px(height)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let dialog = window.within("dialog").find(0usize).bounds();
            assert_eq!(dialog.size.width, px((width - 48.).min(1500.)));
            assert!(dialog.origin.x >= px(24.));
            assert!(dialog.right() <= px(width - 24.));
            let close = window.find("close").bounds();
            assert!(close.right() >= dialog.right() - px(40.));
            assert!(close.right() < dialog.right());
            let body = window.find("help-body").bounds();
            assert!(body.size.height <= px(height - 140.));
            for column in 0..columns {
                let bounds = window.find(format!("help-column-{column}")).bounds();
                assert!(bounds.origin.x >= dialog.origin.x);
                assert!(bounds.right() <= dialog.right());
            }
            assert!(window.try_find(format!("help-column-{columns}")).is_none());
            let key = window.find("help-key-Navigation-0").bounds();
            let description = window.find("help-description-Navigation-0").bounds();
            for row in 1..4 {
                let next_key = window.find(format!("help-key-Navigation-{row}")).bounds();
                let next_description = window
                    .find(format!("help-description-Navigation-{row}"))
                    .bounds();
                assert_eq!(next_key.origin.x, key.origin.x);
                assert_eq!(next_key.size.width, key.size.width);
                assert_eq!(next_description.origin.x, description.origin.x);
                assert!(next_description.origin.x > next_key.right());
            }
            if columns == 1 {
                let before = window.find("help-section-FilesViews").bounds().origin.y;
                window.scroll(
                    "help-body",
                    ScrollDelta::Pixels(point(px(0.), px(-600.))),
                    cx,
                );
                window.render_frame(cx);
                assert!(window.find("help-section-FilesViews").bounds().origin.y < before);
            }
        })
        .unwrap();
    }

    cx.update_window(handle.into(), |_, window, cx| {
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        view.update(cx, |view, cx| {
            view.change_font_size(9, cx);
            view.open_help(window, cx);
        });
        window.render_frame(cx);
        let body = window.find("help-body").bounds();
        assert!(body.size.height <= px(460.));
        window.click("close", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn information_sql_is_readonly_selectable_copyable_and_escape_restores_grid(
    cx: &mut TestAppContext,
) {
    use crate::ui::info::InformationPanel;
    use gpui_kit::ParentElement;
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    let sql = cx.update(|cx| view.read(cx).session.sql.text());
    assert!(sql.contains("Result search and sort"));
    let mut panel = None;
    cx.update_window(handle.into(), |_, window, cx| {
        let entity = cx.new(|cx| {
            InformationPanel::new(
                vec![("n".into(), "BIGINT".into())],
                1,
                String::new(),
                sql.clone(),
                window,
                cx,
            )
        });
        panel = Some(entity.clone());
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.content({
                let entity = entity.clone();
                move |content, _, _| content.child(entity.clone())
            })
        });
        window.render_frame(cx);
        let editor = panel.as_ref().unwrap().read(cx).editor.clone();
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        window.press("ctrl-a", cx);
        window.input("DELETE", cx);
        window.press("backspace", cx);
        assert_eq!(editor.read(cx).value().to_string(), sql);
        window.press("ctrl-c", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(sql.clone())
        );
        window.click("information-copy-sql", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(sql.clone())
        );
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        assert!(
            view.read(cx)
                .grid
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn pivot_sql_cache_and_errors_follow_the_published_result(cx: &mut TestAppContext) {
    use crate::domain::documents::{
        Aggregation, AggregationOp, Condition, ConditionOp, ConditionValue, ScalarKey,
    };
    use crate::domain::sql::SqlStatus;
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
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
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.session.sql.displayed.statements.len(), 4);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.set_search_text("10".into(), window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let previous = cx.update(|cx| {
        let state = view.read(cx);
        assert_eq!(state.session.sql.displayed.statements.len(), 4);
        assert!(
            state.session.sql.displayed.statements[..3]
                .iter()
                .all(|s| s.status == SqlStatus::Cached)
        );
        assert_eq!(
            state.session.sql.displayed.statements[3].status,
            SqlStatus::Succeeded
        );
        state.session.result.result_revision
    });
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| {
            let ViewKind::Query(config) = &mut view.session.working.as_mut().unwrap().view else {
                unreachable!()
            };
            config.pivot_by = vec!["missing".into()];
            view.schedule_search(String::new(), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.error.is_some());
        assert_eq!(state.session.result.result_revision, previous);
        assert_eq!(state.session.sql.displayed.statements.len(), 4);
        assert!(state.session.sql.failed.is_some());
    });
}

#[gpui_kit::test]
fn header_keeps_only_contextual_clear_sort(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "files",
            "order-by",
            "favorites",
            "information",
            "keymap-editor",
            "help",
            "save-view",
            "clear-sort",
        ] {
            assert!(
                window.try_find(id).is_none(),
                "unexpected header button {id}"
            );
        }
        view.update(cx, |view, cx| {
            view.session.set_sort_direction("sales".into(), true);
            let sort = view.session.sort.clone();
            view.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("clear-sort").is_some());
        window.click("clear-sort", cx);
        assert!(view.read(cx).session.sort.is_empty());
        window.render_frame(cx);
        assert!(window.try_find("clear-sort").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn space_menu_is_scoped_to_grid_and_unknown_keys_leave_it_open(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let search_focus = view.read(cx).search_input.read(cx).focus_handle(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&search_focus, cx);
        window.press("space", cx);
        assert!(!view.read(cx).space_menu_open);

        window.focus(&grid_focus, cx);
        window.press("space", cx);
        assert!(view.read(cx).space_menu_open);
        window.render_frame(cx);
        window.press("q", cx);
        assert!(view.read(cx).space_menu_open);
        window.press("escape", cx);
        assert!(!view.read(cx).space_menu_open);
    })
    .unwrap();
}

#[gpui_kit::test]
fn grid_copy_freeze_and_save_shortcuts_respect_focus(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("shortcuts.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let (handle, view) = viewer_with_document(
        cx,
        ViewRepository::new(StorePaths::for_home(home.path())),
        Some(source.to_string_lossy().into_owned()),
        None,
    );
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let search = view.read(cx).search_input.clone();
        window.focus(&search.read(cx).focus_handle(cx), cx);
        window.input("cf", cx);
        assert_eq!(search.read(cx).value().as_str(), "cf");
        assert_eq!(
            view.read(cx).session.effective_table_config().frozen_count,
            0
        );
        assert!(view.read(cx).copy_status.is_none());
        view.update(cx, |view, cx| {
            view.set_search_text(String::new(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("right", cx);
        window.press("c", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("10".into())
        );
        window.press("f", cx);
        assert_eq!(
            view.read(cx).session.effective_table_config().frozen_count,
            1
        );
        assert!(!view.read(cx).space_menu_open);
        window.press("f", cx);
        assert_eq!(
            view.read(cx).session.effective_table_config().frozen_count,
            0
        );
        window.press("ctrl-s", cx);
        assert!(window.has_active_dialog(cx));
        window.render_frame(cx);
        assert!(window.try_find("save-confirm").is_some());
        window.click("save-confirm", cx);
        window.render_frame(cx);
        assert!(!window.has_active_dialog(cx), "{:?}", view.read(cx).error);
        assert!(!view.read(cx).session.dirty);
        assert_eq!(view.read(cx).repository.load_views().0.len(), 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn control_shift_digits_assign_all_favorite_slots(cx: &mut TestAppContext) {
    use crate::application::state::ApplicationState;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("favorite.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let source_path = source.to_string_lossy().into_owned();
    let (handle, view) =
        viewer_with_document(cx, repository.clone(), Some(source_path.clone()), None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        for digit in 1..=9 {
            window.press(&format!("ctrl-shift-{digit}"), cx);
            assert!(view.read(cx).error.is_none(), "{:?}", view.read(cx).error);
            assert_eq!(
                ApplicationState::load(repository.clone())
                    .unwrap()
                    .settings
                    .favorites[digit - 1],
                Some(crate::infrastructure::json_store::FavoriteTarget::Source(
                    source_path.clone()
                ))
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn changed_source_refreshes_all_open_windows_and_pivots(cx: &mut TestAppContext) {
    use crate::domain::documents::{DocumentDefinition, QueryConfig};
    use crate::infrastructure::source_database::{LoadMode, SourceDatabase};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("sales.csv");
    std::fs::write(&source, "region,sales\nNorth,10\nSouth,20\n").unwrap();
    let source_path = source.to_string_lossy().into_owned();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let database = SourceDatabase::for_profile(&repository.paths.config);
    let frame = FrameSnapshot::new(database.load(&source, LoadMode::Check).unwrap(), 1);
    let (_, table) = viewer_with_document(cx, repository.clone(), Some(source_path.clone()), None);
    let mut definition = DocumentDefinition::draft_table("By region".into(), source_path.clone());
    definition.view = ViewKind::Query(QueryConfig {
        group_by: vec!["region".into()],
        ..Default::default()
    });
    let (_, query) = viewer_with_document(cx, repository, Some(source_path), Some(definition));
    cx.update(|cx| {
        table.update(cx, |view, cx| view.replace_dataframe(frame.clone(), cx));
        query.update(cx, |view, cx| view.replace_dataframe(frame, cx));
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(table.read(cx).session.result.row_count(), 2);
        assert_eq!(query.read(cx).session.result.row_count(), 2);
    });
    std::fs::write(
        &source,
        "region,sales,extra\nEast,100,new\nEast,200,new\nWest,300,new\n",
    )
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|cx| {
        for view in [&table, &query] {
            let view = view.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.session.source.row_count(), 3);
            assert_eq!(view.session.source.columns.len(), 3);
            assert_eq!(view.session.source.store.lock().unwrap().table_sql(), "files.\"sales_csv\"");
        }
        assert_eq!(table.read(cx).session.result.row_count(), 3);
        let view = query.read(cx);
        assert_eq!(view.session.result.row_count(), 2);
        let store = view.session.result.store.lock().unwrap();
        let rows = store.fetch_row_ids(&store.initial_row_ids().unwrap()).unwrap();
        assert_eq!(rows.iter().map(|row| row.values[0].as_deref()).collect::<Vec<_>>(), [Some("East"), Some("West")]);
        assert!(matches!(&view.session.working.as_ref().unwrap().view, ViewKind::Query(config) if config.group_by == [crate::domain::documents::Dimension::from("region")]));
    });
}

#[gpui_kit::test]
fn space_menu_table_commands_sort_and_unfreeze(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("space", cx);
        window.press("]", cx);
        assert!(!view.read(cx).space_menu_open);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(view.read(cx).session.sort[0].column, "region");
        assert!(view.read(cx).session.sort[0].descending);

        window.press("space", cx);
        window.press("f", cx);
        let frozen = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Table(table) = &definition.view {
                    Some(table.frozen_count)
                } else {
                    None
                }
            });
        assert_eq!(frozen, Some(1));

        window.press("space", cx);
        window.press("shift-f", cx);
        let frozen = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Table(table) = &definition.view {
                    Some(table.frozen_count)
                } else {
                    None
                }
            });
        assert_eq!(frozen, Some(0));
    })
    .unwrap();
}

#[gpui_kit::test]
fn space_menu_opens_group_pivot_and_aggregation_workflows(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);

        window.press("space", cx);
        assert!(view.read(cx).space_menu_open);
        window.render_frame(cx);
        window.press("g", cx);
        assert!(!view.read(cx).space_menu_open);
        assert!(window.has_active_dialog(cx));
        window.render_frame(cx);
        window.input("region", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let group_by = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Query(config) = &definition.view {
                    Some(config.group_by.clone())
                } else {
                    None
                }
            });
        assert_eq!(group_by, Some(vec!["region".into()]));
        assert!(view.read(cx).error.is_none());
        assert!(window.has_active_dialog(cx));
        window.close_dialog(cx);

        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("p", cx);
        window.render_frame(cx);
        window.input("sales", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let pivot_by = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Query(config) = &definition.view {
                    Some(config.pivot_by.clone())
                } else {
                    None
                }
            });
        assert_eq!(pivot_by, Some(vec!["sales".into()]));
        assert!(window.has_active_dialog(cx));
        window.close_dialog(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("a", cx);
        window.render_frame(cx);
        window.input("sales", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let aggregations = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Query(config) = &definition.view {
                    Some(config.aggregations.clone())
                } else {
                    None
                }
            });
        let aggregations = aggregations.unwrap();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(
            aggregations[0].operation,
            crate::domain::documents::AggregationOp::Sum
        );
        assert_eq!(aggregations[0].name, None);
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn group_right_pane_reorders_and_removes_selected_field(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("g", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.close_dialog(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("g", cx);
        window.render_frame(cx);
        window.input("sales", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.has_active_dialog(cx));
        window.close_dialog(cx);
        let grid_focus = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        window.press("g", cx);
        window.render_frame(cx);
        window.press("tab", cx);
        window.press("alt-down", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let group_by = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Query(config) = &definition.view {
                    Some(config.group_by.clone())
                } else {
                    None
                }
            });
        assert_eq!(group_by, Some(vec!["sales".into(), "region".into()]));
        window.press("delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        let group_by = view
            .read(cx)
            .session
            .working
            .as_ref()
            .and_then(|definition| {
                if let ViewKind::Query(config) = &definition.view {
                    Some(config.group_by.clone())
                } else {
                    None
                }
            });
        assert_eq!(group_by, Some(vec!["sales".into()]));
    })
    .unwrap();
}

#[gpui_kit::test]
fn aggregation_right_pane_edits_and_removes_selected_measure(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.open_aggregation_panel(window, cx);
        });
        window.render_frame(cx);
        window.input("sales", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.has_active_dialog(cx));
        let aggregation = view.read(cx).session.query_config().aggregations[0].clone();
        assert_eq!(
            aggregation.operation,
            crate::domain::documents::AggregationOp::Sum
        );
        window.render_frame(cx);
        window.press("tab", cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find("aggregation-apply").is_some());
        assert!(window.try_find("condition-add").is_none());
        window.press("delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).session.query_config().aggregations.is_empty());
        assert!(view.read(cx).is_table());
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

fn aggregation_editor(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<DataFrameView>,
    gpui_kit::Entity<crate::ui::aggregation_editor::AggregationPanel>,
) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    let panel = cx
        .update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_aggregation_panel(window, cx).unwrap()
            })
        })
        .unwrap();
    (handle, view, panel)
}

fn set_aggregation_sql(
    panel: &gpui_kit::Entity<crate::ui::aggregation_editor::AggregationPanel>,
    sql: &str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) {
    let state = panel.read(cx).sql.read(cx).state().clone();
    state.update(cx, |state, cx| state.set_value(sql, window, cx));
}

#[gpui_kit::test]
fn aggregation_function_reference_inserts_a_call_into_custom_sql(cx: &mut TestAppContext) {
    let (handle, view, panel) = aggregation_editor(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("aggregation-show-functions", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // Descriptions are searchable as well as names.
        window.input("deviation", cx);
        window.render_frame(cx);
        assert!(window.try_find("aggregation-sql").is_none());
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let state = panel.read(cx).sql.read(cx).state().clone();
        assert_eq!(state.read(cx).value().as_ref(), "");
        // Clear the query and look the function up by name; `median` comes
        // before `mad`, whose description mentions the median.
        for _ in 0.."deviation".len() {
            window.press("backspace", cx);
        }
        window.input("median", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        let state = panel.read(cx).sql.read(cx).state().clone();
        assert_eq!(state.read(cx).value().as_ref(), "median()");
        // The cursor waits between the parentheses for a column.
        window.input("\"sales\"", cx);
        assert_eq!(state.read(cx).value().as_ref(), "median(\"sales\")");
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let aggregations = view.read(cx).session.query_config().aggregations.clone();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(aggregations[0].sql.as_deref(), Some("median(\"sales\")"));
    });
}

#[gpui_kit::test]
fn aggregation_sql_fills_its_area_and_scrolls_multiline_content(cx: &mut TestAppContext) {
    let (handle, _, panel) = aggregation_editor(cx);
    let sql = (0..40).map(|_| "-- SQL line\n").collect::<String>() + "count(*)";
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
    })
    .unwrap();
    for (width, height) in [(1400., 900.), (1000., 700.)] {
        cx.simulate_window_resize(handle.into(), size(px(width), px(height)));
        cx.update_window(handle.into(), |_, window, cx| {
            set_aggregation_sql(&panel, &sql, window, cx);
            window.render_frame(cx);
            let state = panel.read(cx).sql.read(cx).state().clone();
            let editor = window.find(("input", state.entity_id())).bounds();
            let expected_height = (height - 560_f32).clamp(70., 220.);
            assert!((px(expected_height) - editor.size.height).abs() <= px(2.));
            let apply = window.find("aggregation-apply").bounds();
            let before = state.read(cx).scroll_offset();
            window.scroll(
                ("input", state.entity_id()),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
                cx,
            );
            assert!(state.read(cx).scroll_offset().y < before.y);
            assert_eq!(window.find("aggregation-apply").bounds(), apply);
            assert_eq!(state.read(cx).value().as_ref(), sql);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn aggregation_sql_applies_with_ctrl_enter_and_preserves_success_on_errors(
    cx: &mut TestAppContext,
) {
    let (handle, view, panel) = aggregation_editor(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("aggregation-custom", cx);
        window.render_frame(cx);
        window.input("count(*) AS Rows", cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let successful = cx.update(|cx| {
        let view = view.read(cx);
        assert!(
            !view.session.query_config().aggregations.is_empty(),
            "panel error: {:?}; SQL: {}; view error: {:?}",
            panel.read(cx).error,
            panel.read(cx).sql.read(cx).state().read(cx).value(),
            view.error
        );
        assert_eq!(
            view.session.query_config().aggregations[0].sql.as_deref(),
            Some("count(*) AS Rows")
        );
        assert!(
            view.session.query_config().aggregations[0]
                .source_column
                .is_empty()
        );
        assert_eq!(view.session.result.columns[0].0, "Rows");
        assert_eq!(view.session.result.row_count(), 1);
        view.session.result.clone()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        set_aggregation_sql(&panel, "sum(\"missing\")", window, cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(panel.read(cx).error.as_ref().unwrap().contains("missing"));
        assert_eq!(
            view.read(cx).session.query_config().aggregations[0]
                .sql
                .as_deref(),
            Some("count(*) AS Rows")
        );
        assert!(std::sync::Arc::ptr_eq(
            &view.read(cx).session.result.store,
            &successful.store
        ));
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "sum(\"missing\")"
        );
        window.render_frame(cx);
        window.click("aggregation-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(panel.read(cx).error.is_some());
        assert_eq!(view.read(cx).session.query_config().aggregations.len(), 1);
        window.render_frame(cx);
        window.click("aggregation-revert", cx);
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "count(*) AS Rows"
        );
        set_aggregation_sql(&panel, "sum(\"sales\")", window, cx);
        panel.update(cx, |panel, cx| panel.focus_sql(window, cx));
        window.render_frame(cx);
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        let reopened = view.update(cx, |view, cx| {
            view.open_aggregation_panel(window, cx).unwrap()
        });
        assert_eq!(
            reopened
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "count(*) AS Rows"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn aggregation_selection_applies_sql_aliases_and_reorders_them(cx: &mut TestAppContext) {
    let (handle, view, panel) = aggregation_editor(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("sales", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(view.read(cx).session.result.columns[0].0, "sum(sales)");
        set_aggregation_sql(&panel, "sum(\"sales\") AS Revenue", window, cx);
        window.render_frame(cx);
        window.click("aggregation-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let saved = view.read(cx).session.query_config().aggregations[0].clone();
        assert!(saved.name.is_none());
        assert_eq!(saved.sql.as_deref(), Some("sum(\"sales\") AS Revenue"));
        assert_eq!(view.read(cx).session.result.columns[0].0, "Revenue");
        set_aggregation_sql(&panel, "count(*)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(view.read(cx).session.query_config().aggregations.len(), 2);
        set_aggregation_sql(&panel, "avg(\"sales\")", window, cx);
        window.render_frame(cx);
        window.click("aggregation-0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            view.read(cx).session.query_config().aggregations[1]
                .sql
                .as_deref(),
            Some("avg(\"sales\")")
        );
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "sum(\"sales\") AS Revenue"
        );
        set_aggregation_sql(&panel, "sum(\"missing\")", window, cx);
        window.render_frame(cx);
        window.click("aggregation-1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(panel.read(cx).error.is_some());
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "sum(\"missing\")"
        );
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        window.render_frame(cx);
        window.press("alt-down", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        assert_eq!(
            view.read(cx).session.query_config().aggregations[1]
                .sql
                .as_deref(),
            Some("sum(\"sales\") AS Revenue")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn aggregation_invalid_selection_and_stale_checks_cannot_change_applied_config(
    cx: &mut TestAppContext,
) {
    let (handle, view, panel) = aggregation_editor(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_aggregation_sql(&panel, "count(*)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        set_aggregation_sql(&panel, "count(*) + 1", window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).session.query_config().aggregations.is_empty());
        assert!(panel.read(cx).error.as_ref().unwrap().contains("changed"));
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_aggregation_sql(&panel, "count(*)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_aggregation_sql(&panel, "sum(\"sales\")", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            view.read(cx).session.query_config().aggregations[0]
                .sql
                .as_deref(),
            Some("sum(\"sales\")")
        );
        assert!(panel.read(cx).error.is_none());
        set_aggregation_sql(&panel, "sum(\"missing\")", window, cx);
        window.render_frame(cx);
        window.click("aggregation-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(panel.read(cx).error.is_some());
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "sum(\"missing\")"
        );
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        set_aggregation_sql(&panel, "count(*)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        panel.update(cx, |panel, _| panel.close());
        window.close_dialog(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).session.query_config().aggregations[0]
                .sql
                .as_deref(),
            Some("sum(\"sales\")")
        );
    });
}

#[gpui_kit::test]
fn aggregation_column_addition_applies_pending_custom_sql_and_pivot_filter_is_inline(
    cx: &mut TestAppContext,
) {
    use crate::domain::documents::QueryConfig;
    let (handle, view, panel) = aggregation_editor(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_aggregation_sql(&panel, "count(*)", window, cx);
        panel.update(cx, |panel, cx| panel.focus_available(window, cx));
        window.render_frame(cx);
        window.input("region", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let config = view.read(cx).session.query_config();
        assert_eq!(config.aggregations.len(), 2);
        assert_eq!(config.aggregations[0].sql.as_deref(), Some("count(*)"));
        assert_eq!(
            config.aggregations[1].operation,
            crate::domain::documents::AggregationOp::NonNullCount
        );
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "count(\"region\")"
        );
        assert!(window.has_active_dialog(cx));
        window.close_dialog(cx);
        view.update(cx, |view, cx| {
            view.session
                .set_query_config(
                    QueryConfig {
                        group_by: vec!["region".into()],
                        pivot_by: vec!["sales".into()],
                        aggregations: vec![crate::domain::documents::Aggregation::sql_expression(
                            "count(*)".into(),
                            None,
                        )],
                        ..Default::default()
                    },
                    "Pivot".into(),
                )
                .unwrap();
            view.schedule_search(String::new(), cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let panel = view.update(cx, |view, cx| {
            view.open_aggregation_panel(window, cx).unwrap()
        });
        set_aggregation_sql(&panel, "count(*) FILTER (WHERE true)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        assert!(
            panel
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("DuckDB ignores FILTER")
        );
        assert_eq!(
            view.read(cx).session.query_config().aggregations[0]
                .sql
                .as_deref(),
            Some("count(*)")
        );
        window.render_frame(cx);
        assert!(window.try_find("aggregation-limitations").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn query_limit_commits_globally_preserves_dirty_state_and_reuses_search_cache(
    cx: &mut TestAppContext,
) {
    use crate::domain::documents::QueryConfig;
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (first_handle, first) = viewer_with_repository(cx, repository.clone());
    let (second_handle, second) = viewer_with_repository(cx, repository.clone());
    for (handle, entity) in [(first_handle, &first), (second_handle, &second)] {
        cx.update_window(handle.into(), |_, _, cx| {
            entity.update(cx, |view, cx| {
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
            })
        })
        .unwrap();
    }
    cx.run_until_parked();
    cx.update_window(first_handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(first_handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("query-limit").is_some());
        let input = first.read(cx).limit_input.clone();
        window.focus(&input.read(cx).focus_handle(cx), cx);
        window.press("ctrl-a", cx);
        window.input("1", cx);
        assert_eq!(first.read(cx).query_limit, 100000);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    for (handle, entity) in [(first_handle, &first), (second_handle, &second)] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let view = entity.read(cx);
            assert_eq!(view.query_limit, 1);
            assert_eq!(view.limit_input.read(cx).value().as_ref(), "1");
            assert_eq!(view.session.result.row_count(), 1);
            assert!(!view.session.dirty);
        })
        .unwrap();
    }
    assert_eq!(repository.load_settings().unwrap().query_limit, 1);
    let cached = cx.update(|cx| {
        first
            .read(cx)
            .query_cache
            .as_ref()
            .unwrap()
            .frame
            .store
            .clone()
    });
    cx.update_window(first_handle.into(), |_, window, cx| {
        first.update(cx, |view, cx| {
            view.set_search_text("South".into(), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = first.read(cx);
        // The cached result stopped at the limit of 1, so the search reran
        // inside the query and found South past it; the cache is unchanged.
        assert_eq!(view.session.result.row_count(), 1);
        assert!(Arc::ptr_eq(
            &cached,
            &view.query_cache.as_ref().unwrap().frame.store
        ));
        assert!(
            view.session.sql.displayed.statements[0].status
                == crate::domain::sql::SqlStatus::Cached
        );
    });
    cx.update_window(first_handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let limit = first.read(cx).limit_input.read(cx).focus_handle(cx);
        window.focus(&limit, cx);
        window.press("ctrl-a", cx);
        window.input("2", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(first_handle.into(), |_, window, cx| {
        let grid = first.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = first.read(cx);
        assert_eq!(view.query_limit, 2);
        assert_eq!(view.session.result.row_count(), 1);
        assert!(!Arc::ptr_eq(
            &cached,
            &view.query_cache.as_ref().unwrap().frame.store
        ));
        assert!(!view.session.dirty);
    });
}

#[gpui_kit::test]
fn query_limit_invalid_escape_failed_save_and_keyboard_focus(cx: &mut TestAppContext) {
    use crate::domain::documents::QueryConfig;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, entity) = viewer_with_repository(cx, repository.clone());
    cx.update_window(handle.into(), |_, _, cx| {
        entity.update(cx, |view, cx| {
            view.session
                .set_query_config(
                    QueryConfig {
                        group_by: vec!["region".into()],
                        ..Default::default()
                    },
                    "Query".into(),
                )
                .unwrap();
            view.schedule_search(String::new(), cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let search = entity.read(cx).search_input.read(cx).focus_handle(cx);
        let limit = entity.read(cx).limit_input.read(cx).focus_handle(cx);
        window.focus(&search, cx);
        window.press("tab", cx);
        assert_eq!(window.focused(cx), Some(limit.clone()));
        window.press("ctrl-a", cx);
        window.input("0", cx);
        window.press("enter", cx);
        assert_eq!(entity.read(cx).query_limit, 100000);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let search = entity.read(cx).search_input.read(cx).focus_handle(cx);
        let limit = entity.read(cx).limit_input.read(cx).focus_handle(cx);
        let grid = entity.read(cx).grid.read(cx).focus_handle(cx);
        assert!(
            entity.read(cx).limit_error.is_some(),
            "text={:?} committed={} mode={:?}",
            entity.read(cx).limit_input.read(cx).value(),
            entity.read(cx).query_limit,
            entity.read(cx).session.working
        );
        window.press("escape", cx);
        assert_eq!(
            entity.read(cx).limit_input.read(cx).value().as_ref(),
            "100000"
        );
        assert!(entity.read(cx).limit_error.is_none());
        window.press("ctrl-a", cx);
        window.input("gap i/ ", cx);
        assert!(!window.has_active_dialog(cx));
        assert!(!entity.read(cx).space_menu_open);
        window.press("escape", cx);
        window.press("shift-tab", cx);
        assert_eq!(window.focused(cx), Some(search));
        window.press("tab", cx);
        window.press("tab", cx);
        assert_eq!(window.focused(cx), Some(grid));
        window.press("shift-tab", cx);
        assert_eq!(window.focused(cx), Some(limit));
        let blocked = home.path().join("blocked-config");
        std::fs::write(&blocked, "blocked").unwrap();
        entity.update(cx, |view, cx| {
            view.repository.paths.config = blocked;
            view.limit_input
                .update(cx, |input, cx| input.set_value("1", window, cx));
            view.commit_query_limit(window, cx);
        });
        assert_eq!(entity.read(cx).query_limit, 100000);
        assert!(
            entity
                .read(cx)
                .limit_error
                .as_ref()
                .unwrap()
                .contains("Could not save")
        );
        assert_eq!(entity.read(cx).session.result.row_count(), 2);
        assert_eq!(repository.load_settings().unwrap().query_limit, 100000);
    })
    .unwrap();
}

#[gpui_kit::test]
fn empty_query_and_pivot_setup_keep_table_controls_and_avoid_sql(cx: &mut TestAppContext) {
    use crate::domain::documents::{QueryConfig, SlicerDefinition};
    let (handle, entity) = viewer(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.open_pivot_panel(
                crate::application::pivot_editor::PivotFieldKind::GroupBy,
                window,
                cx,
            );
        });
        assert!(entity.read(cx).is_table());
        window.close_dialog(cx);
        entity.update(cx, |view, cx| {
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
        window.render_frame(cx);
        assert!(window.try_find("query-limit").is_some());
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!entity.read(cx).busy);
        assert!(entity.read(cx).error.is_none());
        entity.update(cx, |view, cx| {
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
        window.render_frame(cx);
        assert!(entity.read(cx).is_table());
        assert!(window.try_find("query-limit").is_none());
        assert_eq!(
            entity
                .read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .len(),
            1
        );
    })
    .unwrap();
}

fn dimension_editor(
    cx: &mut TestAppContext,
    kind: crate::application::pivot_editor::PivotFieldKind,
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<DataFrameView>,
    gpui_kit::Entity<crate::ui::pivot_panel::PivotPanel>,
) {
    let (handle, view) = viewer(cx);
    cx.run_until_parked();
    let panel = cx
        .update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_pivot_panel(kind, window, cx))
        })
        .unwrap();
    (handle, view, panel)
}

fn set_dimension_sql(
    panel: &gpui_kit::Entity<crate::ui::pivot_panel::PivotPanel>,
    sql: &str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) {
    let state = panel.read(cx).sql.read(cx).state().clone();
    state.update(cx, |state, cx| state.set_value(sql, window, cx));
}

#[gpui_kit::test]
fn dimension_expressions_apply_in_both_managers_and_errors_preserve_results(
    cx: &mut TestAppContext,
) {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let (handle, view, group) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("dimension-custom", cx);
        window.render_frame(cx);
        window.input("upper(region) AS RegionCode", cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let successful = cx
        .update_window(handle.into(), |_, window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(group.read(cx).error.is_none(), "{:?}", group.read(cx).error);
            assert_eq!(
                view.read(cx).session.query_config().group_by,
                [Dimension::expression("upper(region) AS RegionCode")]
            );
            assert_eq!(view.read(cx).session.result.columns[0].0, "RegionCode");
            view.read(cx).session.result.clone()
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        set_dimension_sql(&group, "upper(missing)", window, cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(group.read(cx).error.as_ref().unwrap().contains("missing"));
        assert!(std::sync::Arc::ptr_eq(
            &view.read(cx).session.result.store,
            &successful.store
        ));
        window.render_frame(cx);
        window.click("dimension-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let pivot = cx
        .update_window(handle.into(), |_, window, cx| {
            assert!(group.read(cx).error.is_some());
            assert_eq!(
                group
                    .read(cx)
                    .sql
                    .read(cx)
                    .state()
                    .read(cx)
                    .value()
                    .as_ref(),
                "upper(missing)"
            );
            group.update(cx, |panel, cx| panel.revert(window, cx));
            assert_eq!(
                group
                    .read(cx)
                    .sql
                    .read(cx)
                    .state()
                    .read(cx)
                    .value()
                    .as_ref(),
                "upper(region) AS RegionCode"
            );
            window.close_dialog(cx);
            view.update(cx, |view, cx| {
                view.open_pivot_panel(PivotFieldKind::PivotBy, window, cx)
            })
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        pivot.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&pivot, "sales / 10 AS Bucket", window, cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(pivot.read(cx).error.is_none(), "{:?}", pivot.read(cx).error);
        assert!(window.has_active_dialog(cx));
        assert_eq!(
            view.read(cx).session.query_config().pivot_by,
            [Dimension::expression("sales / 10 AS Bucket")]
        );
        assert_eq!(view.read(cx).session.result.columns[0].0, "RegionCode");
        assert!(view.read(cx).session.result.columns.len() > 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn dimension_column_edits_aliases_reorder_focus_and_return_to_table(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("region", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "\"region\""
        );
        set_dimension_sql(&panel, "upper(region) AS Code", window, cx);
        window.render_frame(cx);
        window.click("dimension-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            view.read(cx).session.query_config().group_by,
            [Dimension::expression("upper(region) AS Code")]
        );
        set_dimension_sql(&panel, "sales AS Amount", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("dimension-0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "upper(region) AS Code"
        );
        window.render_frame(cx);
        window.press("tab", cx);
        assert_eq!(
            window.focused(cx),
            Some(
                panel
                    .read(cx)
                    .sql
                    .read(cx)
                    .state()
                    .read(cx)
                    .focus_handle(cx)
            )
        );
        window.press("shift-tab", cx);
        window.press("alt-down", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            view.read(cx).session.query_config().group_by,
            [
                Dimension::expression("sales AS Amount"),
                Dimension::expression("upper(region) AS Code")
            ]
        );
        window.press("delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(view.read(cx).session.query_config().group_by.len(), 1);
        window.press("delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).is_table());
        assert!(view.read(cx).session.query_config().group_by.is_empty());
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn dimension_pending_setup_validates_and_then_builds_a_pivot(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::PivotBy);
    let previous = cx.update(|cx| view.read(cx).session.result.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "sum(sales) AS Bad", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).session.query_config().pivot_by.is_empty());
        assert!(panel.read(cx).error.is_some());
        set_dimension_sql(&panel, "sales / 10 AS Bucket", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    let group = cx
        .update_window(handle.into(), |_, window, cx| {
            assert!(panel.read(cx).error.is_none(), "{:?}", panel.read(cx).error);
            assert_eq!(
                view.read(cx).session.query_config().pivot_by,
                [Dimension::expression("sales / 10 AS Bucket")]
            );
            assert!(std::sync::Arc::ptr_eq(
                &view.read(cx).session.result.store,
                &previous.store
            ));
            assert!(!view.read(cx).busy);
            window.close_dialog(cx);
            view.update(cx, |view, cx| {
                view.open_pivot_panel(PivotFieldKind::GroupBy, window, cx)
            })
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        group.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&group, "upper(region) AS Code", window, cx);
        group.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        assert!(group.read(cx).error.is_none(), "{:?}", group.read(cx).error);
        assert_eq!(view.read(cx).session.result.columns[0].0, "Code");
        assert!(view.read(cx).session.result.columns.len() > 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn dimension_stale_and_closed_checks_cannot_commit(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "upper(region)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        set_dimension_sql(&panel, "lower(region)", window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).session.query_config().group_by.is_empty());
        assert!(panel.read(cx).error.as_ref().unwrap().contains("changed"));
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "upper(region)", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
        window.render_frame(cx);
        window.click("dimension-close", cx);
        assert!(!window.has_active_dialog(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(view.read(cx).session.query_config().group_by.is_empty());
    });
}

#[gpui_kit::test]
fn dimension_pending_column_addition_and_duplicate_output_names(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::documents::Dimension;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "upper(region) AS Code", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "lower(region) AS code", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(
            panel
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("duplicate dimension output name")
        );
        assert_eq!(view.read(cx).session.query_config().group_by.len(), 1);
        panel.update(cx, |panel, cx| panel.revert(window, cx));
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "sales % 10 AS Bucket", window, cx);
        panel.update(cx, |panel, cx| panel.focus_available(window, cx));
        window.render_frame(cx);
        window.input("region", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(panel.read(cx).error.is_none(), "{:?}", panel.read(cx).error);
        assert_eq!(
            view.read(cx).session.query_config().group_by,
            [
                Dimension::expression("upper(region) AS Code"),
                Dimension::expression("sales % 10 AS Bucket"),
                Dimension::from("region")
            ]
        );
        assert_eq!(
            panel
                .read(cx)
                .sql
                .read(cx)
                .state()
                .read(cx)
                .value()
                .as_ref(),
            "\"region\""
        );
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn expression_group_can_be_removed_from_its_result_column(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    use crate::domain::selection::Cell;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "upper(region) AS Code", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
        let grid = view.read(cx).grid.clone();
        grid.update(cx, |grid, cx| {
            grid.select(Cell { row: 0, column: 0 }, false, cx)
        });
        view.update(cx, |view, cx| view.remove_active_group_column(cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(view.read(cx).session.query_config().group_by.is_empty());
        assert!(view.read(cx).is_table());
    });
}

#[gpui_kit::test]
fn dimension_check_rejects_session_changes(cx: &mut TestAppContext) {
    use crate::application::pivot_editor::PivotFieldKind;
    let (handle, view, panel) = dimension_editor(cx, PivotFieldKind::GroupBy);
    cx.update_window(handle.into(), |_, window, cx| {
        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
        set_dimension_sql(&panel, "upper(region) AS Code", window, cx);
        panel.update(cx, |panel, cx| panel.apply(cx));
    })
    .unwrap();
    cx.update_window(handle.into(), |_, _, cx| {
        view.update(cx, |view, cx| view.schedule_search("10".into(), cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(view.read(cx).session.query_config().group_by.is_empty());
        assert!(panel.read(cx).error.as_ref().unwrap().contains("changed"));
    });
}

#[gpui_kit::test]
fn ctrl_l_cycles_slicer_inputs_and_ctrl_d_returns_to_the_grid(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.simulate_window_resize(handle.into(), size(px(1400.), px(900.)));
    cx.update_window(handle.into(), |_, _, cx| {
        let source = DuckDbStore::from_rows(
            &[
                ("region".into(), "VARCHAR".into()),
                ("sales".into(), "BIGINT".into()),
                ("flag".into(), "BOOLEAN".into()),
            ],
            [
                vec![
                    Value::Text("North".into()),
                    Value::BigInt(10),
                    Value::Boolean(true),
                ],
                vec![
                    Value::Text("South".into()),
                    Value::BigInt(20),
                    Value::Boolean(false),
                ],
            ],
        )
        .unwrap();
        view.update(cx, |view, cx| {
            view.replace_dataframe(FrameSnapshot::new(source, 2), cx);
            view.add_slicer("region".into(), cx);
            view.add_condition_slicer("sales".into(), cx);
            view.add_slicer("flag".into(), cx);
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    // Inputs only report blur, which commits a condition, in an active window.
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    // From the grid, the first card's value filter opens and takes the typing.
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid, cx);
        window.render_frame(cx);
        window.press("ctrl-l", cx);
        window.render_frame(cx);
        window.input("No", cx);
        window.render_frame(cx);
        assert!(window.try_find("slicer-value:region:0").is_some());
        assert!(window.try_find("slicer-value:region:1").is_none());
        // The condition card is next; moving on commits its text.
        window.press("ctrl-l", cx);
        window.render_frame(cx);
        window.input(">15", cx);
        window.press("ctrl-l", cx);
        window.render_frame(cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| assert_eq!(view.read(cx).session.result.row_count(), 1));
    cx.update_window(handle.into(), |_, window, cx| {
        // After the last card, focus wraps to the first, whose filter text stays.
        window.render_frame(cx);
        window.press("ctrl-l", cx);
        window.render_frame(cx);
        assert!(window.try_find("slicer-value:region:0").is_some());
        assert!(window.try_find("slicer-value:region:1").is_none());
        window.press("ctrl-d", cx);
        let grid = view.read(cx).grid.read(cx).focus_handle(cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&grid));
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_l_without_slicers_reports_how_to_add_one(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-l", cx);
        let grid = view.read(cx).grid.read(cx).focus_handle(cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&grid));
        assert_eq!(
            view.read(cx).copy_status.as_deref(),
            Some("No slicers to focus. Use Add slicer to add one")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn undo_steps_back_slicer_selections_and_cards_but_not_search_box_typing(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        let source = DuckDbStore::from_rows(
            &[("region".into(), "VARCHAR".into())],
            [
                vec![Value::Text("North".into())],
                vec![Value::Text("South".into())],
            ],
        )
        .unwrap();
        view.update(cx, |view, cx| {
            view.replace_dataframe(FrameSnapshot::new(source, 2), cx)
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| view.update(cx, |view, cx| view.add_slicer("region".into(), cx)));
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-value:region:1", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    let state = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = view.read(cx);
            let cards = view
                .session
                .working
                .as_ref()
                .map_or(0, |definition| definition.slicers.len());
            (cards, view.session.result.row_count())
        })
    };
    assert_eq!(state(cx), (1, 1));
    // In the search box, ctrl-z undoes typing rather than the view.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("/", cx);
        window.render_frame(cx);
        window.input("x", cx);
        window.press("ctrl-z", cx);
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| assert_eq!(view.read(cx).session.search_text, ""));
    assert_eq!(state(cx), (1, 1));
    let undo = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("ctrl-d", cx);
            window.render_frame(cx);
            window.press("ctrl-z", cx);
        })
        .unwrap();
        finish_slicer_work(cx);
    };
    undo(cx);
    assert_eq!(state(cx), (1, 2));
    undo(cx);
    assert_eq!(state(cx), (0, 2));
}

#[gpui_kit::test]
fn ctrl_f_focuses_search_from_the_grid_and_from_slicer_inputs(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    cx.update_window(handle.into(), |_, _, cx| {
        let source = DuckDbStore::from_rows(
            &[("sales".into(), "BIGINT".into())],
            [vec![Value::BigInt(10)], vec![Value::BigInt(20)]],
        )
        .unwrap();
        view.update(cx, |view, cx| {
            view.replace_dataframe(FrameSnapshot::new(source, 2), cx)
        });
    })
    .unwrap();
    finish_slicer_work(cx);
    cx.update(|cx| view.update(cx, |view, cx| view.add_condition_slicer("sales".into(), cx)));
    finish_slicer_work(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        let search = view.read(cx).search_input.read(cx).focus_handle(cx);
        let grid = view.read(cx).grid.read(cx).focus_handle(cx);
        window.focus(&grid, cx);
        window.render_frame(cx);
        window.press("ctrl-f", cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&search));
        window.press("ctrl-l", cx);
        window.render_frame(cx);
        assert_ne!(window.focused(cx).as_ref(), Some(&search));
        window.press("ctrl-f", cx);
        assert_eq!(window.focused(cx).as_ref(), Some(&search));
    })
    .unwrap();
}

#[gpui_kit::test]
fn aggregation_sql_editor_lists_columns_after_a_quote_above_the_dialog(cx: &mut TestAppContext) {
    let (handle, view) = viewer(cx);
    let (sql, state) = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let panel = view
                .update(cx, |view, cx| view.open_aggregation_panel(window, cx))
                .unwrap();
            panel.update(cx, |panel, cx| panel.start_custom(window, cx));
            let sql = panel.read(cx).sql.clone();
            let state = sql.read(cx).state().clone();
            state.update(cx, |state, cx| {
                state.set_value("sum(", window, cx);
                state.set_selected_range(4..4, cx);
                state.focus(window, cx);
            });
            (sql, state)
        })
        .unwrap();
    cx.run_until_parked();
    let menu = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let menu = state.read(cx).completion_menu_state();
            menu.open.then(|| {
                menu.items
                    .iter()
                    .map(|item| item.label.clone())
                    .collect::<Vec<_>>()
            })
        })
    };
    let press = |cx: &mut TestAppContext, key: &str| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    let input = |cx: &mut TestAppContext, text: &str| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    input(cx, "\"");
    assert_eq!(menu(cx), Some(vec!["region".into(), "sales".into()]));
    // The kit draws its own menu below dialogs, so the editor draws one above.
    assert!(cx.update(|cx| sql.read(cx).completion_menu(cx).is_some()));
    // Down then Enter picks the second column; the dialog stays open.
    press(cx, "down");
    press(cx, "enter");
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "sum(\"sales\"");
    assert!(menu(cx).is_none());
    cx.update_window(handle.into(), |_, window, cx| assert!(window.has_active_dialog(cx)))
        .unwrap();
    // Escape closes the list but not the dialog; Tab accepts.
    input(cx, " + \"re");
    assert_eq!(menu(cx), Some(vec!["region".into()]));
    press(cx, "escape");
    assert!(menu(cx).is_none());
    cx.update_window(handle.into(), |_, window, cx| assert!(window.has_active_dialog(cx)))
        .unwrap();
    input(cx, "g");
    press(cx, "tab");
    assert_eq!(
        cx.update(|cx| state.read(cx).value().to_string()),
        "sum(\"sales\" + \"region\""
    );
}
