//! CST snapshots: each `tests/data/*.sql` beside its tree dump (`.rast`).
//! Run with `UPDATE_EXPECT=1` to rewrite the dumps after a grammar change.

use std::path::Path;

#[test]
fn snapshots() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let update = std::env::var_os("UPDATE_EXPECT").is_some();
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        let actual = diver_sql::parse(&text).debug_tree();
        let rast = path.with_extension("rast");
        let expected = std::fs::read_to_string(&rast)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if actual != expected {
            if update {
                std::fs::write(&rast, &actual).unwrap();
            } else {
                stale.push(rast.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    assert!(
        stale.is_empty(),
        "stale snapshots (rerun with UPDATE_EXPECT=1 and review the diff): {stale:?}"
    );
}
