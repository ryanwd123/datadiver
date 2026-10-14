use super::*;
use crate::infrastructure::source_database::{database_file_kind, source_reader};

/// Why `path` cannot be cached as a watched file, if it cannot.
fn cache_rejection(path: &std::path::Path) -> Option<String> {
    if path.is_dir() {
        return Some(format!(
            "{} is a folder; use Add folder to watch a folder",
            path.display()
        ));
    }
    if database_file_kind(path).is_some() {
        return Some(format!(
            "{} is a database; attach it from the startup SQL script to use it every time",
            path.display()
        ));
    }
    source_reader(path).err()
}

impl DataFrameView {
    pub(super) fn set_cache_drop_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.cache_drop_open = open;
        cx.notify();
    }

    pub(super) fn add_watched_files_with_picker(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add watched files".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = chosen.await;
            let _ = this.update(cx, |view, cx| match result {
                Ok(Ok(Some(paths))) => view.cache_files(paths, cx),
                Ok(Ok(None)) => {}
                result => {
                    view.error = Some(format!("File picker failed: {result:?}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Adds files to `watched_files` and imports them into the cache, so
    /// they open in table mode. A file that fails to import is not kept.
    pub(super) fn cache_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.cache_drop_open = false;
        let (files, rejected): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .map(|path| match cache_rejection(&path) {
                Some(reason) => Err(reason),
                None => Ok(path),
            })
            .partition(Result::is_ok);
        let files = files.into_iter().flatten().collect::<Vec<_>>();
        let mut errors = rejected
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if files.is_empty() {
            self.error = errors.first().cloned();
            cx.notify();
            return;
        }
        let added = ApplicationState::load(self.repository.clone())
            .and_then(|mut state| state.add_watched_files(&files));
        if let Err(error) = added {
            self.error = Some(format!("Add watched files failed: {error}"));
            cx.notify();
            return;
        }
        self.copy_status = Some(format!("Caching {} file(s)…", files.len()));
        cx.notify();
        let database = self.database.clone();
        let repository = self.repository.clone();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    files
                        .into_iter()
                        .map(|path| {
                            let result = database.load(&path, LoadMode::Check).map(drop);
                            if result.is_err() {
                                let saved = path.canonicalize().unwrap_or_else(|_| path.clone());
                                let _ = ApplicationState::load(repository.clone()).and_then(
                                    |mut state| state.remove_watched_file(&saved.to_string_lossy()),
                                );
                            }
                            (path, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let mut cached = Vec::new();
                for (path, result) in results {
                    match result {
                        Ok(()) => cached.push(source_file_name(&path)),
                        Err(error) => errors.push(error),
                    }
                }
                view.copy_status = (!cached.is_empty())
                    .then(|| format!("Cached {}", cached.join(", ")));
                view.error = (!errors.is_empty()).then(|| errors.join("\n"));
                view.refresh_database_tree(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_cache_drop(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let browse = cx.listener(|view, _: &ClickEvent, _, cx| {
            view.add_watched_files_with_picker(cx);
            view.set_cache_drop_open(false, cx);
        });
        let cancel = cx.listener(|view, _: &ClickEvent, _, cx| {
            view.set_cache_drop_open(false, cx);
        });
        let dropped = cx.listener(|view, paths: &ExternalPaths, _, cx| {
            view.cache_files(paths.paths().to_vec(), cx);
            cx.stop_propagation();
        });
        div()
            .id("cache-drop")
            .absolute()
            .inset_0()
            .on_drop(dropped)
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.tokens.background.opacity(0.85))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(520.))
                    .p_6()
                    .rounded_md()
                    .border_2()
                    .border_dashed()
                    .border_color(theme.tokens.primary)
                    .bg(theme.tokens.popover)
                    .shadow_lg()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .child("Drop files to add to cache"),
                    )
                    .child(div().text_color(theme.tokens.muted_foreground).child(
                        "Cached files are imported into the files schema, open in table \
                         mode, and are listed under Sources › Watched files. Files opened \
                         any other way are read in place in query mode.",
                    ))
                    .child(div().text_color(theme.tokens.muted_foreground).child(
                        "CSV, TSV, CSV.GZ, Parquet and XLSX files. To watch a whole folder, \
                         use Add folder.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("cache-drop-browse")
                                    .label("Browse…")
                                    .on_click(browse),
                            )
                            .child(
                                Button::new("cache-drop-cancel")
                                    .label("Cancel (Esc)")
                                    .on_click(cancel),
                            ),
                    ),
            )
    }
}
