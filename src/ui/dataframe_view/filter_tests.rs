use super::{DataFrameView, FileEntry};
use crate::application::commands::CommandId;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, QueryConfig, ValueFilter, ViewKind,
};
use crate::domain::frame::FrameSnapshot;
use crate::domain::selection::Cell;
use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use gpui_kit::component::{Root, WindowExt as _};
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

/// Focuses the grid, selects `cell`, and presses `key` there.
fn press_on_cell(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entity: &Entity<DataFrameView>,
    cell: Cell,
    key: &str,
) {
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        grid.update(cx, |grid, cx| grid.select(cell, false, cx));
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn column_values(entity: &Entity<DataFrameView>, cx: &mut TestAppContext) -> Vec<Option<String>> {
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        let result = &view.session.result;
        result
            .fetch_rows(0, result.row_count())
            .unwrap()
            .into_iter()
            .map(|row| row.values[result.column_indices[0]].clone())
            .collect()
    })
}

#[gpui_kit::test]
fn value_keys_filter_exclude_null_cells_and_clear(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\nNorth,30\n,40\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    assert_eq!(column_values(&entity, cx).len(), 4);

    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "=");
    assert_eq!(
        column_values(&entity, cx),
        [Some("North".into()), Some("North".into())]
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let view = entity.read(cx);
        assert_eq!(view.session.value_filters()[0].label(), "region = North");
        assert!(view.session.has_filters());
        entity.update(cx, |view, cx| view.remove_value_filter(0, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(column_values(&entity, cx).len(), 4);

    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "-");
    assert_eq!(column_values(&entity, cx), [Some("South".into()), None]);
    press_on_cell(cx, handle, &entity, Cell { row: 1, column: 0 }, "=");
    assert_eq!(column_values(&entity, cx), [None]);
    cx.update(|cx| {
        assert_eq!(
            entity.read(cx).session.value_filters()[1],
            ValueFilter {
                target: "region".into(),
                value: None,
                exclude: false,
            }
        )
    });

    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::ClearFilters, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(column_values(&entity, cx).len(), 4);
    cx.update(|cx| assert!(entity.read(cx).session.value_filters().is_empty()));
}

#[gpui_kit::test]
fn enter_drills_from_pivot_cells_to_source_rows_and_back_returns(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(
        &path,
        "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        pivot_by: vec![Dimension::Column("kind".into())],
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
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.result.columns[1].0, "a_total");
    });

    // A pivoted measure drills into both the group and the pivot keys.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 1 }, "enter");
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(
            view.session
                .value_filters()
                .iter()
                .map(ValueFilter::label)
                .collect::<Vec<_>>(),
            ["region = North", "kind = a"]
        );
        assert_eq!(view.session.result.row_count(), 2);
    });

    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.press("u", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 2);
    });

    // A group-by cell drills into every row of the group.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "enter");
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
    });
}

#[gpui_kit::test]
fn space_t_adds_row_totals_and_a_grand_total_footer(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(
        &path,
        "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        pivot_by: vec![Dimension::Column("kind".into())],
        aggregations: vec![Aggregation {
            source_column: "n".into(),
            operation: AggregationOp::Mean,
            name: None,
            sql: None,
            conditions: Vec::new(),
            percentile: None,
            interpolation: None,
            ddof: None,
        }],
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(pivot));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "space");
    cx.update_window(handle.into(), |_, window, cx| window.press("t", cx))
        .unwrap();
    cx.run_until_parked();
    let state = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            let names = view
                .session
                .result
                .columns
                .iter()
                .map(|column| column.0.clone())
                .collect::<Vec<_>>();
            (names, view.grid.read(cx).footer().cloned())
        })
    };
    let (names, footer) = state(cx);
    assert_eq!(names, ["region", "a", "b", "(Total)"]);
    // North's mean over its three rows is 15, not the mean of its cells (7.5, 30).
    let north_total = cx.update(|cx| {
        let result = entity.read(cx).session.result.clone();
        let row = result.fetch_rows(0, 1).unwrap().remove(0);
        row.values[result.column_indices[3]].clone()
    });
    assert_eq!(north_total.as_deref(), Some("15.0"));
    let footer = footer.expect("grand total row");
    assert_eq!(footer.label, "Total");
    let value = |name: &str| {
        footer
            .values
            .iter()
            .find(|(column, _)| column == name)
            .and_then(|(_, value)| value.clone())
    };
    assert_eq!(value("a").as_deref(), Some("11.666666666666666"));
    assert_eq!(value("b").as_deref(), Some("30.0"));
    assert_eq!(value("(Total)").as_deref(), Some("16.25"));

    let region_width = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            entity.read(cx).grid.read(cx).column_width("region")
        })
        .unwrap()
    };
    let narrow = region_width(cx);

    // Searching narrows the rows, not the totals, and the label says so. The
    // longer label widens its column.
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.set_search_text("south".into(), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    let (_, footer) = state(cx);
    assert_eq!(footer.unwrap().label, "Total (all groups)");
    let wide = region_width(cx);
    assert!(wide > narrow, "{wide} > {narrow}");
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.set_search_text(String::new(), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    // Widths only grow on their own; recalculating fits the short label again.
    assert_eq!(region_width(cx), wide);
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::RecalculateWidths, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    region_width(cx);
    cx.run_until_parked();
    assert_eq!(region_width(cx), narrow);

    // A row total drills into every row of its group.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 3 }, "enter");
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
        assert!(view.grid.read(cx).footer().is_none());
    });
}

#[gpui_kit::test]
fn value_keys_on_pivot_group_cells_filter_the_source(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\nEast,5\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    let mut query = DocumentDefinition::draft_table("Query".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        aggregations: vec![Aggregation::sql_expression(
            "sum(n)".into(),
            Some("total".into()),
        )],
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(query));
    assert_eq!(column_values(&entity, cx).len(), 3);
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "-");
    assert_eq!(column_values(&entity, cx).len(), 2);
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 1 }, "=");
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.as_deref().unwrap().contains("group-by"));
        assert_eq!(view.session.value_filters().len(), 1);
        assert!(view.session.dirty);
    });
}

#[gpui_kit::test]
fn jump_to_column_moves_the_cursor_to_the_chosen_column(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("wide.csv");
    std::fs::write(&path, "alpha,beta,gamma,delta\n1,2,3,4\n5,6,7,8\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        grid.update(cx, |grid, cx| {
            grid.select(Cell { row: 1, column: 0 }, false, cx)
        });
        window.focus(&grid.read(cx).focus_handle(cx), cx);
        window.render_frame(cx);
        window.press("ctrl-g", cx);
        window.render_frame(cx);
        window.input("gamm", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        let active = grid.read(cx).selection().unwrap().active;
        assert_eq!(active, Cell { row: 1, column: 2 });
        assert!(grid.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn drill_down_from_a_database_view_query_keeps_the_view_source(cx: &mut TestAppContext) {
    use crate::application::session::SessionIdentity;
    use crate::domain::documents::DatabaseView;
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let (handle, entity) = viewer(cx, repository);
    cx.update(|cx| {
        entity
            .read(cx)
            .database
            .connect()
            .unwrap()
            .execute_batch(
                "CREATE VIEW main.sales AS SELECT range AS n, range % 3 AS category FROM range(30)",
            )
            .unwrap()
    });
    let mut query = DocumentDefinition::draft_database_view(DatabaseView::view("main", "sales"));
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("category".into())],
        aggregations: vec![Aggregation::sql_expression(
            "count(*)".into(),
            Some("rows".into()),
        )],
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(query));
    assert_eq!(column_values(&entity, cx).len(), 3);
    press_on_cell(cx, handle, &entity, Cell { row: 1, column: 1 }, "enter");
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert_eq!(view.session.value_filters()[0].label(), "category = 1");
        assert_eq!(view.session.result.row_count(), 10);
    });
}

#[gpui_kit::test]
fn value_filters_grey_out_slicer_values_without_matching_rows(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("colors.csv");
    std::fs::write(&path, "region,color\nEast,Red\nEast,Blue\nWest,Red\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    cx.update(|cx| entity.update(cx, |view, cx| view.add_slicer("color".into(), cx)));
    cx.run_until_parked();
    let available = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            let card = &view.slicer_stage.as_ref().unwrap().cards[0];
            card.values
                .iter()
                .map(|value| (value.label.clone(), value.available))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        available(cx),
        [("Blue".to_owned(), true), ("Red".to_owned(), true)]
    );
    // Excluding East leaves only West's Red row.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "-");
    assert_eq!(
        available(cx),
        [("Blue".to_owned(), false), ("Red".to_owned(), true)]
    );
}

#[gpui_kit::test]
fn slicer_card_filter_narrows_values_and_enter_selects_them(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("cities.csv");
    std::fs::write(
        &path,
        "city\nAustin\nBoston\nDallas\nHouston\nParis\nAtlanta\nDenver\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    cx.update(|cx| entity.update(cx, |view, cx| view.add_slicer("city".into(), cx)));
    cx.run_until_parked();
    // Values sort as Atlanta, Austin, Boston, Dallas, Denver, Houston, Paris.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("slicer-value:city:0").is_some());
        window.click("slicer-search:city", cx);
        window.input("TON", cx);
        window.render_frame(cx);
        assert!(window.try_find("slicer-value:city:0").is_none());
        assert!(window.try_find("slicer-value:city:2").is_some());
        assert!(window.try_find("slicer-value:city:5").is_some());
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        column_values(&entity, cx),
        [Some("Boston".into()), Some("Houston".into())]
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("slicer-value:city:0").is_some());
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(column_values(&entity, cx).len(), 2);
}

#[gpui_kit::test]
fn truncated_query_results_warn_and_search_past_the_limit(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let path = home.path().join("years.csv");
    std::fs::write(&path, "year,n\n2000,1\n2004,2\n2008,3\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let mut query = DocumentDefinition::draft_table("Years".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("year".into())],
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(query));
    let warning = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find("query-limit-reached")
                .and_then(|element| element.label().map(str::to_owned))
        })
        .unwrap()
    };
    assert_eq!(
        column_values(&entity, cx),
        [Some("2000".into()), Some("2004".into())]
    );
    assert_eq!(warning(cx).as_deref(), Some("Limited to 2 rows"));

    // 2008 is past the limit, but search runs inside the query and finds it.
    let search = |text: &str, cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            entity.update(cx, |view, cx| view.set_search_text(text.into(), window, cx))
        })
        .unwrap();
        cx.run_until_parked();
    };
    search("2008", cx);
    assert_eq!(column_values(&entity, cx), [Some("2008".into())]);
    // The warning follows the query's rows, not the searched rows.
    assert_eq!(warning(cx).as_deref(), Some("Limited to 2 rows"));

    search("", cx);
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.query_limit = 5;
            view.reload(false, window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(column_values(&entity, cx).len(), 3);
    assert_eq!(warning(cx), None);
}

#[gpui_kit::test]
fn condition_cards_come_from_the_picker_and_filter_on_enter(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("orders.csv");
    std::fs::write(&path, "qty\n5\n15\n25\n35\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.open_slicer_picker(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("condition", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let condition = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            entity.read(cx).session.working.as_ref().unwrap().slicers[0]
                .condition
                .clone()
        })
    };
    assert_eq!(condition(cx).as_deref(), Some(""));
    cx.update(|cx| {
        let view = entity.read(cx);
        let card = &view.slicer_stage.as_ref().unwrap().cards[0];
        assert_eq!(card.bounds, Some(("5".into(), "35".into())));
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-condition:qty", cx);
        window.input("10..30", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(condition(cx).as_deref(), Some("10..30"));
    assert_eq!(
        column_values(&entity, cx),
        [Some("15".into()), Some("25".into())]
    );

    // A broken condition shows on the card and stops filtering.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-condition:qty", cx);
        window.press("ctrl-a", cx);
        window.input(">=abc", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = entity.read(cx);
        let card = &view.slicer_stage.as_ref().unwrap().cards[0];
        assert!(card.error.as_deref().unwrap().contains("abc"));
    });
    assert_eq!(column_values(&entity, cx).len(), 4);

    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::ClearFilters, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(condition(cx).as_deref(), Some(""));
    assert_eq!(column_values(&entity, cx).len(), 4);

    // After an operator `"` lists the columns to compare against; Escape
    // closes the list and keeps the text.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("slicer-condition:qty", cx);
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.input(">\"", cx))
        .unwrap();
    cx.run_until_parked();
    let items = |cx: &mut TestAppContext| {
        cx.update(|cx| entity.read(cx).slicer_sidebar.read(cx).completion_items(cx))
    };
    assert_eq!(items(cx), Some(vec!["qty".into()]));
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(items(cx), None);
}

#[gpui_kit::test]
fn sql_filter_card_is_first_in_the_picker_and_filters_any_column(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("orders.csv");
    std::fs::write(
        &path,
        "region,qty
North,5
South,15
North,25
",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    let pick_first = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            entity.update(cx, |view, cx| view.open_slicer_picker(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    // With nothing typed, Enter picks the first entry: the SQL filter.
    pick_first(cx);
    let slicers = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            entity
                .read(cx)
                .session
                .working
                .as_ref()
                .unwrap()
                .slicers
                .clone()
        })
    };
    assert_eq!(slicers(cx).len(), 1);
    assert!(slicers(cx)[0].sql);

    // `"` lists columns in SQL cards too; Tab inserts the quoted name.
    let step = |cx: &mut TestAppContext, action: &dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App)| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            action(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            entity
                .read(cx)
                .slicer_sidebar
                .read(cx)
                .completion_items(cx)
        })
    };
    step(cx, &|window, cx| window.click("slicer-sql", cx));
    assert_eq!(step(cx, &|window, cx| window.input("\"", cx)), Some(vec!["region".into(), "qty".into()]));
    assert_eq!(step(cx, &|window, cx| window.input("re", cx)), Some(vec!["region".into()]));
    assert_eq!(step(cx, &|window, cx| window.press("tab", cx)), None);
    // Inside a 'string' a quote is just text.
    assert_eq!(step(cx, &|window, cx| window.input(" = 'No\"", cx)), None);
    step(cx, &|window, cx| window.press("backspace", cx));
    step(cx, &|window, cx| window.input("rth' AND \"q", cx));
    step(cx, &|window, cx| window.press("enter", cx));
    step(cx, &|window, cx| window.input(" > 10", cx));
    step(cx, &|window, cx| window.press("enter", cx));
    assert_eq!(
        slicers(cx)[0].condition.as_deref(),
        Some("\"region\" = 'North' AND \"qty\" > 10")
    );
    assert_eq!(column_values(&entity, cx), [Some("North".into())]);

    // Only one SQL card: the picker now starts with the region values card.
    pick_first(cx);
    let cards = slicers(cx);
    assert_eq!(cards.len(), 2);
    assert!(!cards[1].sql && cards[1].column == "region");
}

#[gpui_kit::test]
fn hidden_columns_leave_the_grid_and_plain_search_and_persist(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,note,city\nAda,secret,Paris\nBob,plain,Rome\n").unwrap();
    let (handle, entity) = viewer(cx, repository.clone());
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let columns = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            view.session
                .result
                .columns
                .iter()
                .map(|column| column.0.clone())
                .collect::<Vec<_>>()
        })
    };
    let search = |text: &str, cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            entity.update(cx, |view, cx| view.set_search_text(text.into(), window, cx))
        })
        .unwrap();
        cx.run_until_parked();
    };

    // Space h hides the active column.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 1 }, "space");
    cx.update_window(handle.into(), |_, window, cx| window.press("h", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(columns(cx), ["name", "city"]);

    // Plain search skips the hidden column; a column term still reaches it.
    search("secret", cx);
    assert!(column_values(&entity, cx).is_empty());
    search("note:secret", cx);
    assert_eq!(column_values(&entity, cx), [Some("Ada".into())]);
    search("", cx);

    // A plain file remembers hidden columns.
    let (handle, reopened) = viewer(cx, repository);
    open(cx, handle, &reopened, FileEntry::Source(path));
    cx.update(|cx| {
        let names = reopened
            .read(cx)
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, ["name", "city"]);
    });

    // Showing it again puts it back in its place.
    cx.update_window(handle.into(), |_, window, cx| {
        reopened.update(cx, |view, cx| {
            view.dispatch_command(CommandId::ShowAllColumns, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = reopened.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        let names = view
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, ["name", "note", "city"]);
    });
}

/// Runs Export, answers the save dialog with `target`, and returns the status.
fn export_to(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    entity: &Entity<DataFrameView>,
    target: &std::path::Path,
) -> Option<String> {
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::Export, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let target = target.to_path_buf();
    cx.simulate_new_path_selection(move |_| Some(target));
    cx.run_until_parked();
    cx.update(|cx| entity.read(cx).copy_status.clone())
}

#[gpui_kit::test]
fn export_writes_the_displayed_rows_and_visible_columns(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,note,n\nAda,x,1\nBob,y,2\nCy,x,3\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 1 }, "space");
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("h", cx);
        entity.update(cx, |view, cx| {
            view.set_search_text("note:x".into(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let grid = entity.read(cx).grid.clone();
        grid.update(cx, |grid, cx| {
            grid.select(Cell { row: 0, column: 1 }, false, cx)
        });
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::SortDescending, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let target = home.path().join("out.csv");
    assert_eq!(
        export_to(cx, handle, &entity, &target).as_deref(),
        Some("Exported 2 rows to out.csv")
    );
    assert_eq!(
        std::fs::read_to_string(&target)
            .unwrap()
            .replace("\r\n", "\n"),
        "name,n\nCy,3\nAda,1\n"
    );

    let unknown = home.path().join("out.txt");
    let status = export_to(cx, handle, &entity, &unknown).unwrap();
    assert!(
        status.starts_with("Export failed: Cannot tell the export format"),
        "{status}"
    );
    assert!(!unknown.exists());
}

#[gpui_kit::test]
fn export_reruns_a_truncated_query_without_the_limit(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let path = home.path().join("years.csv");
    std::fs::write(&path, "year,n\n2000,1\n2004,2\n2008,3\n2012,4\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path.clone()));
    let mut query = DocumentDefinition::draft_table("Years".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("year".into())],
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(query));
    assert_eq!(column_values(&entity, cx).len(), 2);
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.set_search_text("-2004".into(), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();

    let target = home.path().join("years.json");
    assert_eq!(
        export_to(cx, handle, &entity, &target).as_deref(),
        Some("Exported 3 rows to years.json")
    );
    let records: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let years = records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["year"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(years, [2000, 2008, 2012]);
}

#[gpui_kit::test]
fn column_chooser_toggles_columns_and_stays_open(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("wide.csv");
    std::fs::write(&path, "alpha,beta,gamma\n1,2,3\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.dispatch_command(CommandId::ChooseColumns, window, cx)
        });
        window.render_frame(cx);
        window.input("beta", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        let view = entity.read(cx);
        let names = view
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, ["alpha", "gamma"]);
        // Enter again on the same row shows it.
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(entity.read(cx).session.result.column_count(), 3));
}

fn status(entity: &Entity<DataFrameView>, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| entity.read(cx).copy_status.clone())
}

#[gpui_kit::test]
fn ctrl_z_and_ctrl_y_undo_and_redo_filters_and_sort_but_keep_search(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,city\nCy,Oslo\nAda,Rome\nBob,Oslo\n").unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    let state = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            (
                view.session.value_filters().len(),
                view.session.sort.len(),
                view.session.result.row_count(),
            )
        })
    };
    // Keep rows in Oslo, then sort by name.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 1 }, "=");
    assert_eq!(state(cx), (1, 0, 2));
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| {
            view.grid.update(cx, |grid, cx| {
                grid.select(Cell { row: 0, column: 0 }, false, cx)
            });
            view.dispatch_command(CommandId::SortAscending, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx), (1, 1, 2));
    assert_eq!(
        column_values(&entity, cx),
        [Some("Bob".into()), Some("Cy".into())]
    );
    cx.update_window(handle.into(), |_, window, cx| {
        entity.update(cx, |view, cx| view.set_search_text("y".into(), window, cx))
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx), (1, 1, 1));

    // Undo leaves the search alone: "y" still narrows the rows to Cy.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(state(cx), (1, 0, 1));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(state(cx), (0, 0, 1));
    cx.update(|cx| assert_eq!(entity.read(cx).session.search_text, "y"));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(status(&entity, cx).as_deref(), Some("Nothing to undo"));
    assert_eq!(state(cx), (0, 0, 1));

    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-y");
    assert_eq!(state(cx), (1, 0, 1));
    press_on_cell(
        cx,
        handle,
        &entity,
        Cell { row: 0, column: 0 },
        "ctrl-shift-z",
    );
    assert_eq!(state(cx), (1, 1, 1));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-y");
    assert_eq!(status(&entity, cx).as_deref(), Some("Nothing to redo"));
}

#[gpui_kit::test]
fn undo_restores_group_by_and_totals_of_a_query_and_resets_on_navigation(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(
        &path,
        "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    let mut query = DocumentDefinition::draft_table("Query".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![
            Dimension::Column("region".into()),
            Dimension::Column("kind".into()),
        ],
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
    open(cx, handle, &entity, FileEntry::View(query));
    let config = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            let config = view.session.query_config();
            (
                config.group_by.len(),
                config.totals,
                view.session.result.row_count(),
            )
        })
    };
    assert_eq!(config(cx), (2, false, 3));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "space");
    cx.update_window(handle.into(), |_, window, cx| window.press("t", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(config(cx), (2, true, 3));
    // Delete removes the active group-by column.
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "delete");
    assert_eq!(config(cx), (1, true, 2));

    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(config(cx), (2, true, 3));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(config(cx), (2, false, 3));

    // A new location starts with nothing to undo.
    open(cx, handle, &entity, FileEntry::Source(path));
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "ctrl-z");
    assert_eq!(status(&entity, cx).as_deref(), Some("Nothing to undo"));
    assert!(cx.update(|cx| entity.read(cx).is_table()));
}

#[gpui_kit::test]
fn shift_enter_drills_down_in_a_new_window_and_keeps_the_pivot(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(
        &path,
        "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        pivot_by: vec![Dimension::Column("kind".into())],
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
    let before = cx.update(|cx| cx.windows());
    press_on_cell(
        cx,
        handle,
        &entity,
        Cell { row: 0, column: 1 },
        "shift-enter",
    );
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
    });
    let added = cx
        .update(|cx| cx.windows())
        .into_iter()
        .filter(|window| !before.contains(window))
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1);
    let drilled = cx
        .update_window(added[0], |root, _, cx| {
            let Ok(root) = root.downcast::<Root>() else {
                panic!("window root")
            };
            let Ok(view) = root.read(cx).view().clone().downcast::<DataFrameView>() else {
                panic!("dataframe view")
            };
            view
        })
        .unwrap();
    cx.update(|cx| {
        let view = drilled.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(
            view.session
                .value_filters()
                .iter()
                .map(ValueFilter::label)
                .collect::<Vec<_>>(),
            ["region = North", "kind = a"]
        );
        assert_eq!(view.session.result.row_count(), 2);
    });
}

#[gpui_kit::test]
fn d_shows_the_active_cell_in_a_detail_pane_that_follows_the_cursor(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("notes.csv");
    std::fs::write(
        &path,
        "payload,note\n\"{\"\"b\"\":1,\"\"a\"\":[]}\",\"one\ntwo\"\n,short\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    open(cx, handle, &entity, FileEntry::Source(path));
    let pane = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            entity
                .read(cx)
                .cell_detail
                .as_ref()
                .map(|(pane, _)| pane.read(cx).text().map(str::to_owned))
        })
    };
    assert_eq!(pane(cx), None);
    press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "d");
    assert_eq!(
        pane(cx),
        Some(Some("{\n  \"b\": 1,\n  \"a\": []\n}".into()))
    );
    // The editor shows the same text once rendered.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let (pane, _) = entity.read(cx).cell_detail.as_ref().unwrap();
        assert_eq!(
            pane.read(cx).editor.read(cx).value().as_ref(),
            "{\n  \"b\": 1,\n  \"a\": []\n}"
        );
        window.press("right", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(pane(cx), Some(Some("one\ntwo".into())));
    press_on_cell(cx, handle, &entity, Cell { row: 1, column: 0 }, "left");
    assert_eq!(pane(cx), Some(None));
    press_on_cell(cx, handle, &entity, Cell { row: 1, column: 0 }, "d");
    assert_eq!(pane(cx), None);
}

#[gpui_kit::test]
fn space_m_shades_pivot_measures_on_one_scale_and_toggles_off(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let repository = ViewRepository::new(StorePaths::for_home(home.path()));
    let path = home.path().join("sales.csv");
    std::fs::write(
        &path,
        "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n",
    )
    .unwrap();
    let (handle, entity) = viewer(cx, repository);
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        pivot_by: vec![Dimension::Column("kind".into())],
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
        totals: true,
        ..Default::default()
    });
    open(cx, handle, &entity, FileEntry::View(pivot));
    let ranges = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = entity.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            let mut ranges = view
                .heat_ranges
                .as_ref()
                .map(|(_, ranges)| ranges.clone().into_iter().collect::<Vec<_>>())
                .unwrap_or_default();
            ranges.sort_by(|a, b| a.0.cmp(&b.0));
            ranges
        })
    };
    assert!(ranges(cx).is_empty());
    let toggle = |cx: &mut TestAppContext| {
        press_on_cell(cx, handle, &entity, Cell { row: 0, column: 0 }, "space");
        cx.update_window(handle.into(), |_, window, cx| window.press("m", cx))
            .unwrap();
        cx.run_until_parked();
    };
    toggle(cx);
    // Both pivot columns share the sum's range, North a = 15 to North b = 30;
    // the (Total) column is not shaded.
    assert_eq!(
        ranges(cx),
        [
            ("a_total".to_string(), (15.0, 30.0)),
            ("b_total".to_string(), (15.0, 30.0))
        ]
    );
    cx.update(|cx| {
        let view = entity.read(cx);
        assert!(view.session.query_config().heatmap);
        assert!(view.session.dirty);
    });
    toggle(cx);
    assert!(ranges(cx).is_empty());
    cx.update(|cx| assert!(!entity.read(cx).session.query_config().heatmap));
}
