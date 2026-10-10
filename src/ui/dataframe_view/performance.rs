use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, AppContext, AsyncApp, Entity};

use super::{DataFrameView, FileEntry};
use crate::domain::documents::{DocumentDefinition, SlicerDefinition, SortKey, ViewKind};
use crate::infrastructure::performance;

async fn wait_ready(viewer: &Entity<DataFrameView>, cx: &mut AsyncApp) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let (ready, error) = viewer.update(cx, |view, cx| {
            let grid = view.grid.read(cx);
            (
                !view.loading
                    && !view.busy
                    && grid.benchmark_has_rendered_result()
                    && grid.widths_ready()
                    && grid.result().generation == view.session.result.generation
                    && grid.result().generation == view.session.token().request_revision,
                view.error.clone(),
            )
        });
        if let Some(error) = error {
            return Err(error);
        }
        if ready {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err("timed out waiting for data".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
}

async fn table_switches(
    viewer: &Entity<DataFrameView>,
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let (source, path, repository) = viewer.update(cx, |view, _| {
        (
            view.session.source.clone(),
            view.session.source_path.clone(),
            view.repository.clone(),
        )
    });
    let Some(path) = path else {
        return Ok(());
    };
    let (base, reordered, sorted, sliced, search) = cx.background_executor().spawn(async move {
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
            table.sort = vec![SortKey { column: first.name.clone(), descending: true }];
        }
        repository.save_view(&mut sorted).map_err(|e| e.to_string())?;
        let mut sliced = base.clone();
        sliced.id.clear();
        sliced.name = "Benchmark sliced".into();
        let mut search = None;
        for column in schema.iter().filter(|c| crate::domain::slicers::supported(&c.duckdb_type)) {
            let values = store.slicer_values(&column.name, &[], &[])?;
            if !values.is_empty() && values.len() <= crate::domain::slicers::MAX_UNIQUE_VALUES {
                let selected = values.iter().find(|v| matches!(&v.key, crate::domain::documents::ScalarKey::String(s) if !s.is_empty()))
                    .unwrap_or(&values[0]);
                if let crate::domain::documents::ScalarKey::String(value) = &selected.key {
                    search = value.split_whitespace().next().map(str::to_owned);
                }
                sliced.slicers = vec![SlicerDefinition { sql: false, condition: None, column: column.name.clone(), selected: vec![selected.key.clone()] }];
                break;
            }
        }
        if sliced.slicers.is_empty() { return Err("benchmark needs an eligible slicer".to_owned()); }
        repository.save_view(&mut sliced).map_err(|e| e.to_string())?;
        performance::event(serde_json::json!({"metric":"dataset", "rows":store.row_count(), "columns":schema.len(), "sort_column":first.name, "slicer_column":sliced.slicers[0].column}));
        Ok::<_, String>((base, reordered, sorted, sliced, search.unwrap_or_else(|| "1".into())))
    }).await?;

    for (scenario, a, b, carry) in [
        ("presentation_only", &base, &reordered, false),
        ("different_sort", &base, &sorted, false),
        ("active_slicer", &base, &sliced, false),
        ("carried_search", &base, &reordered, true),
        ("cleared_search", &base, &reordered, false),
    ] {
        // Install the scenario's starting query before collecting warm samples.
        cx.update_window(handle, |_, window, cx| {
            viewer.update(cx, |view, cx| {
                view.set_search_text(
                    if carry || scenario == "cleared_search" {
                        search.clone()
                    } else {
                        String::new()
                    },
                    window,
                    cx,
                )
            })
        })
        .map_err(|e| e.to_string())?;
        wait_ready(viewer, cx).await?;
        for iteration in 0..22 {
            if scenario == "cleared_search" {
                cx.update_window(handle, |_, window, cx| {
                    viewer.update(cx, |view, cx| {
                        view.set_search_text(search.clone(), window, cx)
                    })
                })
                .map_err(|e| e.to_string())?;
                wait_ready(viewer, cx).await?;
            }
            performance::sample(scenario, iteration);
            let start = Instant::now();
            let destination = if iteration % 2 == 0 { a } else { b };
            cx.update_window(handle, |_, _, cx| {
                viewer.update(cx, |view, cx| {
                    view.navigate_to_file(
                        FileEntry::View(destination.clone()),
                        carry,
                        false,
                        false,
                        handle,
                        cx,
                    )
                })
            })
            .map_err(|e| e.to_string())?;
            wait_ready(viewer, cx).await?;
            next_frame(handle, cx, "navigation_to_frame", start).await?;
        }
    }
    Ok(())
}

async fn next_frame(
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
    metric: &'static str,
    start: Instant,
) -> Result<(), String> {
    let (sent, received) = performance::frame_signal();
    cx.update_window(handle, |_, window, _| {
        window.on_next_frame(move |_, _| {
            performance::record(metric, start);
            sent.store(true, Ordering::Release);
        })
    })
    .map_err(|error| error.to_string())?;
    let deadline = Instant::now();
    while !received.load(Ordering::Acquire) {
        if deadline.elapsed() > Duration::from_secs(60) {
            return Err("timed out waiting for a visible window frame".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
    Ok(())
}

pub(super) async fn run(
    viewer: &Entity<DataFrameView>,
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let start = Instant::now();
    wait_ready(viewer, cx).await?;
    next_frame(handle, cx, "first_useful_frame", start).await?;
    table_switches(viewer, handle, cx).await?;
    for text in ["label_42", "region_2", ""] {
        let start = Instant::now();
        cx.update_window(handle, |_, window, cx| {
            viewer.update(cx, |view, cx| view.set_search_text(text.into(), window, cx))
        })
        .map_err(|error| error.to_string())?;
        wait_ready(viewer, cx).await?;
        next_frame(handle, cx, "search_to_frame", start).await?;
    }
    for step in 0..120 {
        let start = Instant::now();
        cx.update_window(handle, |_, _, cx| {
            viewer.update(cx, |view, cx| {
                let count = view.session.result.row_count();
                view.grid.update(cx, |grid, cx| {
                    grid.benchmark_scroll_to((step * 20).min(count.saturating_sub(1)), cx)
                });
            })
        })
        .map_err(|error| error.to_string())?;
        next_frame(handle, cx, "scroll_frame_interval", start).await?;
    }
    Ok(())
}
