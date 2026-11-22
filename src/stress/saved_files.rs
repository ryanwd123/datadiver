//! Fuzzers for reading damaged saved files: settings, the keymap, the
//! theme, history, views, archives and table preferences that were cut
//! short, garbled, or edited into the wrong shape must load as an error or
//! as something usable, never a panic.
//!
//! [`damage`] is shared with the chaos tests, which damage a profile the
//! monkey filled through the UI and then reopen the app on it.

use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::Value;

use super::Rng;
use crate::application::state::ApplicationState;
use crate::infrastructure::json_store::{StorePaths, ViewRepository};

/// The files the app wrote under `home`: everything but the data files.
pub(crate) fn profile_files(home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let saved = matches!(path.extension().and_then(|e| e.to_str()), Some("json" | "jsonl" | "sql"));
            // The chaos tests' data file is not the app's.
            let fixture = path == home.join("rows.json");
            if path.is_dir() {
                pending.push(path);
            } else if saved && !fixture {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Values a saved field might be replaced with.
fn odd_value(rng: &mut Rng) -> Value {
    match rng.below(14) {
        0 => Value::Null,
        1 => Value::Bool(rng.chance(50)),
        2 => serde_json::json!(-1),
        3 => serde_json::json!(0),
        4 => serde_json::from_str("1e308").unwrap(),
        5 => serde_json::from_str("18446744073709551616").unwrap(),
        6 => serde_json::from_str("-9223372036854775809").unwrap(),
        7 => serde_json::json!(4_294_967_296u64),
        8 => Value::String(String::new()),
        9 => Value::String("x".repeat(10_000)),
        10 => Value::String("日本 🙂 \u{0} \"'".into()),
        11 => Value::Array(Vec::new()),
        12 => Value::Object(Default::default()),
        _ => serde_json::json!(2.5),
    }
}

/// Every place in `value`, as paths of object keys and array indexes.
fn places(value: &Value, path: &mut Vec<Value>, out: &mut Vec<Vec<Value>>) {
    out.push(path.clone());
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                path.push(Value::String(key.clone()));
                places(child, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                path.push(serde_json::json!(index));
                places(child, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(value: &'a mut Value, path: &[Value]) -> &'a mut Value {
    path.iter().fold(value, |value, step| match step {
        Value::String(key) => &mut value[key.as_str()],
        Value::Number(index) => &mut value[index.as_u64().unwrap() as usize],
        _ => unreachable!(),
    })
}

/// Changes one place of a JSON document into the wrong shape.
fn damage_json(mut value: Value, rng: &mut Rng) -> Value {
    let mut all = Vec::new();
    places(&value, &mut Vec::new(), &mut all);
    let path = rng.pick(&all).clone();
    let (parent_path, last) = match path.split_last() {
        Some((last, parent)) => (parent.to_vec(), Some(last.clone())),
        None => (Vec::new(), None),
    };
    let choice = rng.below(6);
    let parent = at(&mut value, &parent_path);
    match (choice, last, parent) {
        // Drop a field or an item.
        (0, Some(Value::String(key)), Value::Object(map)) => {
            map.remove(&key);
        }
        (0, Some(Value::Number(index)), Value::Array(items)) => {
            items.remove(index.as_u64().unwrap() as usize);
        }
        // Repeat an item, or reverse a list.
        (1, Some(Value::Number(index)), Value::Array(items)) => {
            let item = items[index.as_u64().unwrap() as usize].clone();
            items.push(item);
        }
        (2, _, Value::Array(items)) => items.reverse(),
        // Add a field nobody knows.
        (3, _, Value::Object(map)) => {
            map.insert("unexpected_field".into(), odd_value(rng));
        }
        _ => *at(&mut value, &path) = odd_value(rng),
    }
    value
}

/// `bytes` damaged at random: cut short, garbled, emptied, or (for JSON)
/// edited into the wrong shape. `None` deletes the file.
pub(crate) fn damage(bytes: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let json = serde_json::from_slice::<Value>(bytes).ok();
    Some(match rng.below(10) {
        0 => return None,
        1 => Vec::new(),
        2 => bytes[..rng.below(bytes.len() + 1)].to_vec(),
        3 => {
            let mut bytes = bytes.to_vec();
            for _ in 0..1 + rng.below(4) {
                if !bytes.is_empty() {
                    let i = rng.below(bytes.len());
                    bytes[i] = rng.next_u64() as u8;
                }
            }
            bytes
        }
        4 => b"\xef\xbb\xbf".iter().chain(bytes).copied().collect(),
        5 => format!("{}{}", "[".repeat(5_000), "]".repeat(5_000)).into_bytes(),
        _ => match json {
            Some(mut value) => {
                for _ in 0..1 + rng.below(3) {
                    value = damage_json(value, rng);
                }
                serde_json::to_vec_pretty(&value).unwrap()
            }
            None => bytes[..rng.below(bytes.len() + 1)].to_vec(),
        },
    })
}

/// A profile with every kind of saved file, written by the app's own code.
fn saved_profile(home: &Path) -> ViewRepository {
    use crate::domain::documents::{DocumentDefinition, SortKey};
    let repository = ViewRepository::new(StorePaths::for_home(home));
    repository.save_settings(&Default::default()).unwrap();
    repository.save_keymap(&Default::default()).unwrap();
    repository.save_theme(&Default::default()).unwrap();
    repository.save_history(&Default::default()).unwrap();
    repository.save_startup_sql("select 1;").unwrap();
    repository
        .append_sql_history(crate::infrastructure::json_store::SqlHistoryEntry {
            sql: "select 1".into(),
            engine: "duckdb".into(),
            ran_at_unix_ms: 1,
            elapsed_ms: 2,
            rows: Some(1),
            error: None,
        })
        .unwrap();
    let mut view = DocumentDefinition::draft_table("Sales".into(), home.join("data.csv").display().to_string());
    if let crate::domain::documents::ViewKind::Table(table) = &mut view.view {
        table.sort = vec![SortKey { column: "amount".into(), descending: true, sql: false }];
        table.column_order = vec!["amount".into(), "region".into()];
        table.frozen_count = 1;
        table.hidden = vec!["region".into()];
    }
    repository.save_view(&mut view).unwrap();
    let mut other = DocumentDefinition::draft_table("Old".into(), "gone.csv".into());
    repository.save_view(&mut other).unwrap();
    repository.archive(&other.id).unwrap();
    let source = home.join("data.csv");
    let patch = crate::domain::table_preferences::TablePreferencesPatch {
        widths: [("amount".into(), Some(120.0))].into(),
        column_order: Some(vec!["amount".into(), "region".into()]),
        frozen_columns: Some(vec!["amount".into()]),
        slicer_columns: Some(vec!["region".into()]),
        ..Default::default()
    };
    repository.update_table_preferences(&source, &patch).unwrap();
    repository
}

/// Reads everything a profile holds, the way the app does at startup.
fn load_everything(repository: &ViewRepository, home: &Path) {
    if let Ok(settings) = repository.load_settings() {
        let _ = repository.load_keymap(&settings);
    }
    let _ = repository.load_theme();
    let _ = repository.load_history();
    let _ = repository.load_startup_sql();
    let _ = repository.load_sql_history();
    let (views, _) = repository.load_views();
    for view in &views {
        let _ = repository.load_view(&view.id);
        let _ = repository.views_on_sql_source(&view.id);
    }
    let _ = repository.load_archives();
    let _ = repository.load_table_preferences(&home.join("data.csv"));
    let _ = ApplicationState::load(repository.clone());
}

proptest! {
    #![proptest_config(super::config(500))]

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_damaged_saved_files_load(seed in any::<u64>(), count in 1..4usize) {
        let home = tempfile::tempdir().unwrap();
        let repository = saved_profile(home.path());
        let files = profile_files(home.path());
        prop_assert!(files.len() >= 6, "the profile has {files:?}");
        let mut rng = Rng::new(seed);
        for _ in 0..count {
            let file = rng.pick(&files).clone();
            let Ok(bytes) = std::fs::read(&file) else { continue };
            match damage(&bytes, &mut rng) {
                Some(damaged) => std::fs::write(&file, damaged).unwrap(),
                None => std::fs::remove_file(&file).unwrap(),
            }
        }
        load_everything(&repository, home.path());
        // What loads can be saved again.
        if let Ok(settings) = repository.load_settings() {
            repository.save_settings(&settings).unwrap();
        }
        if let Ok(history) = repository.load_history() {
            repository.save_history(&history).unwrap();
        }
        // Saving checks more than loading, such as that a view has a source,
        // so a damaged view may load and then refuse to save; it must not panic.
        for mut view in repository.load_views().0 {
            let _ = repository.save_view(&mut view);
        }
        // A damaged history is set aside, so the history always loads.
        prop_assert!(repository.load_history().is_ok());
        load_everything(&repository, home.path());
    }
}
