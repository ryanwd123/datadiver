use super::FileEntry;
use super::test_support::{Viewer, empty_frame, repository};
use crate::application::commands::CommandId;
use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, DocumentDefinition, OutlineLayout, QueryConfig, ValueFilter, ViewKind,
};
use crate::infrastructure::json_store::ViewRepository;
use crate::ui::hit::{GridHit, Hit};
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

/// Opens the command menu and presses O, turning outline mode on or off.
fn toggle_outline(viewer: &Viewer) {
    viewer.select(0, 0);
    viewer.render();
    viewer.press("ctrl-k o");
    viewer.run_until_parked();
    viewer.render();
}

/// The title of the open palette.
fn palette_title(viewer: &Viewer) -> Option<String> {
    viewer.read(|view| match &view.dialog {
        Some(super::dialogs::Dialog::Palette(dialog)) => Some(dialog.title.clone()),
        _ => None,
    })
}

#[test]
fn outline_mode_turns_on_from_the_command_menu_and_alt_shift_arrows_change_fields() {
    let (_home, viewer, _) = setup();
    // The outline keys need outline mode.
    press_on_cell(&viewer, 0, 1, "alt-shift-left");
    assert!(status(&viewer).contains("Outline mode is off"), "{}", status(&viewer));
    assert_eq!(config(&viewer).depth, None);
    toggle_outline(&viewer);
    assert_eq!(config(&viewer).outline.map(|outline| outline.layout), Some(OutlineLayout::Compact));
    // Compact by default, with every region's subtotal above its products.
    assert_eq!(
        rows(&viewer),
        [
            text(&["North", "3"]),
            text(&["North", "1"]),
            text(&["North", "2"]),
            text(&["South", "12"]),
            text(&["South", "3"]),
            text(&["South", "9"]),
        ]
    );
    // On a measure, every item of the last level above the groups
    // collapses, keeping its marker to expand it again.
    let level = |viewer: &Viewer| config(viewer).outline.and_then(|outline| outline.level);
    press_on_cell(&viewer, 0, 1, "alt-shift-left");
    assert_eq!(level(&viewer), Some(1));
    assert_eq!(config(&viewer).depth, None);
    assert_eq!(rows(&viewer), [text(&["North", "3"]), text(&["South", "12"])]);
    viewer.read(|view| assert!(view.session.result.outline.is_some()));
    // The top level stays.
    press_on_cell(&viewer, 0, 1, "alt-shift-left");
    assert_eq!(status(&viewer), "This is the outline's top level");
    assert_eq!(level(&viewer), Some(1));
    press_on_cell(&viewer, 0, 0, "alt-left");
    assert_eq!(status(&viewer), "This is the outline's top level");
    assert_eq!(config(&viewer).outline.unwrap().toggled.len(), 0);
    // A click on North's marker opens North alone, and another closes it.
    let click_marker = |row: usize| {
        viewer.render();
        viewer.click(|hit| matches!(hit, Hit::Grid(GridHit::OutlineMarker { row: at, .. }) if *at == row));
        viewer.run_until_parked();
        viewer.render();
    };
    click_marker(0);
    assert_eq!(rows(&viewer), [text(&["North", "3"]), text(&["North", "1"]), text(&["North", "2"]), text(&["South", "12"])]);
    click_marker(0);
    assert_eq!(rows(&viewer).len(), 2);
    press_on_cell(&viewer, 0, 0, "alt-shift-right");
    assert_eq!(level(&viewer), None);
    assert_eq!(rows(&viewer).len(), 6);
    // At the lowest level, expanding lists the columns to expand into.
    press_on_cell(&viewer, 0, 0, "alt-shift-right");
    assert_eq!(palette_title(&viewer).as_deref(), Some("Expand every item into"));
    viewer.press("enter");
    viewer.run_until_parked();
    viewer.render();
    assert_eq!(config(&viewer).group_by.last(), Some(&Dimension::from("amount")));
    assert_eq!(rows(&viewer).len(), 11);
    // Undo takes the new field back out.
    command(&viewer, CommandId::Undo);
    assert_eq!(config(&viewer).group_by.len(), 2);
    // Turning outline mode off shows every row of every field.
    press_on_cell(&viewer, 0, 1, "alt-shift-left");
    assert_eq!(level(&viewer), Some(1));
    toggle_outline(&viewer);
    assert_eq!(config(&viewer).outline, None);
    assert_eq!(config(&viewer).depth, None);
    assert_eq!(rows(&viewer).len(), 4);
    assert!(viewer.read(|view| view.session.result.outline.is_none()));
}

#[test]
fn alt_arrows_collapse_items_into_subtotal_rows_that_drill_and_filter_by_their_keys() {
    let (_home, viewer, path) = setup();
    toggle_outline(&viewer);
    viewer.update(|view, cx| view.set_outline_layout(OutlineLayout::Tabular, cx));
    viewer.run_until_parked();
    // Alt+Left on North's region cell collapses North into one row.
    press_on_cell(&viewer, 0, 0, "alt-left");
    assert_eq!(
        rows(&viewer),
        [text(&["North", "", "3"]), text(&["South", "", "12"]), text(&["South", "A", "3"]), text(&["South", "B", "9"])]
    );
    viewer.read(|view| {
        assert!(view.session.result.outline.is_some());
        // The hidden level and open columns stay out of the grid.
        assert_eq!(view.session.result.column_count(), 3);
    });
    // A leaf's Alt+Left on its measure collapses its parent.
    press_on_cell(&viewer, 3, 2, "alt-left");
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
    // A fresh draft starts flat, outside outline mode.
    open(&viewer, FileEntry::View(query(&path)));
    assert_eq!(rows(&viewer).len(), 4);
    press_on_cell(&viewer, 0, 0, "alt-left");
    assert!(status(&viewer).contains("Outline mode is off"));
    toggle_outline(&viewer);
    viewer.update(|view, cx| view.set_outline_layout(OutlineLayout::Tabular, cx));
    viewer.run_until_parked();
    press_on_cell(&viewer, 0, 0, "alt-left");
    // Filtering by the rolled-up product cell has no key to use.
    press_on_cell(&viewer, 0, 1, "=");
    assert!(viewer.read(|view| view.error.clone()).is_some_and(|error| error.contains("total over every product")));
    viewer.update(|view, _| view.error = None);
    // Alt+Right expands it again; on an expanded row it says so.
    press_on_cell(&viewer, 0, 0, "alt-right");
    assert_eq!(rows(&viewer).len(), 6);
    press_on_cell(&viewer, 0, 0, "alt-right");
    assert!(status(&viewer).contains("expanded already"));
    // On a leaf, Alt+Right lists the columns to open that item into.
    press_on_cell(&viewer, 1, 2, "alt-right");
    assert_eq!(palette_title(&viewer).as_deref(), Some("Expand A into"));
    viewer.press("enter");
    viewer.run_until_parked();
    viewer.render();
    let outline = config(&viewer).outline.unwrap();
    assert_eq!(outline.level, Some(2));
    assert_eq!(outline.toggled, [text(&["North", "A"])]);
    assert_eq!(
        rows(&viewer),
        [
            text(&["North", "", "", "3"]),
            text(&["North", "A", "", "1"]),
            text(&["North", "A", "1", "1"]),
            text(&["North", "B", "", "2"]),
            text(&["South", "", "", "12"]),
            text(&["South", "A", "", "3"]),
            text(&["South", "B", "", "9"]),
        ]
    );
}

#[test]
fn layouts_subtotals_levels_search_and_export_follow_the_outline() {
    let (home, viewer, _) = setup();
    toggle_outline(&viewer);
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
    toggle_outline(&viewer);
    assert!(viewer.read(|view| view.session.result.outline.is_none()));
    assert_eq!(rows(&viewer).len(), 4);
}

#[test]
fn collapsing_keeps_the_widths_and_compact_labels_fit_every_field() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("sales.csv");
    // Products have much longer names than regions.
    std::fs::write(
        &path,
        "region,product,amount\nN,A product with a very long name indeed,1\nN,B,2\nS,Another long product name here,3\n",
    )
    .unwrap();
    let viewer = viewer(repository(home.path()));
    open(&viewer, FileEntry::Source(path.clone()));
    open(&viewer, FileEntry::View(query(&path)));
    toggle_outline(&viewer);
    let settled = |viewer: &Viewer| {
        for _ in 0..20 {
            viewer.render();
            viewer.run_until_parked();
        }
        viewer.read(|view| view.grid.widths.clone())
    };
    let widths = settled(&viewer);
    // The labels column fits the long products, not just the short regions.
    assert!(widths[0] > 200.0, "{widths:?}");
    // Collapsing and expanding change rows, never the widths.
    press_on_cell(&viewer, 0, 1, "alt-shift-left");
    assert_eq!(rows(&viewer).len(), 2);
    assert_eq!(viewer.read(|view| view.grid.widths.clone()), widths);
    press_on_cell(&viewer, 0, 0, "alt-shift-right");
    assert_eq!(viewer.read(|view| view.grid.widths.clone()), widths);
    assert_eq!(settled(&viewer), widths);
}
