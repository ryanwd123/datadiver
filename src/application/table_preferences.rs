//! One ordered, debounced preferences writer per profile, independent of window lifetime.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::domain::table_preferences::{TablePreferences, TablePreferencesPatch};
use crate::infrastructure::json_store::ViewRepository;
use crate::infrastructure::source_database::normalized_path;

static WRITERS: OnceLock<Mutex<HashMap<PathBuf, Arc<TablePreferencesWriter>>>> = OnceLock::new();
type Reply = mpsc::Sender<Result<(), String>>;

enum Message {
    Edit(PathBuf, TablePreferencesPatch, Reply),
    Load(PathBuf, mpsc::SyncSender<Result<TablePreferences, String>>),
    Flush(mpsc::SyncSender<Result<(), String>>),
}

struct Pending {
    patch: TablePreferencesPatch,
    due: Option<Instant>,
    replies: Vec<Reply>,
}

pub struct TablePreferencesWriter {
    sender: mpsc::Sender<Message>,
}

impl TablePreferencesWriter {
    pub fn for_repository(repository: &ViewRepository) -> Arc<Self> {
        let mut writers = WRITERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writers
            .entry(normalized_path(&repository.paths.data))
            .or_insert_with(|| Self::new(repository.clone()))
            .clone()
    }

    fn new(repository: ViewRepository) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut pending = HashMap::<PathBuf, Pending>::new();
            loop {
                let wait = pending
                    .values()
                    .filter_map(|entry| entry.due)
                    .min()
                    .map(|due| due.saturating_duration_since(Instant::now()));
                let message = match wait {
                    Some(wait) => match receiver.recv_timeout(wait) {
                        Ok(message) => Some(message),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    },
                    None => match receiver.recv() {
                        Ok(message) => Some(message),
                        Err(_) => break,
                    },
                };
                match message {
                    Some(Message::Edit(path, patch, reply)) => {
                        let entry = pending.entry(path).or_insert_with(|| Pending {
                            patch: Default::default(),
                            due: None,
                            replies: Vec::new(),
                        });
                        entry.patch.merge(patch);
                        entry.due = Some(Instant::now() + Duration::from_millis(250));
                        entry.replies.push(reply);
                    }
                    Some(Message::Load(path, reply)) => {
                        let result = repository
                            .load_table_preferences(&path)
                            .map(|mut preferences| {
                                if let Some(entry) = pending.get(&path) {
                                    entry.patch.apply(&mut preferences);
                                }
                                preferences
                            })
                            .map_err(|error| error.to_string());
                        let _ = reply.send(result);
                    }
                    Some(Message::Flush(reply)) => {
                        let paths = pending.keys().cloned().collect::<Vec<_>>();
                        let mut result = Ok(());
                        for path in paths {
                            if let Err(error) = write_pending(&repository, &mut pending, &path) {
                                result = Err(error);
                            }
                        }
                        let _ = reply.send(result);
                    }
                    None => {
                        let now = Instant::now();
                        let paths = pending
                            .iter()
                            .filter(|(_, entry)| entry.due.is_some_and(|due| due <= now))
                            .map(|(path, _)| path.clone())
                            .collect::<Vec<_>>();
                        for path in paths {
                            let _ = write_pending(&repository, &mut pending, &path);
                        }
                    }
                }
            }
        });
        Arc::new(Self { sender })
    }

    pub fn edit(
        &self,
        source: PathBuf,
        patch: TablePreferencesPatch,
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(Message::Edit(normalized_path(&source), patch, sender))
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }

    /// Call on a worker before navigation; includes edits still in the debounce interval.
    pub fn load(&self, source: &std::path::Path) -> Result<TablePreferences, String> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.sender
            .send(Message::Load(normalized_path(source), sender))
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn flush(&self) -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.sender
            .send(Message::Flush(sender))
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn flush_profiles() {
        let writers = WRITERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for writer in writers {
            if let Err(error) = writer.flush() {
                eprintln!("table preferences save failed: {error}");
            }
        }
    }
}

fn write_pending(
    repository: &ViewRepository,
    pending: &mut HashMap<PathBuf, Pending>,
    path: &PathBuf,
) -> Result<(), String> {
    let Some(mut entry) = pending.remove(path) else {
        return Ok(());
    };
    let result = repository
        .update_table_preferences(path, &entry.patch)
        .map(|_| ())
        .map_err(|error| error.to_string());
    for reply in entry.replies.drain(..) {
        let _ = reply.send(result.clone());
    }
    if result.is_err() {
        entry.due = None; // Keep edits for an explicit flush or the next edit, without retry loops.
        pending.insert(path.clone(), entry);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_merges_windows_exposes_pending_edits_and_flushes() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let writer = TablePreferencesWriter::new(repository.clone());
        let path = home.path().join("source.csv");
        let first = writer
            .edit(
                path.clone(),
                TablePreferencesPatch {
                    widths: [("a".into(), Some(100.))].into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let second = writer
            .edit(
                path.clone(),
                TablePreferencesPatch {
                    widths: [("b".into(), Some(200.))].into(),
                    slicer_columns: Some(vec!["a".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(writer.load(&path).unwrap().widths.len(), 2);
        writer.flush().unwrap();
        first.recv().unwrap().unwrap();
        second.recv().unwrap().unwrap();
        let saved = repository.load_table_preferences(&path).unwrap();
        assert_eq!(saved.widths.len(), 2);
        assert_eq!(saved.slicer_columns, ["a"]);
    }

    #[test]
    fn failed_edits_survive_and_retry_without_destroying_malformed_files() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let path = home.path().join("source.csv");
        let file = repository.table_preferences_path(&path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "broken").unwrap();
        let writer = TablePreferencesWriter::new(repository.clone());
        let result = writer
            .edit(
                path.clone(),
                TablePreferencesPatch {
                    widths: [("a".into(), Some(100.))].into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(writer.flush().is_err());
        assert!(result.recv().unwrap().is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "broken");
        std::fs::write(
            &file,
            serde_json::to_string(&TablePreferences::new(
                normalized_path(&path).to_string_lossy().into_owned(),
            ))
            .unwrap(),
        )
        .unwrap();
        writer.flush().unwrap();
        assert_eq!(
            repository.load_table_preferences(&path).unwrap().widths["a"],
            100.
        );
    }

    #[cfg(windows)]
    #[test]
    fn locked_destination_retains_old_file_and_flush_retries_latest_edit() {
        use std::os::windows::fs::OpenOptionsExt;
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let path = home.path().join("source.csv");
        repository
            .update_table_preferences(
                &path,
                &TablePreferencesPatch {
                    widths: [("a".into(), Some(100.))].into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let file = repository.table_preferences_path(&path);
        let previous = std::fs::read_to_string(&file).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&file)
            .unwrap();
        let writer = TablePreferencesWriter::new(repository.clone());
        let reply = writer
            .edit(
                path.clone(),
                TablePreferencesPatch {
                    widths: [("a".into(), Some(120.))].into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(writer.flush().is_err());
        assert!(reply.recv().unwrap().is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), previous);
        drop(lock);
        writer.flush().unwrap();
        assert_eq!(
            repository.load_table_preferences(&path).unwrap().widths["a"],
            120.
        );
    }
}
