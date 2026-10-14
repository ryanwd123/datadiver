use super::*;
use crate::domain::documents::DocumentDefinition;
use crate::infrastructure::source_database::database_file_kind;
use crate::domain::documents::RelationKind;
use crate::ui::database_tree::{DatabaseTree, DatabaseTreeEvent, DeleteTarget, TreeTarget};

/// Says how to make an attached database writable, since a drop attaches it
/// read-only.
fn attach_message(attached: &[(String, PathBuf)], alias: &str, path: &std::path::Path) -> String {
    if attached.len() > 1 {
        let names = attached
            .iter()
            .map(|(alias, _)| alias.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "Attached {names} read-only. To write to one, DETACH it and ATTACH it again in SQL"
        );
    }
    format!(
        "Attached {alias} read-only. To write to it, run in SQL: DETACH {alias}; ATTACH '{}' AS {alias}",
        path.display().to_string().replace('\'', "''")
    )
}

impl DataFrameView {
    pub(super) fn new_database_tree(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<DatabaseTree>, Subscription) {
        let tree = cx.new(|cx| DatabaseTree::new(window, cx));
        let subscription =
            cx.subscribe_in(&tree, window, |view, _, event, window, cx| match event {
                DatabaseTreeEvent::Open {
                    target,
                    new_window,
                    pivot_draft,
                } => view.open_tree_target(target.clone(), *new_window, *pivot_draft, window, cx),
                DatabaseTreeEvent::Delete(target) => {
                    view.delete_tree_target(target.clone(), window, cx)
                }
                DatabaseTreeEvent::FocusGrid => {
                    window.focus(&view.grid.read(cx).focus_handle(cx), cx)
                }
                DatabaseTreeEvent::Refresh => view.refresh_database_tree(cx),
                DatabaseTreeEvent::Close => view.set_database_tree_open(false, window, cx),
            });
        (tree, subscription)
    }

    /// Ctrl+B shows the tree and focuses it, or hides it, as in VS Code.
    pub(super) fn toggle_database_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = !self.database_tree_open;
        self.set_database_tree_open(open, window, cx);
    }

    fn set_database_tree_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.database_tree.read(cx).contains_focus(window, cx);
        self.database_tree_open = open;
        if open {
            self.refresh_database_tree(cx);
            self.database_tree
                .update(cx, |tree, cx| tree.focus(window, cx));
        } else if had_focus {
            window.focus(&self.grid.read(cx).focus_handle(cx), cx);
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

    pub(super) fn refresh_database_tree(&mut self, cx: &mut Context<Self>) {
        let repository = self.repository.clone();
        let database = self.database.clone();
        self.database_tree
            .update(cx, |tree, cx| tree.refresh(repository, database, cx));
    }

    /// Attaches dropped DuckDB and SQLite files read-only, then shows the
    /// last one in the database tree. Writing is left to an explicit ATTACH
    /// in SQL, so a drop can never change a database.
    pub(super) fn attach_databases(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let database = self.database.clone();
        cx.spawn_in(window, async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(|path| {
                            let kind = database_file_kind(&path)?;
                            Some((database.attach(&path, kind), path))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                let mut attached = Vec::new();
                let mut errors = Vec::new();
                for (result, path) in results {
                    match result {
                        Ok(alias) => attached.push((alias, path)),
                        Err(error) => errors.push(error),
                    }
                }
                if !errors.is_empty() {
                    view.error = Some(errors.join(
                        "
",
                    ));
                }
                if let Some((alias, path)) = attached.last() {
                    view.copy_status = Some(attach_message(&attached, alias, path));
                    view.database_tree.update(cx, |tree, _| tree.reveal(alias));
                    view.set_database_tree_open(true, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Delete in the tree: removes what the selected item stands for, asking
    /// first when that cannot be undone or reaches beyond the item.
    fn delete_tree_target(
        &mut self,
        target: DeleteTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            DeleteTarget::View(definition) => {
                self.archive_view(&definition, window, cx);
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
                    let kind = if relation.kind == RelationKind::View {
                        "view"
                    } else {
                        "table"
                    };
                    self.confirm_tree_delete(
                        format!("Drop {kind} {}?", relation.label()),
                        "This cannot be undone.".into(),
                        format!("Drop {kind}"),
                        move |view, cx| {
                            let database = view.database.clone();
                            let relation = relation.clone();
                            view.run_tree_delete(move || database.drop_relation(&relation), cx);
                        },
                        window,
                        cx,
                    );
                }
            }
            DeleteTarget::FilesSchema => {
                self.copy_status = Some(
                    "files holds cached sources; remove watched files or folders under Sources"
                        .into(),
                );
            }
            DeleteTarget::Database(None) => {
                self.copy_status = Some("The profile database cannot be detached".into());
            }
            DeleteTarget::Database(Some(catalog)) if catalog == "temp" => {
                self.copy_status =
                    Some("temp holds this session's temporary objects; drop them in SQL".into());
            }
            DeleteTarget::Database(Some(catalog)) => {
                let database = self.database.clone();
                self.run_tree_delete(move || database.detach(&catalog), cx);
            }
            DeleteTarget::RecentFile(path) => {
                let result = ApplicationState::load(self.repository.clone())
                    .and_then(|mut state| state.remove_recent_source(&path.to_string_lossy()));
                self.error = result
                    .err()
                    .map(|error| format!("Cannot remove the recent file: {error}"));
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
                self.confirm_tree_delete(
                    format!("Stop watching {}?", root.display()),
                    format!(
                        "{name} is in a watched folder, and DataDiver watches whole folders. \
                         Removing the folder drops the cached tables of all its files. To \
                         leave out only some files, move them out of the folder."
                    ),
                    "Remove folder".into(),
                    move |view, cx| view.remove_watched_folder(root.clone(), cx),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn remove_watched_folder(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let removed = ApplicationState::load(self.repository.clone())
            .and_then(|mut state| state.remove_watched_folder(&root.to_string_lossy()));
        self.forget_cached(root, removed, cx);
    }

    /// Drops the cached tables under `path` once it is no longer watched,
    /// keeping files still watched some other way.
    fn forget_cached(
        &mut self,
        path: PathBuf,
        removed: std::io::Result<()>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = removed {
            self.error = Some(format!("Cannot update watched sources: {error}"));
            cx.notify();
            return;
        }
        let database = self.database.clone();
        let repository = self.repository.clone();
        self.run_tree_delete(
            move || {
                let state =
                    ApplicationState::load(repository).map_err(|error| error.to_string())?;
                database.forget_sources(&path, |source| state.is_watched(source))
            },
            cx,
        );
    }

    /// Runs a database change off the UI thread, then shows its error, if
    /// any, and refreshes the tree.
    fn run_tree_delete(
        &mut self,
        change: impl FnOnce() -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { change() }).await;
            let _ = this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.error = Some(error);
                }
                view.refresh_database_tree(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_tree_delete(
        &mut self,
        title: String,
        message: String,
        action: String,
        confirmed: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity();
        let confirmed = std::rc::Rc::new(confirmed);
        window.open_dialog(cx, move |dialog, _, _| {
            let entity = entity.clone();
            let confirmed = confirmed.clone();
            let message = message.clone();
            dialog
                .title(title.clone())
                .content(move |content, _, _| content.child(message.clone()))
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                        .child(
                            DialogAction::new().child(
                                Button::new("tree-delete-confirm")
                                    .danger()
                                    .label(action.clone()),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    entity.update(cx, |view, cx| confirmed(view, cx));
                    true
                })
        });
    }

    pub(super) fn save_database_tree_width(&mut self, width: f32) {
        let width = width.round();
        if width == self.database_tree_width {
            return;
        }
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
    fn open_tree_target(
        &mut self,
        target: TreeTarget,
        new_window: bool,
        pivot_draft: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = match target {
            TreeTarget::View(definition) => FileEntry::View(*definition),
            TreeTarget::Source(path) => self.resolve_source(FileEntry::Source(path)),
            TreeTarget::Relation(relation) => {
                FileEntry::View(DocumentDefinition::draft_database_view(relation))
            }
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
        self.navigate_to_file(entry, false, pivot_draft, false, window.window_handle(), cx);
        window.focus(&self.grid.read(cx).focus_handle(cx), cx);
    }

    /// The tree to the left of everything below the toolbar.
    pub(super) fn with_database_tree(&self, content: AnyElement) -> AnyElement {
        if !self.database_tree_open {
            return content;
        }
        gpui_kit::component::resizable::h_resizable("database-tree-split")
            .with_state(&self.database_tree_split)
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size(px(self.database_tree_width))
                    .size_range(px(160.)..px(900.))
                    .child(self.database_tree.clone()),
            )
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size_range(px(240.)..px(100000.))
                    .child(content),
            )
            .into_any_element()
    }
}
