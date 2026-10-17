use super::*;
use crate::application::state::ApplicationState;
use crate::domain::documents::RelationKind;
use crate::infrastructure::source_database::database_file_kind;
use crate::ui::database_tree::{DatabaseTreeEvent, DeleteTarget, TreeData, TreeTarget};

/// Says how to make an attached database writable, since a drop attaches it
/// read-only.
fn attach_message(attached: &[(String, PathBuf)], alias: &str, path: &std::path::Path) -> String {
    if attached.len() > 1 {
        let names = attached
            .iter()
            .map(|(alias, _)| alias.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return format!("Attached {names} read-only. To write to one, DETACH it and ATTACH it again in SQL");
    }
    format!(
        "Attached {alias} read-only. To write to it, run in SQL: DETACH {alias}; ATTACH '{}' AS {alias}",
        path.display().to_string().replace('\'', "''")
    )
}

impl DataFrameView {
    /// Handles what the tree reported since the last call.
    pub(crate) fn handle_tree_events(&mut self, cx: &mut Cx<Self>) {
        for event in self.database_tree.take_events() {
            match event {
                DatabaseTreeEvent::Open {
                    target,
                    new_window,
                    pivot_draft,
                } => self.open_tree_target(target, new_window, pivot_draft, cx),
                DatabaseTreeEvent::Delete(target) => self.delete_tree_target(target, cx),
                DatabaseTreeEvent::FocusGrid => self.focus_grid(cx),
                DatabaseTreeEvent::Refresh => self.refresh_database_tree(cx),
                DatabaseTreeEvent::Close => self.set_database_tree_open(false, cx),
            }
        }
    }

    /// Ctrl+B shows the tree and focuses it, or hides it, as in VS Code.
    pub(crate) fn toggle_database_tree(&mut self, cx: &mut Cx<Self>) {
        let open = !self.database_tree_open;
        self.set_database_tree_open(open, cx);
    }

    pub(crate) fn set_database_tree_open(&mut self, open: bool, cx: &mut Cx<Self>) {
        let had_focus = self.focus.is_some_and(|focus| self.database_tree.contains(focus));
        self.database_tree_open = open;
        if open {
            self.refresh_database_tree(cx);
            let mut focus = self.focus;
            self.database_tree.focus(&mut focus);
            self.set_focus(focus, cx);
        } else if had_focus {
            self.focus_grid(cx);
        }
        let saved = self.repository.load_settings().and_then(|mut settings| {
            settings.database_tree_open = open;
            self.repository.save_settings(&settings)
        });
        if let Err(error) = saved {
            self.error = Some(format!("Could not save the database tree state: {error}"));
        }
        cx.notify();
    }

    pub(crate) fn refresh_database_tree(&mut self, cx: &mut Cx<Self>) {
        let repository = self.repository.clone();
        let database = self.database.clone();
        let revision = self.database_tree.begin_refresh();
        cx.notify();
        cx.spawn(
            move || TreeData::load(repository, &database),
            move |view: &mut Self, cx, data| {
                view.database_tree.finish_refresh(revision, data);
                cx.notify();
            },
        )
        .detach();
    }

    /// Attaches dropped DuckDB and SQLite files read-only, then shows the
    /// last one in the database tree. Writing is left to an explicit ATTACH
    /// in SQL, so a drop can never change a database.
    pub(crate) fn attach_databases(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Self>) {
        let database = self.database.clone();
        cx.spawn(
            move || {
                paths
                    .into_iter()
                    .filter_map(|path| {
                        let kind = database_file_kind(&path)?;
                        Some((database.attach(&path, kind), path))
                    })
                    .collect::<Vec<_>>()
            },
            |view: &mut Self, cx, results| {
                let mut attached = Vec::new();
                let mut errors = Vec::new();
                for (result, path) in results {
                    match result {
                        Ok(alias) => attached.push((alias, path)),
                        Err(error) => errors.push(error),
                    }
                }
                if !errors.is_empty() {
                    view.error = Some(errors.join("\n"));
                }
                if let Some((alias, path)) = attached.last() {
                    view.copy_status = Some(attach_message(&attached, alias, path));
                    view.database_tree.reveal(alias);
                    view.set_database_tree_open(true, cx);
                }
                cx.notify();
            },
        )
        .detach();
    }

    /// Delete in the tree: removes what the selected item stands for, asking
    /// first when that cannot be undone or reaches beyond the item.
    fn delete_tree_target(&mut self, target: DeleteTarget, cx: &mut Cx<Self>) {
        match target {
            DeleteTarget::View(definition) => {
                self.archive_view(&definition, cx);
                self.copy_status = Some(format!("Archived {}", definition.name));
                self.refresh_database_tree(cx);
            }
            DeleteTarget::Relation(relation) => {
                if let Some(catalog) = &relation.catalog {
                    self.copy_status = Some(format!(
                        "{} is in {catalog}, which the tree does not change; use SQL to change it",
                        relation.label()
                    ));
                } else if relation.schema == "files" {
                    self.copy_status = Some(format!(
                        "{} is a cached file; remove its watched file or folder under Sources",
                        relation.label()
                    ));
                } else {
                    let kind = if relation.kind == RelationKind::View { "view" } else { "table" };
                    self.confirm(
                        format!("Drop {kind} {}?", relation.label()),
                        "This cannot be undone.".into(),
                        format!("Drop {kind}"),
                        true,
                        dialogs::ConfirmAction::DropRelation(relation),
                        cx,
                    );
                }
            }
            DeleteTarget::FilesSchema => {
                self.copy_status = Some("files holds cached sources; remove watched files or folders under Sources".into());
            }
            DeleteTarget::Database(None) => {
                self.copy_status = Some("The profile database cannot be detached".into());
            }
            DeleteTarget::Database(Some(catalog)) if catalog == "temp" => {
                self.copy_status = Some("temp holds this session's temporary objects; drop them in SQL".into());
            }
            DeleteTarget::Database(Some(catalog)) => {
                let database = self.database.clone();
                self.run_tree_delete(move || database.detach(&catalog), cx);
            }
            DeleteTarget::RecentFile(path) => {
                let result = ApplicationState::load(self.repository.clone())
                    .and_then(|mut state| state.remove_recent_source(&path.to_string_lossy()));
                self.error = result.err().map(|error| format!("Cannot remove the recent file: {error}"));
                self.refresh_database_tree(cx);
            }
            DeleteTarget::WatchedFile(path) => {
                let removed = ApplicationState::load(self.repository.clone())
                    .and_then(|mut state| state.remove_watched_file(&path.to_string_lossy()));
                self.forget_cached(path, removed, cx);
            }
            DeleteTarget::WatchedFolder(root) => {
                self.remove_watched_folder(root, cx);
            }
            DeleteTarget::InWatchedFolder { root, path } => {
                let name = source_file_name(&path);
                self.confirm(
                    format!("Stop watching {}?", root.display()),
                    format!(
                        "{name} is in a watched folder, and DataDiver watches whole folders. \
                         Removing the folder drops the cached tables of all its files. To \
                         leave out only some files, move them out of the folder."
                    ),
                    "Remove folder".into(),
                    false,
                    dialogs::ConfirmAction::RemoveWatchedFolder(root),
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub(crate) fn remove_watched_folder(&mut self, root: PathBuf, cx: &mut Cx<Self>) {
        let removed = ApplicationState::load(self.repository.clone())
            .and_then(|mut state| state.remove_watched_folder(&root.to_string_lossy()));
        self.forget_cached(root, removed, cx);
    }

    /// Drops the cached tables under `path` once it is no longer watched,
    /// keeping files still watched some other way.
    fn forget_cached(&mut self, path: PathBuf, removed: std::io::Result<()>, cx: &mut Cx<Self>) {
        if let Err(error) = removed {
            self.error = Some(format!("Cannot update watched sources: {error}"));
            cx.notify();
            return;
        }
        let database = self.database.clone();
        let repository = self.repository.clone();
        self.run_tree_delete(
            move || {
                let state = ApplicationState::load(repository).map_err(|error| error.to_string())?;
                database.forget_sources(&path, |source| state.is_watched(source))
            },
            cx,
        );
    }

    /// Runs a database change off the UI thread, then shows its error, if
    /// any, and refreshes the tree.
    pub(crate) fn run_tree_delete(&mut self, change: impl FnOnce() -> Result<(), String> + Send + 'static, cx: &mut Cx<Self>) {
        cx.spawn(change, |view: &mut Self, cx, result| {
            if let Err(error) = result {
                view.error = Some(error);
            }
            view.refresh_database_tree(cx);
            cx.notify();
        })
        .detach();
    }

    pub(crate) fn save_database_tree_width(&mut self, width: f32) {
        let width = width.round();
        self.database_tree_width = width;
        let saved = self.repository.load_settings().and_then(|mut settings| {
            settings.database_tree_width = Some(width);
            self.repository.save_settings(&settings)
        });
        if let Err(error) = saved {
            self.error = Some(format!("Could not save the database tree width: {error}"));
        }
    }

    /// Opens a tree leaf here, or in a new window with Shift; Ctrl starts a
    /// pivot draft, as in the files palette.
    fn open_tree_target(&mut self, target: TreeTarget, new_window: bool, pivot_draft: bool, cx: &mut Cx<Self>) {
        let entry = match target {
            TreeTarget::View(definition) => FileEntry::View(*definition),
            TreeTarget::Source(path) => self.resolve_source(FileEntry::Source(path)),
            TreeTarget::Relation(relation) => FileEntry::View(DocumentDefinition::draft_database_view(relation)),
        };
        if new_window {
            self.open_window_with(
                ViewerOptions {
                    open_entry: Some((entry, None)),
                    ..Default::default()
                },
                cx,
            );
            return;
        }
        self.navigate_to_file(entry, false, pivot_draft, false, cx);
        self.focus_grid(cx);
    }
}
