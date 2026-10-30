//! The files a profile has open, read by the engine each file is set to:
//! Polars frames held in memory and shared by every window while the file is
//! unchanged, or DuckDB tables in the profile database's in-memory files
//! catalog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use polars::prelude::DataFrame;

use super::memory::{self, MemoryRefusal};
use super::polars::readers::read_frame;
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
    /// The share of the machine's memory the read may take.
    pub memory_limit_percent: u8,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            preserve_order: true,
            memory_limit_percent: super::memory::DEFAULT_LIMIT_PERCENT,
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
        if (mode == LoadMode::Force || !self.holds_current(path, options))
            && let Err(refusal) = memory::check_read(path, options.memory_limit_percent)
        {
            return Err(format!("{refusal}; scan it in place instead (Read mode in View options)"));
        }
        match options.engine {
            Engine::DuckDb => {
                let existing = match existing {
                    Some(SourceIdentity::DuckDb(identity)) => Some(identity),
                    _ => None,
                };
                Ok(
                    match self.database.load_reusing(path, mode, options.preserve_order, existing)? {
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
                self.load_frame(path, mode, existing)
            }
        }
    }

    /// Why `path` should not be read into memory now: it would not fit, and
    /// no unchanged read of it is held already.
    pub fn memory_refusal(&self, path: &Path, options: ReadOptions) -> Option<MemoryRefusal> {
        if self.holds_current(path, options) {
            return None;
        }
        memory::check_read(path, options.memory_limit_percent).err()
    }

    /// Whether an unchanged read of `path` in `options.engine` is held, so
    /// opening it again takes no more memory.
    fn holds_current(&self, path: &Path, options: ReadOptions) -> bool {
        let source_path = absolute_source_path(path);
        match options.engine {
            Engine::DuckDb => self.database.holds_current(&source_path, options.preserve_order),
            Engine::Polars => file_stamp(&source_path).is_ok_and(|stamp| {
                self.cached(&path_key(&source_path), (stamp.modified_ns, stamp.size))
                    .is_some()
            }),
        }
    }

    fn load_frame(
        &self,
        path: &Path,
        mode: LoadMode,
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
            && let Some((version, frame)) = self.cached(&key, stamp)
        {
            let identity = FrameIdentity { key, version };
            if existing == Some(&identity) {
                return Ok(SourceLoad::Reused);
            }
            return Ok(SourceLoad::Loaded(Box::new(
                PolarsStore::from_shared_frame(frame).with_identity(identity).into(),
            )));
        }
        for attempt in 0..2 {
            let frame = read_frame(&source_path)?;
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
                        version,
                        frame: Arc::downgrade(&frame),
                    },
                );
            self.revision.fetch_add(1, Ordering::AcqRel);
            return Ok(SourceLoad::Loaded(Box::new(
                PolarsStore::from_shared_frame(frame)
                    .with_identity(FrameIdentity { key, version })
                    .into(),
            )));
        }
        unreachable!()
    }

    /// The frame of the read that matches `stamp`, while some window holds it.
    fn cached(&self, key: &str, stamp: (i128, u64)) -> Option<(u64, Arc<DataFrame>)> {
        let frames = self.frames.lock().ok()?;
        let entry = frames.get(key)?;
        (entry.stamp == stamp)
            .then(|| entry.frame.upgrade().map(|frame| (entry.version, frame)))
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
            .query_row("SELECT count(*) FROM files.sales_csv", [], |row| row.get(0))
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
                "SELECT count(*) > 0 FROM duckdb_tables() WHERE database_name = 'files' AND table_name = 'sales_csv'",
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
}
