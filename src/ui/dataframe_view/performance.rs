//! The `--benchmark-ui` script: timings of switching views, searching, and
//! scrolling, written to the performance log.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::startup::Step;
use super::{DataFrameView, FileEntry};
use crate::domain::documents::{DocumentDefinition, SlicerDefinition, SortKey, ViewKind};
use crate::infrastructure::performance;

fn ready(view: &DataFrameView) -> Result<bool, String> {
    if let Some(error) = &view.error {
        return Err(error.clone());
    }
    let grid = &view.grid;
    Ok(!view.loading
        && !view.busy
        && grid.benchmark_has_rendered_result()
        && grid.widths_ready()
        && grid.result().generation == view.session.result.generation
        && grid.result().generation == view.session.token().request_revision)
}

fn wait_ready() -> Step {
    Step::Wait(Box::new(ready), Duration::from_secs(60), "timed out waiting for data")
}

fn run(f: impl FnOnce(&mut DataFrameView, &mut crate::ui::platform::Cx<DataFrameView>) -> Result<(), String> + 'static) -> Step {
    Step::Run(Box::new(f))
}

/// The saved views the table-switch scenarios move between.
#[derive(Default)]
struct Scenarios {
    views: Option<(DocumentDefinition, DocumentDefinition, DocumentDefinition, DocumentDefinition, String)>,
}

fn prepare(view: &DataFrameView) -> Result<Option<(DocumentDefinition, DocumentDefinition, DocumentDefinition, DocumentDefinition, String)>, String> {
    let Some(path) = view.session.source_path.clone() else {
        return Ok(None);
    };
    let source = view.session.source.clone();
    let repository = view.repository.clone();
    let store = source.store.lock().map_err(|_| "store lock poisoned")?;
    let schema = store.schema();
    let first = schema.first().ok_or("benchmark needs columns")?;
    let mut base = DocumentDefinition::draft_table("Benchmark base".into(), path);
    repository.save_view(&mut base).map_err(|e| e.to_string())?;
    let mut reordered = base.clone();
    reordered.id.clear();
    reordered.name = "Benchmark reordered".into();
    if let ViewKind::Table(table) = &mut reordered.view {
        table.column_order = schema.iter().rev().map(|c| c.name.clone()).collect();
        table.frozen_count = 1;
    }
    repository.save_view(&mut reordered).map_err(|e| e.to_string())?;
    let mut sorted = base.clone();
    sorted.id.clear();
    sorted.name = "Benchmark sorted".into();
    if let ViewKind::Table(table) = &mut sorted.view {
        table.sort = vec![SortKey {
            column: first.name.clone(),
            descending: true,
        }];
    }
    repository.save_view(&mut sorted).map_err(|e| e.to_string())?;
    let mut sliced = base.clone();
    sliced.id.clear();
    sliced.name = "Benchmark sliced".into();
    let mut search = None;
    for column in schema.iter().filter(|c| crate::domain::slicers::supported(&c.sql_type)) {
        let values = store.slicer_values(&column.name, &[], &[])?;
        if !values.is_empty() && values.len() <= crate::domain::slicers::MAX_UNIQUE_VALUES {
            let selected = values
                .iter()
                .find(|v| matches!(&v.key, crate::domain::documents::ScalarKey::String(s) if !s.is_empty()))
                .unwrap_or(&values[0]);
            if let crate::domain::documents::ScalarKey::String(value) = &selected.key {
                search = value.split_whitespace().next().map(str::to_owned);
            }
            sliced.slicers = vec![SlicerDefinition {
                sql: false,
                condition: None,
                column: column.name.clone(),
                selected: vec![selected.key.clone()],
            }];
            break;
        }
    }
    if sliced.slicers.is_empty() {
        return Err("benchmark needs an eligible slicer".to_owned());
    }
    repository.save_view(&mut sliced).map_err(|e| e.to_string())?;
    performance::event(serde_json::json!({"metric":"dataset", "rows":store.row_count(), "columns":schema.len(), "sort_column":first.name, "slicer_column":sliced.slicers[0].column}));
    Ok(Some((base, reordered, sorted, sliced, search.unwrap_or_else(|| "1".into()))))
}

/// The whole benchmark as script steps.
pub(super) fn steps() -> VecDeque<Step> {
    let mut steps = VecDeque::new();
    let start = Rc::new(Cell::new(Instant::now()));
    steps.push_back(wait_ready());
    steps.push_back(Step::Frame("first_useful_frame", start.clone()));
    let scenarios = Rc::new(RefCell::new(Scenarios::default()));
    let prepared = scenarios.clone();
    steps.push_back(run(move |view, _| {
        prepared.borrow_mut().views = prepare(view)?;
        Ok(())
    }));
    for (scenario, first, second, carry) in [
        ("presentation_only", 0, 1, false),
        ("different_sort", 0, 2, false),
        ("active_slicer", 0, 3, false),
        ("carried_search", 0, 1, true),
        ("cleared_search", 0, 1, false),
    ] {
        let views = scenarios.clone();
        // Install the scenario's starting query before collecting warm samples.
        steps.push_back(run(move |view, cx| {
            let Some((.., search)) = views.borrow().views.clone() else {
                return Ok(());
            };
            let text = if carry || scenario == "cleared_search" { search } else { String::new() };
            view.set_search_text(text, cx);
            Ok(())
        }));
        steps.push_back(wait_ready());
        for iteration in 0..22 {
            if scenario == "cleared_search" {
                let views = scenarios.clone();
                steps.push_back(run(move |view, cx| {
                    if let Some((.., search)) = views.borrow().views.clone() {
                        view.set_search_text(search, cx);
                    }
                    Ok(())
                }));
                steps.push_back(wait_ready());
            }
            let views = scenarios.clone();
            let frame_start = start.clone();
            steps.push_back(run(move |view, cx| {
                let Some((base, reordered, sorted, sliced, _)) = views.borrow().views.clone() else {
                    return Ok(());
                };
                performance::sample(scenario, iteration);
                frame_start.set(Instant::now());
                let all = [base, reordered, sorted, sliced];
                let destination = if iteration % 2 == 0 { &all[first] } else { &all[second] };
                view.navigate_to_file(FileEntry::View(destination.clone()), carry, false, false, cx);
                Ok(())
            }));
            steps.push_back(wait_ready());
            steps.push_back(Step::Frame("navigation_to_frame", start.clone()));
        }
    }
    for text in ["label_42", "region_2", ""] {
        let frame_start = start.clone();
        steps.push_back(run(move |view, cx| {
            frame_start.set(Instant::now());
            view.set_search_text(text.into(), cx);
            Ok(())
        }));
        steps.push_back(wait_ready());
        steps.push_back(Step::Frame("search_to_frame", start.clone()));
    }
    for step in 0..120 {
        let frame_start = start.clone();
        steps.push_back(run(move |view, cx| {
            frame_start.set(Instant::now());
            let count = view.session.result.row_count();
            view.grid.benchmark_scroll_to((step * 20).min(count.saturating_sub(1)));
            cx.notify();
            Ok(())
        }));
        steps.push_back(Step::Frame("scroll_frame_interval", start.clone()));
    }
    steps
}
