//! The files a profile has open, read by the engine each file is set to:
//! Polars frames held in memory and shared by every window while the file is
//! unchanged, or DuckDB tables in the profile database's in-memory files
//! catalog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use polars::prelude::DataFrame;

use super::polars::readers::{read_frame_limited, with_categorical_text};
use super::polars::store::PolarsStore;
use super::source_database::{
    LoadMode, PersistentSourceIdentity, SourceDatabase, absolute_source_path, file_stamp,
    normalized_path, path_key, source_reader,
};
use super::store::{Engine, Store};

type Profiles = HashMap<PathBuf, Weak<Sources>>;
static PROFILES: OnceLock<Mutex<Profiles>> = OnceLock::new();
/// Numbers every Polars read of a file, across profiles.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

/// How a file is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadOptions {
    pub engine: Engine,
    /// DuckDB only: keep rows in file order. Off reads large files faster.
    pub preserve_order: bool,
    /// The most rows read into memory: the Limit setting.
    pub row_limit: usize,
    /// Polars only: read text columns as categoricals.
    pub categorical: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            preserve_order: true,
            row_limit: super::json_store::DEFAULT_QUERY_LIMIT as usize,
            categorical: false,
        }
    }
}

/// Which read of a file a Polars store holds, so an unchanged file can be reused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameIdentity {
    pub(crate) key: String,
    pub(crate) version: u64,
}

/// Which read of a file a store holds, in either engine.
#[derive(Clone)]
pub enum SourceIdentity {
    DuckDb(PersistentSourceIdentity),
    Polars(FrameIdentity),
}

pub enum SourceLoad {
    Reused,
    Loaded(Box<Store>),
}

/// What a source's freshness depends on, cheap enough to read every second:
/// while it is unchanged, a checked source that was reused is still current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceProbe {
    revision: (u64, u64),
    stamp: (i128, u64),
}

impl SourceProbe {
    /// Whether both probes saw the same file contents, ignoring other reads.
    pub fn same_file(&self, other: &Self) -> bool {
        self.stamp == other.stamp
    }
}

struct CachedFrame {
    stamp: (i128, u64),
    row_limit: usize,
    /// The file had more rows than the limit.
    cut: bool,
    /// Text columns were read as categoricals.
    categorical: bool,
    version: u64,
    /// Dropped once no window shows the file.
    frame: Weak<DataFrame>,
}

/// One registry per profile, shared by its windows.
pub struct Sources {
    database: Arc<SourceDatabase>,
    revision: AtomicU64,
    frames: Mutex<HashMap<String, CachedFrame>>,
}

impl Sources {
    pub fn for_profile(config: &Path) -> Arc<Self> {
        let profile = normalized_path(config);
        let mut profiles = PROFILES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        profiles.retain(|_, sources| sources.strong_count() > 0);
        if let Some(sources) = profiles.get(&profile).and_then(Weak::upgrade) {
            return sources;
        }
        let sources = Arc::new(Self {
            database: SourceDatabase::for_profile(config),
            revision: AtomicU64::new(0),
            frames: Mutex::new(HashMap::new()),
        });
        profiles.insert(profile, Arc::downgrade(&sources));
        sources
    }

    pub fn database(&self) -> &Arc<SourceDatabase> {
        &self.database
    }

    /// Reads the revisions before the file so a change racing the probe makes
    /// the next probe differ. DuckDB's revision also covers SQL writes.
    pub fn probe(&self, path: &Path) -> Option<SourceProbe> {
        let revision = (
            self.revision.load(Ordering::Acquire),
            self.database.revision(),
        );
        let stamp = file_stamp(path).ok()?;
        Some(SourceProbe {
            revision,
            stamp: (stamp.modified_ns, stamp.size),
        })
    }

    /// The file's rows in `options.engine`, read again unless an unchanged
    /// read is still in use and `mode` allows reusing it.
    pub fn load(&self, path: &Path, options: ReadOptions, mode: LoadMode) -> Result<Store, String> {
        match self.load_reusing(path, options, mode, None)? {
            SourceLoad::Loaded(store) => Ok(*store),
            SourceLoad::Reused => unreachable!("no existing source was supplied"),
        }
    }

    /// Like [`Self::load`], but reports [`SourceLoad::Reused`] when `existing`
    /// already holds the current read in the same engine.
    pub fn load_reusing(
        &self,
        path: &Path,
        options: ReadOptions,
        mode: LoadMode,
        existing: Option<&SourceIdentity>,
    ) -> Result<SourceLoad, String> {
        source_reader(path)?;
        match options.engine {
            Engine::DuckDb => {
                let existing = match existing {
                    Some(SourceIdentity::DuckDb(identity)) => Some(identity),
                    _ => None,
                };
                Ok(
                    match self.database.load_reusing(path, mode, options.preserve_order, Some(options.row_limit), existing)? {
                        super::source_database::SourceLoad::Reused => SourceLoad::Reused,
                        super::source_database::SourceLoad::Loaded(store) => {
                            SourceLoad::Loaded(Box::new(Store::from(*store)))
                        }
                    },
                )
            }
            Engine::Polars => {
                let existing = match existing {
                    Some(SourceIdentity::Polars(identity)) => Some(identity),
                    _ => None,
                };
                self.load_frame(path, mode, options.row_limit, options.categorical, existing)
            }
        }
    }

    fn load_frame(
        &self,
        path: &Path,
        mode: LoadMode,
        row_limit: usize,
        categorical: bool,
        existing: Option<&FrameIdentity>,
    ) -> Result<SourceLoad, String> {
        let source_path = absolute_source_path(path);
        let key = path_key(&source_path);
        let stamp_of = |path: &Path| file_stamp(path).map(|stamp| (stamp.modified_ns, stamp.size));
        let mut stamp = stamp_of(&source_path).map_err(|error| {
            self.forget_key(&key);
            format!("cannot access source {}: {error}", path.display())
        })?;
        if mode == LoadMode::Check
            && let Some((version, frame, cut)) = self.cached(&key, stamp, row_limit, categorical)
        {
            let identity = FrameIdentity { key, version };
            if existing == Some(&identity) {
                return Ok(SourceLoad::Reused);
            }
            return Ok(SourceLoad::Loaded(Box::new(
                PolarsStore::from_shared_frame(frame)
                    .with_identity(identity)
                    .with_cut_at(cut.then_some(row_limit))
                    .into(),
            )));
        }
        for attempt in 0..2 {
            let (frame, cut) = read_frame_limited(&source_path, Some(row_limit))?;
            let frame = if categorical { with_categorical_text(frame)? } else { frame };
            let after = stamp_of(&source_path)
                .map_err(|error| format!("cannot verify source after reading: {error}"))?;
            if after != stamp {
                if attempt == 1 {
                    return Err(
                        "source changed repeatedly while reading; try again when the file is stable"
                            .into(),
                    );
                }
                stamp = after;
                continue;
            }
            let frame = Arc::new(frame);
            let version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
            self.frames
                .lock()
                .map_err(|_| "source cache lock poisoned")?
                .insert(
                    key.clone(),
                    CachedFrame {
                        stamp,
                        row_limit,
                        cut,
                        categorical,
                        version,
                        frame: Arc::downgrade(&frame),
                    },
                );
            self.revision.fetch_add(1, Ordering::AcqRel);
            return Ok(SourceLoad::Loaded(Box::new(
                PolarsStore::from_shared_frame(frame)
                    .with_identity(FrameIdentity { key, version })
                    .with_cut_at(cut.then_some(row_limit))
                    .into(),
            )));
        }
        unreachable!()
    }

    /// The frame of the read that matches `stamp`, `row_limit` and
    /// `categorical`, while some window holds it, and whether the file had
    /// more rows.
    fn cached(&self, key: &str, stamp: (i128, u64), row_limit: usize, categorical: bool) -> Option<(u64, Arc<DataFrame>, bool)> {
        let frames = self.frames.lock().ok()?;
        let entry = frames.get(key)?;
        // A read that wasn't cut holds the whole file, enough for any larger limit.
        let covers = entry.row_limit == row_limit || (!entry.cut && row_limit > entry.row_limit);
        (entry.stamp == stamp && covers && entry.categorical == categorical)
            .then(|| entry.frame.upgrade().map(|frame| (entry.version, frame, entry.cut)))
            .flatten()
    }

    fn forget_key(&self, key: &str) {
        if let Ok(mut frames) = self.frames.lock() {
            frames.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(engine: Engine) -> ReadOptions {
        ReadOptions {
            engine,
            ..ReadOptions::default()
        }
    }

    #[test]
    fn both_engines_read_the_same_rows_and_reuse_an_unchanged_read() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("sales.csv");
        std::fs::write(&path, "region,n\nNorth,1\nSouth,2\n").unwrap();
        for engine in Engine::ALL {
            let store = sources.load(&path, options(engine), LoadMode::Check).unwrap();
            assert_eq!(store.engine(), engine);
            assert_eq!(store.row_count(), 2);
            assert_eq!(
                store.schema().iter().map(|column| column.sql_type.as_str()).collect::<Vec<_>>(),
                ["VARCHAR", "BIGINT"]
            );
            assert_eq!(
                store.fetch_row_ids(&[1]).unwrap()[0].values,
                [Some("South".into()), Some("2".into())]
            );
            let identity = store.source_identity().unwrap();
            assert!(matches!(
                sources
                    .load_reusing(&path, options(engine), LoadMode::Check, Some(&identity))
                    .unwrap(),
                SourceLoad::Reused
            ));
            // The other engine never reuses this engine's read.
            let other = Engine::ALL.into_iter().find(|other| *other != engine).unwrap();
            assert!(matches!(
                sources
                    .load_reusing(&path, options(other), LoadMode::Check, Some(&identity))
                    .unwrap(),
                SourceLoad::Loaded(_)
            ));
            assert!(matches!(
                sources
                    .load_reusing(&path, options(engine), LoadMode::Force, Some(&identity))
                    .unwrap(),
                SourceLoad::Loaded(_)
            ));
        }
    }

    #[test]
    fn duckdb_tables_live_in_memory_while_a_store_reads_them() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("sales.csv");
        std::fs::write(&path, "region\nNorth\n").unwrap();
        let store = sources.load(&path, options(Engine::DuckDb), LoadMode::Check).unwrap();
        assert_eq!(sources.database().loaded_table(&path).as_deref(), Some("sales_csv"));
        // SQL sees the table by its file name while the store is open.
        let count: i64 = sources
            .database()
            .connect()
            .unwrap()
            .query_row("SELECT count(*) FROM __dd_files.sales_csv", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(store);
        assert_eq!(sources.database().loaded_table(&path), None);
        // The next load drops the table nothing reads.
        let other = home.path().join("other.csv");
        std::fs::write(&other, "n\n1\n").unwrap();
        let _other = sources.load(&other, options(Engine::DuckDb), LoadMode::Check).unwrap();
        let exists: bool = sources
            .database()
            .connect()
            .unwrap()
            .query_row(
                "SELECT count(*) > 0 FROM duckdb_tables() WHERE database_name = '__dd_files' AND table_name = 'sales_csv'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists);
        // Nothing is written to the profile database.
        assert!(!sources.database().relations().unwrap().iter().any(|relation| relation.name.contains("csv")));
    }

    #[test]
    fn duckdb_reads_without_preserving_order_keep_every_row() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("numbers.csv");
        let mut text = String::from("n\n");
        for n in 0..5000 {
            text.push_str(&format!("{n}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let unordered = ReadOptions {
            engine: Engine::DuckDb,
            preserve_order: false,
            ..ReadOptions::default()
        };
        let store = sources.load(&path, unordered, LoadMode::Check).unwrap();
        assert_eq!(store.row_count(), 5000);
        let identity = store.source_identity().unwrap();
        // Changing the option reads the file again.
        assert!(matches!(
            sources
                .load_reusing(&path, options(Engine::DuckDb), LoadMode::Check, Some(&identity))
                .unwrap(),
            SourceLoad::Loaded(_)
        ));
    }

    #[test]
    fn polars_reads_are_shared_while_unchanged_and_reread_when_the_file_changes() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("sales.csv");
        std::fs::write(&path, "n\n1\n").unwrap();
        let first = sources.load(&path, options(Engine::Polars), LoadMode::Check).unwrap();
        let second = sources.load(&path, options(Engine::Polars), LoadMode::Check).unwrap();
        let (Some(SourceIdentity::Polars(a)), Some(SourceIdentity::Polars(b))) =
            (first.source_identity(), second.source_identity())
        else {
            panic!("Polars identities");
        };
        assert_eq!(a, b);
        std::fs::write(&path, "n\n1\n2\n").unwrap();
        let third = sources.load(&path, options(Engine::Polars), LoadMode::Check).unwrap();
        assert_eq!(third.row_count(), 2);
        assert_eq!(first.row_count(), 1);
    }

    #[test]
    fn reads_stop_at_the_row_limit_and_say_when_the_file_had_more() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(&home.path().join("profile"));
        let csv = home.path().join("rows.csv");
        std::fs::write(&csv, "n
1
2
3
4
").unwrap();
        let parquet = home.path().join("rows.parquet");
        duckdb::Connection::open_in_memory()
            .unwrap()
            .execute(
                "COPY (SELECT range + 1 AS n FROM range(4)) TO ? (FORMAT parquet)",
                [parquet.to_string_lossy().as_ref()],
            )
            .unwrap();
        for path in [&csv, &parquet] {
            for engine in Engine::ALL {
                let read = |row_limit| {
                    let options = ReadOptions { engine, row_limit, ..ReadOptions::default() };
                    sources.load(path, options, LoadMode::Check).unwrap()
                };
                let cut = read(3);
                assert_eq!((cut.row_count(), cut.cut_at()), (3, Some(3)), "{engine:?} {}", path.display());
                let first = cut.fetch_row_ids(&[0, 2]).unwrap();
                assert_eq!(first[0].values, [Some("1".into())]);
                assert_eq!(first[1].values, [Some("3".into())]);
                // A different limit reads the file again rather than reuse the cut read.
                let whole = read(4);
                assert_eq!((whole.row_count(), whole.cut_at()), (4, None), "{engine:?} {}", path.display());
                // A larger limit reuses a read that wasn't cut.
                let identity = whole.source_identity().unwrap();
                let options = ReadOptions { engine, row_limit: 10, ..ReadOptions::default() };
                assert!(matches!(
                    sources.load_reusing(path, options, LoadMode::Check, Some(&identity)).unwrap(),
                    SourceLoad::Reused
                ));
                let again = read(3);
                assert_eq!((again.row_count(), again.cut_at()), (3, Some(3)));
            }
        }
    }
}
