use super::FileEntry;
use super::test_support::{Viewer, empty_frame, repository};
use crate::application::commands::CommandId;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, OutlineLayout, QueryConfig, ValueFilter, ViewKind,
};
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::platform::system::fake;

const SALES: &str = "region,product,amount\nNorth,A,1\nNorth,B,2\nSouth,A,3\nSouth,B,4\nSouth,B,5\n";

fn viewer(repository: ViewRepository) -> Viewer {
    Viewer::open(empty_frame(), None, None, repository)
}

fn open(viewer: &Viewer, entry: FileEntry) {
    viewer.update(|view, cx| view.navigate_to_file(entry, false, false, false, cx));
    viewer.run_until_parked();
}

fn press_on_cell(viewer: &Viewer, row: usize, column: usize, keys: &str) {
    viewer.select(row, column);
    viewer.render();
    viewer.press(keys);
    viewer.run_until_parked();
    viewer.render();
}

fn command(viewer: &Viewer, id: CommandId) {
    viewer.update(|view, cx| view.dispatch_command(id, cx));
    viewer.run_until_parked();
    viewer.render();
}

/// The query by region and product, summing amount.
fn query(path: &std::path::Path) -> DocumentDefinition {
    let mut query = DocumentDefinition::draft_table("Sales".into(), path.to_string_lossy().into());
    query.view = ViewKind::Query(QueryConfig {
        group_by: vec![Dimension::from("region"), Dimension::from("product")],
        aggregations: vec![Aggregation {
            sql: None,
            source_column: "amount".into(),
            operation: AggregationOp::Sum,
            name: Some("total".into()),
            conditions: Vec::new(),
            percentile: None,
            interpolation: None,
            ddof: None,
        }],
        ..Default::default()
    });
    query
}

/// Every shown row, as the result's own columns.
fn rows(viewer: &Viewer) -> Vec<Vec<Option<String>>> {
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let result = &view.session.result;
        result
            .fetch_rows(0, result.row_count())
            .unwrap()
            .into_iter()
            .map(|row| result.column_indices.iter().map(|index| row.values[*index].clone()).collect())
            .collect()
    })
}

fn text(values: &[&str]) -> Vec<Option<String>> {
    values.iter().map(|value| (!value.is_empty()).then(|| value.to_string())).collect()
}

fn config(viewer: &Viewer) -> QueryConfig {
    viewer.read(|view| match view.session.working.as_ref().map(|definition| &definition.view) {
        Some(ViewKind::Query(config)) => config.clone(),
        _ => panic!("expected a query"),
    })
}

fn status(viewer: &Viewer) -> String {
    viewer.read(|view| view.copy_status.clone().unwrap_or_default())
}

fn setup() -> (tempfile::TempDir, Viewer, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    std::fs::write(&path, SALES).unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path.clone()));
    open(&viewer, FileEntry::View(query(&path)));
    (home, viewer, path)
}

#[test]
fn ctrl_alt_arrows_collapse_and_expand_whole_fields() {
    let (_home, viewer, _) = setup();
    assert_eq!(rows(&viewer).len(), 4);
    // On the product column, the query groups by region only.
    press_on_cell(&viewer, 0, 1, "ctrl-alt-left");
    assert_eq!(config(&viewer).depth, Some(1));
    assert_eq!(rows(&viewer), [text(&["North", "3"]), text(&["South", "12"])]);
    assert!(status(&viewer).contains("first 1 field"), "{}", status(&viewer));
    // On the first field, only the grand total is left.
    press_on_cell(&viewer, 0, 0, "ctrl-alt-left");
    assert_eq!(config(&viewer).depth, Some(0));
    assert_eq!(rows(&viewer), [text(&["15"])]);
    press_on_cell(&viewer, 0, 0, "ctrl-alt-right");
    press_on_cell(&viewer, 0, 0, "ctrl-alt-right");
    assert_eq!(config(&viewer).depth, None);
    assert_eq!(rows(&viewer).len(), 4);
    press_on_cell(&viewer, 0, 0, "ctrl-alt-right");
    assert_eq!(status(&viewer), "Every group-by field is shown");
    // Undo brings the collapsed field back.
    command(&viewer, CommandId::Undo);
    assert_eq!(config(&viewer).depth, Some(1));
}

#[test]
fn alt_arrows_collapse_items_into_subtotal_rows_that_drill_and_filter_by_their_keys() {
    let (_home, viewer, path) = setup();
    // Alt+Left on North's region cell collapses North into one row.
    press_on_cell(&viewer, 0, 0, "alt-left");
    assert_eq!(
        rows(&viewer),
        [text(&["North", "", "3"]), text(&["South", "A", "3"]), text(&["South", "B", "9"])]
    );
    viewer.read(|view| {
        assert!(view.session.result.outline.is_some());
        // The hidden level and open columns stay out of the grid.
        assert_eq!(view.session.result.column_count(), 3);
    });
    // A leaf's Alt+Left on its measure collapses its parent.
    press_on_cell(&viewer, 2, 2, "alt-left");
    assert_eq!(rows(&viewer), [text(&["North", "", "3"]), text(&["South", "", "12"])]);
    // Sorting would separate items from their children.
    press_on_cell(&viewer, 0, 2, "]");
    assert!(status(&viewer).contains("outline keeps"), "{}", status(&viewer));
    assert_eq!(rows(&viewer)[0], text(&["North", "", "3"]));
    // Drilling down from a subtotal row filters by its own key only.
    press_on_cell(&viewer, 1, 2, "enter");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.is_table());
        assert_eq!(view.session.value_filters().iter().map(ValueFilter::label).collect::<Vec<_>>(), ["region = South"]);
        assert_eq!(view.session.result.row_count(), 3);
    });
    open(&viewer, FileEntry::View(query(&path)));
    // A fresh draft starts flat again.
    assert_eq!(rows(&viewer).len(), 4);
    press_on_cell(&viewer, 0, 0, "alt-left");
    // Filtering by the rolled-up product cell has no key to use.
    press_on_cell(&viewer, 0, 1, "=");
    assert!(viewer.read(|view| view.error.clone()).is_some_and(|error| error.contains("total over every product")));
    viewer.update(|view, _| view.error = None);
    // Alt+Right expands it again; on an expanded row it says so.
    press_on_cell(&viewer, 0, 0, "alt-right");
    assert_eq!(rows(&viewer).len(), 4);
    press_on_cell(&viewer, 0, 0, "alt-right");
    assert!(status(&viewer).contains("expanded already"));
}

#[test]
fn layouts_subtotals_levels_search_and_export_follow_the_outline() {
    let (home, viewer, _) = setup();
    viewer.update(|view, cx| view.set_outline_layout(OutlineLayout::Outline, cx));
    viewer.run_until_parked();
    viewer.render();
    // A row above each region holds its subtotal.
    assert_eq!(
        rows(&viewer),
        [
            text(&["North", "", "3"]),
            text(&["North", "A", "1"]),
            text(&["North", "B", "2"]),
            text(&["South", "", "12"]),
            text(&["South", "A", "3"]),
            text(&["South", "B", "9"]),
        ]
    );
    // A search keeps the rows above its matches.
    viewer.update(|view, cx| view.set_search_text("product:B".into(), cx));
    viewer.run_until_parked();
    assert_eq!(rows(&viewer).len(), 4);
    viewer.update(|view, cx| view.set_search_text(String::new(), cx));
    viewer.run_until_parked();
    // Without subtotals for region, its rows are labels only.
    press_on_cell(&viewer, 0, 0, "");
    command(&viewer, CommandId::ToggleSubtotals);
    assert_eq!(rows(&viewer)[0], text(&["North", "", ""]));
    command(&viewer, CommandId::ToggleSubtotals);
    // Expanding to region's level collapses every region.
    command(&viewer, CommandId::ExpandToLevel);
    assert_eq!(rows(&viewer), [text(&["North", "", "3"]), text(&["South", "", "12"])]);
    press_on_cell(&viewer, 1, 0, "alt-right");
    assert_eq!(rows(&viewer).len(), 4);
    command(&viewer, CommandId::ExpandAll);
    assert_eq!(rows(&viewer).len(), 6);
    // The compact layout shows one column of row labels.
    viewer.update(|view, cx| view.set_outline_layout(OutlineLayout::Compact, cx));
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(rows(&viewer)[1], text(&["North", "1"]));
    // Filtering by a label filters by its item's keys.
    press_on_cell(&viewer, 1, 0, "=");
    viewer.read(|view| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.session.value_filters().iter().map(ValueFilter::label).collect::<Vec<_>>(),
            ["region = North", "product = A"]
        );
    });
    assert_eq!(rows(&viewer).len(), 2);
    viewer.update(|view, cx| {
        view.session.edit_definition(|definition| definition.filters.clear()).unwrap();
        view.schedule_search(String::new(), cx);
    });
    viewer.run_until_parked();
    // An export writes every key and the level, whatever the layout shows.
    let target = home.path().join("outline.csv");
    fake::with(|system| system.new_path_answers.push_back(Some(target.clone())));
    command(&viewer, CommandId::Export);
    viewer.run_until_parked();
    assert_eq!(status(&viewer), "Exported 6 rows to outline.csv");
    let written = std::fs::read_to_string(&target).unwrap();
    let lines = written.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "region,product,total,Level");
    assert_eq!(lines[1], "North,,3,1");
    assert_eq!(lines[2], "North,A,1,2");
    // Turning the outline off shows the flat result, which sorts again.
    command(&viewer, CommandId::FlatResult);
    assert!(viewer.read(|view| view.session.result.outline.is_none()));
    assert_eq!(rows(&viewer).len(), 4);
}
