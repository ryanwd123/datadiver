use std::io;
use std::path::Path;

use crate::application::keymap::KeymapFile;
use crate::domain::documents::DocumentDefinition;
use crate::infrastructure::json_store::{
    ApplicationHistory, FavoriteTarget, LoadIssue, Settings, ViewRepository,
};

pub struct ApplicationState {
    repository: ViewRepository,
    pub settings: Settings,
    pub keymap: KeymapFile,
    pub history: ApplicationHistory,
    pub views: Vec<DocumentDefinition>,
    pub view_issues: Vec<LoadIssue>,
}

impl ApplicationState {
    pub fn load(repository: ViewRepository) -> io::Result<Self> {
        let settings = repository.load_settings()?;
        let keymap = repository.load_keymap(&settings)?;
        let history = repository.load_history()?;
        let (views, view_issues) = repository.load_views();
        Ok(Self {
            repository,
            settings,
            keymap,
            history,
            views,
            view_issues,
        })
    }

    pub fn record_source(&mut self, path: &Path) -> io::Result<()> {
        let path = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        self.history = self.repository.update_history(|history| {
            history.last_raw_source = Some(path.clone());
            history.last_view_id = None;
            push_recent(history, path);
        })?;
        Ok(())
    }

    pub fn record_view(&mut self, view: &DocumentDefinition) -> io::Result<()> {
        self.history = self.repository.update_history(|history| {
            history.last_view_id = Some(view.id.clone());
            history.last_raw_source = None;
            let file = view
                .database_view
                .as_ref()
                .and_then(|relation| relation.file_path());
            push_recent(history, file.unwrap_or(&view.source_path).to_owned());
        })?;
        Ok(())
    }

    pub fn set_favorite(&mut self, slot: usize, target: Option<FavoriteTarget>) -> io::Result<()> {
        if slot >= 9 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "favorite slot must be 1 through 9",
            ));
        }
        let mut settings = self.settings.clone();
        settings.favorites[slot] = target;
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }

    pub fn add_watched_folder(&mut self, path: &Path) -> io::Result<()> {
        if !path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("folder does not exist: {}", path.display()),
            ));
        }
        let path = path.canonicalize()?.to_string_lossy().into_owned();
        if self
            .settings
            .watched_folders
            .iter()
            .any(|saved| same_path(saved, &path))
        {
            return Ok(());
        }
        let mut settings = self.settings.clone();
        settings.watched_folders.push(path);
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }

    pub fn remove_watched_folder(&mut self, path: &str) -> io::Result<()> {
        let mut settings = self.settings.clone();
        settings
            .watched_folders
            .retain(|saved| !same_path(saved, path));
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }

    /// Adds files to the cache list, skipping ones already watched directly
    /// or through a folder.
    pub fn add_watched_files(&mut self, paths: &[std::path::PathBuf]) -> io::Result<()> {
        let mut settings = self.settings.clone();
        for path in paths {
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("not a file: {}", path.display()),
                ));
            }
            if watched_in(&settings, path) {
                continue;
            }
            settings
                .watched_files
                .push(path.canonicalize()?.to_string_lossy().into_owned());
        }
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }

    pub fn remove_watched_file(&mut self, path: &str) -> io::Result<()> {
        let mut settings = self.settings.clone();
        settings
            .watched_files
            .retain(|saved| !same_path(saved, path));
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }

    /// Whether `path` is cached: a watched file, or a file under a watched folder.
    pub fn is_watched(&self, path: &Path) -> bool {
        watched_in(&self.settings, path)
    }

    pub fn remove_recent_source(&mut self, path: &str) -> io::Result<()> {
        self.history = self.repository.update_history(|history| {
            history
                .recent_sources
                .retain(|saved| !same_path(saved, path));
            if history
                .last_raw_source
                .as_deref()
                .is_some_and(|saved| same_path(saved, path))
            {
                history.last_raw_source = None;
            }
        })?;
        Ok(())
    }

    pub fn set_binding(
        &mut self,
        command_id: &str,
        binding: Option<String>,
    ) -> io::Result<Option<String>> {
        let mut settings = self.settings.clone();
        let binding = binding.unwrap_or_default();
        let displaced = settings
            .keybindings
            .iter()
            .find(|(id, key)| {
                id.as_str() != command_id
                    && !binding.is_empty()
                    && key.eq_ignore_ascii_case(&binding)
            })
            .map(|(id, _)| id.clone());
        if let Some(id) = &displaced {
            settings.keybindings.insert(id.clone(), String::new());
        }
        settings.keybindings.insert(command_id.into(), binding);
        self.repository.save_settings(&settings)?;
        self.settings = settings;
        Ok(displaced)
    }

    pub fn save_keymap(&mut self, keymap: KeymapFile) -> io::Result<()> {
        self.repository.save_keymap(&keymap)?;
        self.keymap = keymap;
        Ok(())
    }

    pub fn refresh_views(&mut self) {
        let (views, issues) = self.repository.load_views();
        self.views = views;
        self.view_issues = issues;
    }

    pub fn repository(&self) -> &ViewRepository {
        &self.repository
    }
}

fn same_path(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn watched_in(settings: &Settings, path: &Path) -> bool {
    use crate::infrastructure::source_database::normalized_path;
    let key = normalized_path(path);
    settings
        .watched_files
        .iter()
        .any(|file| normalized_path(Path::new(file)) == key)
        || settings
            .watched_folders
            .iter()
            .any(|folder| key.starts_with(normalized_path(Path::new(folder))))
}

fn push_recent(history: &mut ApplicationHistory, path: String) {
    history
        .recent_sources
        .retain(|saved| !saved.trim().is_empty() && !same_path(saved, &path));
    if path.trim().is_empty() {
        return;
    }
    history.recent_sources.insert(0, path);
    history.recent_sources.truncate(15);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::json_store::StorePaths;

    #[test]
    fn records_recent_sources_without_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository).unwrap();
        state.record_source(Path::new("C:\\a.csv")).unwrap();
        state.record_source(Path::new("C:\\a.csv")).unwrap();
        assert_eq!(state.history.recent_sources.len(), 1);
    }

    #[test]
    fn binding_conflict_displaces_previous_command() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository).unwrap();
        state.set_binding("one", Some("ctrl-p".into())).unwrap();
        assert_eq!(
            state.set_binding("two", Some("CTRL-P".into())).unwrap(),
            Some("one".into())
        );
        assert_eq!(state.settings.keybindings.get("one").unwrap(), "");
    }

    #[test]
    fn records_saved_view_as_restore_target() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository).unwrap();
        let mut view = DocumentDefinition::draft_table("View".into(), "C:\\file.csv".into());
        view.id = "id-1".into();
        state.record_view(&view).unwrap();
        assert_eq!(state.history.last_view_id.as_deref(), Some("id-1"));
        assert!(state.history.last_raw_source.is_none());
        assert_eq!(state.history.recent_sources, vec!["C:\\file.csv"]);
    }

    #[test]
    fn saved_sql_does_not_record_an_empty_source_and_old_blanks_are_removed() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        repository
            .save_history(&ApplicationHistory {
                recent_sources: vec!["data.csv".into(), String::new(), "  ".into()],
                ..Default::default()
            })
            .unwrap();
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        assert_eq!(state.history.recent_sources, ["data.csv"]);
        let mut view = DocumentDefinition::draft_table("SQL".into(), String::new());
        view.id = "sql-id".into();
        view.view = crate::domain::documents::ViewKind::Sql(Default::default());
        state.record_view(&view).unwrap();
        assert_eq!(state.history.last_view_id.as_deref(), Some("sql-id"));
        assert_eq!(state.history.recent_sources, ["data.csv"]);
        let json: ApplicationHistory = serde_json::from_slice(
            &std::fs::read(repository.paths.data.join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json.recent_sources, ["data.csv"]);
    }

    #[test]
    fn saved_view_favorites_persist_by_id_alongside_source_favorites() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        let target = FavoriteTarget::View {
            view_id: "saved-id".into(),
        };
        state.set_favorite(0, Some(target.clone())).unwrap();
        state
            .set_favorite(1, Some(FavoriteTarget::Source("data.csv".into())))
            .unwrap();
        let loaded = ApplicationState::load(repository).unwrap();
        assert_eq!(loaded.settings.favorites[0], Some(target));
        assert_eq!(
            loaded.settings.favorites[1],
            Some(FavoriteTarget::Source("data.csv".into()))
        );
    }

    #[test]
    fn watched_folders_persist_without_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let watched = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        state.add_watched_folder(watched.path()).unwrap();
        state.add_watched_folder(watched.path()).unwrap();
        assert_eq!(state.settings.watched_folders.len(), 1);
        let saved = ApplicationState::load(repository.clone()).unwrap();
        assert_eq!(
            saved.settings.watched_folders,
            state.settings.watched_folders
        );
        state
            .remove_watched_folder(&saved.settings.watched_folders[0])
            .unwrap();
        assert!(
            ApplicationState::load(repository)
                .unwrap()
                .settings
                .watched_folders
                .is_empty()
        );
    }

    #[test]
    fn watched_files_and_folders_decide_what_is_cached() {
        let home = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let loose = tempfile::tempdir().unwrap();
        let inside = folder.path().join("inside.csv");
        let single = loose.path().join("single.csv");
        let other = loose.path().join("other.csv");
        for path in [&inside, &single, &other] {
            std::fs::write(
                path, "n\n1\n",
            )
            .unwrap();
        }
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        state.add_watched_folder(folder.path()).unwrap();
        // A file under a watched folder is already cached, so it is not added again.
        state
            .add_watched_files(&[single.clone(), inside.clone(), single.clone()])
            .unwrap();
        assert_eq!(state.settings.watched_files.len(), 1);
        assert!(state.add_watched_files(&[loose.path().into()]).is_err());
        let saved = ApplicationState::load(repository.clone()).unwrap();
        assert!(saved.is_watched(&inside));
        assert!(saved.is_watched(&single));
        assert!(!saved.is_watched(&other));
        state
            .remove_watched_file(&saved.settings.watched_files[0])
            .unwrap();
        assert!(
            !ApplicationState::load(repository)
                .unwrap()
                .is_watched(&single)
        );
    }

    #[test]
    fn removing_a_recent_source_forgets_it_for_restore() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        state.record_source(Path::new("a.csv")).unwrap();
        state.record_source(Path::new("b.csv")).unwrap();
        state.remove_recent_source("b.csv").unwrap();
        let saved = ApplicationState::load(repository).unwrap();
        assert_eq!(saved.history.recent_sources, ["a.csv"]);
        assert_eq!(saved.history.last_raw_source, None);
    }

    #[test]
    fn favorites_persist_by_slot() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut state = ApplicationState::load(repository.clone()).unwrap();
        state
            .set_favorite(8, Some(FavoriteTarget::Source("C:\\data.csv".into())))
            .unwrap();
        assert_eq!(
            ApplicationState::load(repository.clone())
                .unwrap()
                .settings
                .favorites[8],
            Some(FavoriteTarget::Source("C:\\data.csv".into()))
        );
        state.set_favorite(8, None).unwrap();
        assert!(
            ApplicationState::load(repository)
                .unwrap()
                .settings
                .favorites[8]
                .is_none()
        );
        assert!(state.set_favorite(9, None).is_err());
    }
}
