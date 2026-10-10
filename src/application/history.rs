//! Ordered history persistence shared by windows using the same profile.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use crate::domain::documents::DocumentDefinition;
use crate::infrastructure::json_store::ViewRepository;

use super::state::ApplicationState;

static WRITERS: OnceLock<Mutex<HashMap<PathBuf, Arc<HistoryWriter>>>> = OnceLock::new();

enum Message {
    Opened {
        definition: Option<Box<DocumentDefinition>>,
        source: Option<String>,
    },
    Flush(mpsc::SyncSender<()>),
}

pub struct HistoryWriter {
    sender: mpsc::Sender<Message>,
}

impl HistoryWriter {
    pub fn for_repository(repository: &ViewRepository) -> Arc<Self> {
        let mut writers = WRITERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writers
            .entry(crate::infrastructure::source_database::normalized_path(
                &repository.paths.data,
            ))
            .or_insert_with(|| Self::new(repository.clone()))
            .clone()
    }

    fn new(repository: ViewRepository) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    Message::Opened { definition, source } => {
                        let _span =
                            crate::infrastructure::performance::Span::new("history_persistence");
                        let result = (|| {
                            let mut state = ApplicationState::load(repository.clone())?;
                            if let Some(view) = definition.as_ref().filter(|view| {
                                !view.id.is_empty()
                                    && state.views.iter().any(|saved| saved.id == view.id)
                            }) {
                                state.record_view(view)
                            } else if let Some(path) = source {
                                state.record_source(std::path::Path::new(&path))
                            } else {
                                Ok(())
                            }
                        })();
                        if let Err(error) = result {
                            eprintln!("history update failed: {error}");
                        }
                    }
                    Message::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        Arc::new(Self { sender })
    }

    pub fn opened(
        &self,
        definition: Option<DocumentDefinition>,
        source: Option<String>,
    ) -> Result<(), String> {
        self.sender
            .send(Message::Opened {
                definition: definition.map(Box::new),
                source,
            })
            .map_err(|e| e.to_string())
    }

    /// Called after the application loop, so shutdown preserves the latest navigation.
    pub fn flush(&self) {
        let (sent, received) = mpsc::sync_channel(0);
        if self.sender.send(Message::Flush(sent)).is_ok() {
            let _ = received.recv();
        }
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
            writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::json_store::{StorePaths, WindowGeometry};

    #[test]
    fn shared_writer_preserves_order_saved_validation_and_geometry() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut a = DocumentDefinition::draft_table("A".into(), "a.csv".into());
        let mut b = DocumentDefinition::draft_table("B".into(), "b.csv".into());
        repository.save_view(&mut a).unwrap();
        repository.save_view(&mut b).unwrap();
        let writer = HistoryWriter::for_repository(&repository);
        assert!(Arc::ptr_eq(
            &writer,
            &HistoryWriter::for_repository(&repository)
        ));
        writer.opened(Some(a), Some("a.csv".into())).unwrap();
        writer
            .opened(Some(b.clone()), Some("b.csv".into()))
            .unwrap();
        let geometry = WindowGeometry {
            x: 10.,
            y: 20.,
            width: 900.,
            height: 600.,
            maximized: false,
        };
        repository.save_window_geometry(&geometry).unwrap();
        writer.flush();
        let history = repository.load_history().unwrap();
        assert_eq!(history.last_view_id, Some(b.id.clone()));
        assert_eq!(history.recent_sources, ["b.csv", "a.csv"]);
        assert_eq!(history.primary_window, Some(geometry));
        repository.archive(&b.id).unwrap();
        writer.opened(Some(b), Some("b.csv".into())).unwrap();
        writer.flush();
        assert!(repository.load_history().unwrap().last_view_id.is_none());
    }
}
