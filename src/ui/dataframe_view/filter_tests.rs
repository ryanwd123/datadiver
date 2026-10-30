use super::FileEntry;
use super::test_support::{Viewer, empty_frame, repository};
use crate::application::commands::CommandId;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, QueryConfig, ValueFilter, ViewKind,
};
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::hit::{Button, Hit, SlicerHit};
use crate::ui::platform::system::fake;

fn viewer(repository: ViewRepository) -> Viewer {
    Viewer::open(empty_frame(), None, None, repository)
}

fn open(viewer: &Viewer, entry: FileEntry) {
    viewer.update(|view, cx| view.navigate_to_file(entry, false, false, false, cx));
    viewer.run_until_parked();
}

/// Focuses the grid, selects the cell, and presses `keys` there.
fn press_on_cell(viewer: &Viewer, row: usize, column: usize, keys: &str) {
    viewer.select(row, column);
    viewer.render();
    viewer.press(keys);
    viewer.run_until_parked();
}

fn column_values(viewer: &Viewer) -> Vec<Option<String>> {
    viewer.read(|view| {
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

fn column_names(viewer: &Viewer) -> Vec<String> {
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        view.session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect()
    })
}

fn search(viewer: &Viewer, text: &str) {
    viewer.update(|view, cx| view.set_search_text(text.into(), cx));
    viewer.run_until_parked();
}

fn command(viewer: &Viewer, id: CommandId) {
    viewer.update(|view, cx| view.dispatch_command(id, cx));
    viewer.run_until_parked();
}

fn status(viewer: &Viewer) -> Option<String> {
    viewer.read(|view| view.copy_status.clone())
}

fn sum_total() -> Aggregation {
    Aggregation {
        source_column: "n".into(),
        operation: AggregationOp::Sum,
        name: Some("total".into()),
        sql: None,
        conditions: Vec::new(),
        percentile: None,
        interpolation: None,
        ddof: None,
    }
}

/// A pivot of `n` by region and kind.
fn pivot(path: &std::path::Path, aggregation: Aggregation, totals: bool) -> DocumentDefinition {
    let mut pivot = DocumentDefinition::draft_table("Pivot".into(), path.to_string_lossy().into());
    pivot.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        pivot_by: vec![Dimension::Column("kind".into())],
        aggregations: vec![aggregation],
        totals,
        ..Default::default()
    });
    pivot
}

const SALES: &str = "region,kind,n\nNorth,a,10\nNorth,b,30\nSouth,a,20\nNorth,a,5\n";

/// Clicks the input of the slicer card on `column`.
fn click_slicer_input(viewer: &Viewer, column: &str) {
    viewer.render();
    let id = viewer
        .read(|view| view.slicer_sidebar.input_for(column))
        .unwrap_or_else(|| panic!("no input for {column}"));
    viewer.click(|hit| matches!(hit, Hit::Editor(editor, _) if *editor == id));
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

fn completion_items(viewer: &Viewer) -> Option<Vec<String>> {
    viewer.read(|view| view.slicer_sidebar.completion_items())
}

#[test]
fn value_keys_filter_exclude_null_cells_and_clear() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("data.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\nNorth,30\n,40\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    assert_eq!(column_values(&viewer).len(), 4);

    press_on_cell(&viewer, 0, 0, "=");
    assert_eq!(column_values(&viewer), [Some("North".into()), Some("North".into())]);
    viewer.render();
    viewer.read(|view| {
        assert_eq!(view.session.value_filters()[0].label(), "region = North");
        assert!(view.session.has_filters());
    });
    viewer.update(|view, cx| view.remove_value_filter(0, cx));
    viewer.run_until_parked();
    assert_eq!(column_values(&viewer).len(), 4);

    press_on_cell(&viewer, 0, 0, "-");
    assert_eq!(column_values(&viewer), [Some("South".into()), None]);
    press_on_cell(&viewer, 1, 0, "=");
    assert_eq!(column_values(&viewer), [None]);
    assert_eq!(
        viewer.read(|view| view.session.value_filters()[1].clone()),
        ValueFilter {
            target: "region".into(),
            value: None,
            exclude: false,
        }
    );

    command(&viewer, CommandId::ClearFilters);
    assert_eq!(column_values(&viewer).len(), 4);
    assert!(viewer.read(|view| view.session.value_filters().is_empty()));
}

#[test]
fn enter_drills_from_pivot_cells_to_source_rows_and_back_returns() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path.clone()));
    open(&viewer, FileEntry::View(pivot(&path, sum_total(), false)));
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.result.columns[1].0, "a_total");
    });

    // A pivoted measure drills into both the group and the pivot keys.
    press_on_cell(&viewer, 0, 1, "enter");
    viewer.read(|view| {
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

    viewer.focus_grid();
    viewer.render();
    viewer.press("u");
    viewer.run_until_parked();
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
        assert_eq!(view.session.result.row_count(), 2);
    });

    // A group-by cell drills into every row of the group.
    press_on_cell(&viewer, 0, 0, "enter");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
    });
}

#[test]
fn totals_of_a_file_cut_at_the_limit_say_how_many_rows_they_cover() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 3;
    repository.save_settings(&settings).unwrap();
    let viewer = viewer(repository);
    open(&viewer, FileEntry::Source(path.clone()));
    open(&viewer, FileEntry::View(pivot(&path, sum_total(), true)));
    let footer = viewer.read(|view| view.grid.footer().cloned()).expect("grand total row");
    assert_eq!(footer.label, "Total (first 3 rows)");
}

#[test]
fn space_t_adds_row_totals_and_a_grand_total_footer() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path.clone()));
    let mean = Aggregation {
        operation: AggregationOp::Mean,
        name: None,
        ..sum_total()
    };
    open(&viewer, FileEntry::View(pivot(&path, mean, false)));
    press_on_cell(&viewer, 0, 0, "space t");
    let footer = || viewer.read(|view| view.grid.footer().cloned());
    assert_eq!(column_names(&viewer), ["region", "a", "b", "(Total)"]);
    // North's mean over its three rows is 15, not the mean of its cells (7.5, 30).
    let north_total = viewer.read(|view| {
        let result = view.session.result.clone();
        let row = result.fetch_rows(0, 1).unwrap().remove(0);
        row.values[result.column_indices[3]].clone()
    });
    assert_eq!(north_total.as_deref(), Some("15.0"));
    let totals = footer().expect("grand total row");
    assert_eq!(totals.label, "Total");
    let value = |name: &str| {
        totals
            .values
            .iter()
            .find(|(column, _)| column == name)
            .and_then(|(_, value)| value.clone())
    };
    assert_eq!(value("a").as_deref(), Some("11.666666666666666"));
    assert_eq!(value("b").as_deref(), Some("30.0"));
    assert_eq!(value("(Total)").as_deref(), Some("16.25"));

    let region_width = || {
        viewer.render();
        viewer.read(|view| view.grid.column_width("region"))
    };
    let narrow = region_width();

    // Searching narrows the rows, not the totals, and the label says so. The
    // longer label widens its column.
    search(&viewer, "south");
    assert_eq!(footer().unwrap().label, "Total (all groups)");
    let wide = region_width();
    assert!(wide > narrow, "{wide} > {narrow}");
    search(&viewer, "");
    // Widths only grow on their own; recalculating fits the short label again.
    assert_eq!(region_width(), wide);
    command(&viewer, CommandId::RecalculateWidths);
    region_width();
    viewer.run_until_parked();
    assert_eq!(region_width(), narrow);

    // A row total drills into every row of its group.
    press_on_cell(&viewer, 0, 3, "enter");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.result.row_count(), 3);
        assert!(view.grid.footer().is_none());
    });
}

#[test]
fn value_keys_on_pivot_group_cells_filter_the_source() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, "region,n\nNorth,10\nSouth,20\nEast,5\n").unwrap();
    let viewer = viewer(repository(home.path()));
    let mut query = DocumentDefinition::draft_table("Query".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into())],
        aggregations: vec![Aggregation::sql_expression("sum(n)".into(), Some("total".into()))],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(query));
    assert_eq!(column_values(&viewer).len(), 3);
    press_on_cell(&viewer, 0, 0, "-");
    assert_eq!(column_values(&viewer).len(), 2);
    press_on_cell(&viewer, 0, 1, "=");
    viewer.read(|view| {
        assert!(view.error.as_deref().unwrap().contains("group-by"));
        assert_eq!(view.session.value_filters().len(), 1);
        assert!(view.session.dirty);
    });
}

#[test]
fn jump_to_column_moves_the_cursor_to_the_chosen_column() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("wide.csv");
    std::fs::write(&path, "alpha,beta,gamma,delta\n1,2,3,4\n5,6,7,8\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    viewer.select(1, 0);
    viewer.render();
    viewer.press("ctrl-g");
    viewer.render();
    viewer.input("gamm");
    viewer.render();
    viewer.press("enter");
    viewer.run_until_parked();
    let active = viewer.read(|view| view.grid.selection().unwrap().active);
    assert_eq!((active.row, active.column), (1, 2));
    assert!(viewer.grid_focused());
}

#[test]
fn drill_down_from_a_database_view_query_keeps_the_view_source() {
    use crate::application::session::SessionIdentity;
    use crate::domain::documents::DatabaseView;
    let home = tempfile::tempdir().unwrap();
    let viewer = viewer(repository(home.path()));
    viewer.read(|view| {
        view.database
            .connect()
            .unwrap()
            .execute_batch("CREATE VIEW main.sales AS SELECT range AS n, range % 3 AS category FROM range(30)")
            .unwrap()
    });
    let mut query = DocumentDefinition::draft_database_view(DatabaseView::view("main", "sales"));
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("category".into())],
        aggregations: vec![Aggregation::sql_expression("count(*)".into(), Some("rows".into()))],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(query));
    assert_eq!(column_values(&viewer).len(), 3);
    press_on_cell(&viewer, 1, 1, "enter");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.session.identity, SessionIdentity::DraftQuery);
        assert_eq!(view.session.value_filters()[0].label(), "category = 1");
        assert_eq!(view.session.result.row_count(), 10);
    });
}

#[test]
fn value_filters_grey_out_slicer_values_without_matching_rows() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("colors.csv");
    std::fs::write(&path, "region,color\nEast,Red\nEast,Blue\nWest,Red\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    viewer.update(|view, cx| view.add_slicer("color".into(), cx));
    viewer.run_until_parked();
    let available = || {
        viewer.read(|view| {
            let card = &view.slicer_stage.as_ref().unwrap().cards[0];
            card.values
                .iter()
                .map(|value| (value.label.clone(), value.available))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(available(), [("Blue".to_owned(), true), ("Red".to_owned(), true)]);
    // Excluding East leaves only West's Red row.
    press_on_cell(&viewer, 0, 0, "-");
    assert_eq!(available(), [("Blue".to_owned(), false), ("Red".to_owned(), true)]);
}

#[test]
fn slicer_card_filter_narrows_values_and_enter_selects_them() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("cities.csv");
    std::fs::write(&path, "city\nAustin\nBoston\nDallas\nHouston\nParis\nAtlanta\nDenver\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    viewer.update(|view, cx| view.add_slicer("city".into(), cx));
    viewer.run_until_parked();
    // Values sort as Atlanta, Austin, Boston, Dallas, Denver, Houston, Paris.
    assert!(slicer_value_shown(&viewer, "city", 0));
    viewer.click(|hit| *hit == Hit::Button(Button::SlicerSearch("city".into())));
    viewer.input("TON");
    assert!(!slicer_value_shown(&viewer, "city", 0));
    assert!(slicer_value_shown(&viewer, "city", 2));
    assert!(slicer_value_shown(&viewer, "city", 5));
    viewer.press("enter");
    viewer.run_until_parked();
    assert_eq!(column_values(&viewer), [Some("Boston".into()), Some("Houston".into())]);
    viewer.press("escape");
    assert!(slicer_value_shown(&viewer, "city", 0));
    viewer.run_until_parked();
    assert_eq!(column_values(&viewer).len(), 2);
}

#[test]
fn truncated_query_results_warn_and_search_past_the_limit() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let path = home.path().join("years.csv");
    std::fs::write(&path, "year,n\n2000,1\n2004,2\n2008,3\n").unwrap();
    let viewer = viewer(repository);
    open(&viewer, FileEntry::Source(path.clone()));
    // Scanned in place, so the limit cuts the query's rows rather than the file's.
    let mut query =
        DocumentDefinition::draft_database_view(crate::domain::documents::DatabaseView::file(path.to_string_lossy()));
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("year".into())],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(query));
    let warning = || {
        viewer.read(|view| view.limit_reached().then(|| crate::ui::status::limited_rows(view.query_limit)))
    };
    assert_eq!(column_values(&viewer), [Some("2000".into()), Some("2004".into())]);
    assert_eq!(warning().as_deref(), Some("Limited to 2 rows"));

    // 2008 is past the limit, but search runs inside the query and finds it.
    search(&viewer, "2008");
    assert_eq!(column_values(&viewer), [Some("2008".into())]);
    // The warning follows the query's rows, not the searched rows.
    assert_eq!(warning().as_deref(), Some("Limited to 2 rows"));

    search(&viewer, "");
    viewer.update(|view, cx| {
        view.query_limit = 5;
        view.reload(false, cx);
    });
    viewer.run_until_parked();
    assert_eq!(column_values(&viewer).len(), 3);
    assert_eq!(warning(), None);
}

#[test]
fn condition_cards_come_from_the_picker_and_filter_on_enter() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("orders.csv");
    std::fs::write(&path, "qty\n5\n15\n25\n35\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    viewer.update(|view, cx| view.open_slicer_picker(cx));
    viewer.run_until_parked();
    viewer.render();
    viewer.input("condition");
    viewer.render();
    viewer.press("enter");
    viewer.run_until_parked();
    let condition = || viewer.read(|view| view.session.working.as_ref().unwrap().slicers[0].condition.clone());
    assert_eq!(condition().as_deref(), Some(""));
    viewer.read(|view| {
        let card = &view.slicer_stage.as_ref().unwrap().cards[0];
        assert_eq!(card.bounds, Some(("5".into(), "35".into())));
    });

    click_slicer_input(&viewer, "qty");
    viewer.input("10..30");
    viewer.press("enter");
    viewer.run_until_parked();
    assert_eq!(condition().as_deref(), Some("10..30"));
    assert_eq!(column_values(&viewer), [Some("15".into()), Some("25".into())]);

    // A broken condition shows on the card and stops filtering.
    click_slicer_input(&viewer, "qty");
    viewer.press("ctrl-a");
    viewer.input(">=abc");
    viewer.press("enter");
    viewer.run_until_parked();
    viewer.read(|view| {
        let card = &view.slicer_stage.as_ref().unwrap().cards[0];
        assert!(card.error.as_deref().unwrap().contains("abc"));
    });
    assert_eq!(column_values(&viewer).len(), 4);

    command(&viewer, CommandId::ClearFilters);
    assert_eq!(condition().as_deref(), Some(""));
    assert_eq!(column_values(&viewer).len(), 4);

    // After an operator `"` lists the columns to compare against; Escape
    // closes the list and keeps the text.
    click_slicer_input(&viewer, "qty");
    viewer.input(">\"");
    viewer.run_until_parked();
    assert_eq!(completion_items(&viewer), Some(vec!["qty".into()]));
    viewer.press("escape");
    viewer.run_until_parked();
    assert_eq!(completion_items(&viewer), None);
}

#[test]
fn sql_filter_card_is_first_in_the_picker_and_filters_any_column() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("orders.csv");
    std::fs::write(&path, "region,qty\nNorth,5\nSouth,15\nNorth,25\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    let pick_first = || {
        viewer.update(|view, cx| view.open_slicer_picker(cx));
        viewer.run_until_parked();
        viewer.render();
        viewer.press("enter");
        viewer.run_until_parked();
    };
    // With nothing typed, Enter picks the first entry: the SQL filter.
    pick_first();
    let slicers = || viewer.read(|view| view.session.working.as_ref().unwrap().slicers.clone());
    assert_eq!(slicers().len(), 1);
    assert!(slicers()[0].sql);

    // `"` lists columns in SQL cards too; Tab inserts the quoted name.
    let step = |action: &dyn Fn()| {
        viewer.render();
        action();
        viewer.run_until_parked();
        completion_items(&viewer)
    };
    let sql_column = slicers()[0].column.clone();
    step(&|| click_slicer_input(&viewer, &sql_column));
    assert_eq!(step(&|| viewer.input("\"")), Some(vec!["region".into(), "qty".into()]));
    assert_eq!(step(&|| viewer.input("re")), Some(vec!["region".into()]));
    assert_eq!(step(&|| viewer.press("tab")), None);
    // Inside a 'string' a quote is just text.
    assert_eq!(step(&|| viewer.input(" = 'No\"")), None);
    step(&|| viewer.press("backspace"));
    step(&|| viewer.input("rth' AND \"q"));
    step(&|| viewer.press("enter"));
    step(&|| viewer.input(" > 10"));
    step(&|| viewer.press("enter"));
    assert_eq!(slicers()[0].condition.as_deref(), Some("\"region\" = 'North' AND \"qty\" > 10"));
    assert_eq!(column_values(&viewer), [Some("North".into())]);

    // Only one SQL card: the picker now starts with the region values card.
    pick_first();
    let cards = slicers();
    assert_eq!(cards.len(), 2);
    assert!(!cards[1].sql && cards[1].column == "region");
}

#[test]
fn hidden_columns_leave_the_grid_and_plain_search_and_persist() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,note,city\nAda,secret,Paris\nBob,plain,Rome\n").unwrap();
    let viewer = viewer(repository.clone());
    open(&viewer, FileEntry::Source(path.clone()));

    // Space h hides the active column.
    press_on_cell(&viewer, 0, 1, "space h");
    assert_eq!(column_names(&viewer), ["name", "city"]);

    // Plain search skips the hidden column; a column term still reaches it.
    search(&viewer, "secret");
    assert!(column_values(&viewer).is_empty());
    search(&viewer, "note:secret");
    assert_eq!(column_values(&viewer), [Some("Ada".into())]);
    search(&viewer, "");

    // A plain file remembers hidden columns.
    let reopened = Viewer::open_in(&viewer.app, empty_frame(), None, None, repository);
    open(&reopened, FileEntry::Source(path));
    assert_eq!(column_names(&reopened), ["name", "city"]);

    // Showing it again puts it back in its place.
    command(&reopened, CommandId::ShowAllColumns);
    assert_eq!(column_names(&reopened), ["name", "note", "city"]);
}

/// Runs Export, answers the save dialog with `target`, and returns the status.
fn export_to(viewer: &Viewer, target: &std::path::Path) -> Option<String> {
    let prompts = fake::with(|system| {
        system.new_path_answers.push_back(Some(target.to_path_buf()));
        system.new_path_prompts
    });
    command(viewer, CommandId::Export);
    assert_eq!(fake::with(|system| system.new_path_prompts), prompts + 1);
    viewer.run_until_parked();
    status(viewer)
}

#[test]
fn export_writes_the_displayed_rows_and_visible_columns() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,note,n\nAda,x,1\nBob,y,2\nCy,x,3\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    press_on_cell(&viewer, 0, 1, "space h");
    search(&viewer, "note:x");
    viewer.select(0, 1);
    command(&viewer, CommandId::SortDescending);
    let target = home.path().join("out.csv");
    assert_eq!(export_to(&viewer, &target).as_deref(), Some("Exported 2 rows to out.csv"));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap().replace("\r\n", "\n"),
        "name,n\nCy,3\nAda,1\n"
    );

    let unknown = home.path().join("out.txt");
    let status = export_to(&viewer, &unknown).unwrap();
    assert!(status.starts_with("Export failed: Cannot tell the export format"), "{status}");
    assert!(!unknown.exists());
}

#[test]
fn export_reruns_a_truncated_query_without_the_limit() {
    let home = tempfile::tempdir().unwrap();
    let repository = repository(home.path());
    let mut settings = repository.load_settings().unwrap();
    settings.query_limit = 2;
    repository.save_settings(&settings).unwrap();
    let path = home.path().join("years.csv");
    std::fs::write(&path, "year,n\n2000,1\n2004,2\n2008,3\n2012,4\n").unwrap();
    let viewer = viewer(repository);
    open(&viewer, FileEntry::Source(path.clone()));
    // Scanned in place, so the limit cuts the query's rows rather than the file's.
    let mut query =
        DocumentDefinition::draft_database_view(crate::domain::documents::DatabaseView::file(path.to_string_lossy()));
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("year".into())],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(query));
    assert_eq!(column_values(&viewer).len(), 2);
    search(&viewer, "-2004");

    let target = home.path().join("years.json");
    assert_eq!(export_to(&viewer, &target).as_deref(), Some("Exported 3 rows to years.json"));
    let records: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let years = records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["year"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(years, [2000, 2008, 2012]);
}

#[test]
fn column_chooser_toggles_columns_and_stays_open() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("wide.csv");
    std::fs::write(&path, "alpha,beta,gamma\n1,2,3\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    viewer.update(|view, cx| view.dispatch_command(CommandId::ChooseColumns, cx));
    viewer.render();
    viewer.input("beta");
    viewer.render();
    viewer.press("enter");
    viewer.run_until_parked();
    viewer.render();
    assert!(viewer.has_dialog());
    assert_eq!(column_names(&viewer), ["alpha", "gamma"]);
    // Enter again on the same row shows it.
    viewer.press("enter");
    viewer.run_until_parked();
    assert_eq!(viewer.read(|view| view.session.result.column_count()), 3);
}

#[test]
fn ctrl_z_and_ctrl_y_undo_and_redo_filters_and_sort_but_keep_search() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,city\nCy,Oslo\nAda,Rome\nBob,Oslo\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    let state = || {
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            (
                view.session.value_filters().len(),
                view.session.sort.len(),
                view.session.result.row_count(),
            )
        })
    };
    // Keep rows in Oslo, then sort by name.
    press_on_cell(&viewer, 0, 1, "=");
    assert_eq!(state(), (1, 0, 2));
    viewer.select(0, 0);
    command(&viewer, CommandId::SortAscending);
    assert_eq!(state(), (1, 1, 2));
    assert_eq!(column_values(&viewer), [Some("Bob".into()), Some("Cy".into())]);
    search(&viewer, "y");
    assert_eq!(state(), (1, 1, 1));

    // Undo leaves the search alone: "y" still narrows the rows to Cy.
    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(state(), (1, 0, 1));
    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(state(), (0, 0, 1));
    assert_eq!(viewer.read(|view| view.session.search_text.clone()), "y");
    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(status(&viewer).as_deref(), Some("Nothing to undo"));
    assert_eq!(state(), (0, 0, 1));

    press_on_cell(&viewer, 0, 0, "ctrl-y");
    assert_eq!(state(), (1, 0, 1));
    press_on_cell(&viewer, 0, 0, "ctrl-shift-z");
    assert_eq!(state(), (1, 1, 1));
    press_on_cell(&viewer, 0, 0, "ctrl-y");
    assert_eq!(status(&viewer).as_deref(), Some("Nothing to redo"));
}

#[test]
fn undo_restores_group_by_and_totals_of_a_query_and_resets_on_navigation() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    let mut query = DocumentDefinition::draft_table("Query".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::Column("region".into()), Dimension::Column("kind".into())],
        aggregations: vec![sum_total()],
        ..Default::default()
    });
    open(&viewer, FileEntry::View(query));
    let config = || {
        viewer.read(|view| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let config = view.session.query_config();
            (config.group_by.len(), config.totals, view.session.result.row_count())
        })
    };
    assert_eq!(config(), (2, false, 3));
    press_on_cell(&viewer, 0, 0, "space t");
    assert_eq!(config(), (2, true, 3));
    // Delete removes the active group-by column.
    press_on_cell(&viewer, 0, 0, "delete");
    assert_eq!(config(), (1, true, 2));

    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(config(), (2, true, 3));
    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(config(), (2, false, 3));

    // A new location starts with nothing to undo.
    open(&viewer, FileEntry::Source(path));
    press_on_cell(&viewer, 0, 0, "ctrl-z");
    assert_eq!(status(&viewer).as_deref(), Some("Nothing to undo"));
    assert!(viewer.read(|view| view.is_table()));
}

#[test]
fn shift_enter_drills_down_in_a_new_window_and_keeps_the_pivot() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::View(pivot(&path, sum_total(), false)));
    press_on_cell(&viewer, 0, 1, "shift-enter");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.is_table());
    });
    let added = viewer.others();
    assert_eq!(added.len(), 1);
    added[0].run_until_parked();
    added[0].read(|view| {
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

#[test]
fn d_shows_the_active_cell_in_a_detail_pane_that_follows_the_cursor() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("notes.csv");
    std::fs::write(&path, "payload,note\n\"{\"\"b\"\":1,\"\"a\"\":[]}\",\"one\ntwo\"\n,short\n").unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path));
    let pane = || {
        viewer.read(|view| {
            view.cell_detail
                .as_ref()
                .map(|pane| pane.text().map(str::to_owned))
        })
    };
    assert_eq!(pane(), None);
    press_on_cell(&viewer, 0, 0, "d");
    let pretty = "{\n  \"b\": 1,\n  \"a\": []\n}";
    assert_eq!(pane(), Some(Some(pretty.into())));
    // The editor shows the same text once rendered.
    viewer.render();
    assert_eq!(viewer.read(|view| view.cell_detail.as_ref().unwrap().editor.text().to_owned()), pretty);
    viewer.press("right");
    viewer.run_until_parked();
    assert_eq!(pane(), Some(Some("one\ntwo".into())));
    press_on_cell(&viewer, 1, 0, "left");
    assert_eq!(pane(), Some(None));
    press_on_cell(&viewer, 1, 0, "d");
    assert_eq!(pane(), None);
}

#[test]
fn space_m_shades_pivot_measures_on_one_scale_and_toggles_off() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::View(pivot(&path, sum_total(), true)));
    let ranges = || {
        viewer.read(|view| {
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
    assert!(ranges().is_empty());
    let toggle = || press_on_cell(&viewer, 0, 0, "space m");
    toggle();
    // Both pivot columns share the sum's range, North a = 15 to North b = 30;
    // the (Total) column is not shaded.
    assert_eq!(
        ranges(),
        [("a_total".to_string(), (15.0, 30.0)), ("b_total".to_string(), (15.0, 30.0))]
    );
    viewer.read(|view| {
        assert!(view.session.query_config().heatmap);
        assert!(view.session.dirty);
    });
    toggle();
    assert!(ranges().is_empty());
    assert!(!viewer.read(|view| view.session.query_config().heatmap));
}
