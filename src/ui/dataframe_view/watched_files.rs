use super::*;
use crate::application::state::ApplicationState;
use crate::infrastructure::source_database::{LoadMode, database_file_kind, source_reader};
use crate::ui::hit::Button;
use crate::ui::platform::{Align, Canvas, rect, system, text};
use crate::ui::widgets::{ButtonStyle, Row, button, button_height, button_width, colors};

/// Why `path` cannot be cached as a watched file, if it cannot.
fn cache_rejection(path: &std::path::Path) -> Option<String> {
    if path.is_dir() {
        return Some(format!("{} is a folder; use Add folder to watch a folder", path.display()));
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
    pub(crate) fn set_cache_drop_open(&mut self, open: bool, cx: &mut Cx<Self>) {
        self.cache_drop_open = open;
        cx.notify();
    }

    pub(crate) fn add_watched_files_with_picker(&mut self, cx: &mut Cx<Self>) {
        let chosen = system::prompt_for_paths(
            cx.hwnd(),
            system::PathPrompt {
                directories: false,
                multiple: true,
                title: "Add watched files",
            },
        );
        match chosen {
            Ok(Some(paths)) => self.cache_files(paths, cx),
            Ok(None) => {}
            Err(error) => {
                self.error = Some(format!("File picker failed: {error}"));
                cx.notify();
            }
        }
    }

    /// Adds files to `watched_files` and imports them into the cache, so
    /// they open in table mode. A file that fails to import is not kept.
    pub(crate) fn cache_files(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Self>) {
        self.cache_drop_open = false;
        let (files, rejected): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .map(|path| match cache_rejection(&path) {
                Some(reason) => Err(reason),
                None => Ok(path),
            })
            .partition(Result::is_ok);
        let files = files.into_iter().flatten().collect::<Vec<_>>();
        let mut errors = rejected.into_iter().filter_map(Result::err).collect::<Vec<_>>();
        if files.is_empty() {
            self.error = errors.first().cloned();
            cx.notify();
            return;
        }
        let added = ApplicationState::load(self.repository.clone()).and_then(|mut state| state.add_watched_files(&files));
        if let Err(error) = added {
            self.error = Some(format!("Add watched files failed: {error}"));
            cx.notify();
            return;
        }
        self.copy_status = Some(format!("Caching {} file(s)…", files.len()));
        cx.notify();
        let database = self.database.clone();
        let repository = self.repository.clone();
        cx.spawn(
            move || {
                files
                    .into_iter()
                    .map(|path| {
                        let result = database.load(&path, LoadMode::Check).map(drop);
                        if result.is_err() {
                            let saved = path.canonicalize().unwrap_or_else(|_| path.clone());
                            let _ = ApplicationState::load(repository.clone())
                                .and_then(|mut state| state.remove_watched_file(&saved.to_string_lossy()));
                        }
                        (path, result)
                    })
                    .collect::<Vec<_>>()
            },
            move |view: &mut Self, cx, results| {
                let mut cached = Vec::new();
                for (path, result) in results {
                    match result {
                        Ok(()) => cached.push(source_file_name(&path)),
                        Err(error) => errors.push(error),
                    }
                }
                view.copy_status = (!cached.is_empty()).then(|| format!("Cached {}", cached.join(", ")));
                view.error = (!errors.is_empty()).then(|| errors.join("\n"));
                view.refresh_database_tree(cx);
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn paint_cache_drop(&self, canvas: &mut Canvas<Hit>) {
        let fonts = &self.fonts;
        canvas.fill(canvas.bounds(), colors::BACKGROUND.alpha(0.85));
        canvas.hit(canvas.bounds(), Hit::Surface);
        let width = 520.0_f32.min(canvas.width - 32.0);
        let padding = 24.0;
        let inner_width = width - 2.0 * padding;
        let first = "Cached files are imported into the files schema, open in table mode, and are listed under Sources › Watched files. Files opened any other way are read in place in query mode.";
        let second = "CSV, TSV, CSV.GZ, Parquet and XLSX files. To watch a whole folder, use Add folder.";
        let wrapped = |text_value: &str| {
            text::layout(text_value, &fonts.ui, crate::ui::platform::TextOptions::wrapped(inner_width)).height
        };
        let title_height = text::line_height(&fonts.large);
        let height = padding * 2.0 + title_height + 12.0 + wrapped(first) + 12.0 + wrapped(second) + 12.0 + button_height(&fonts.ui);
        let r = rect((canvas.width - width) / 2.0, (canvas.height - height) / 2.0, width, height);
        canvas.shadow(r, 8.0);
        canvas.fill_rounded(r, 8.0, colors::POPOVER);
        canvas.stroke_rounded(r, 8.0, colors::FOCUS, 2.0);
        let mut y = r.y + padding;
        canvas.text(rect(r.x + padding, y, inner_width, title_height), "Drop files to add to cache", &fonts.large, colors::FOREGROUND, Align::Left);
        y += title_height + 12.0;
        y += canvas.paragraph(rect(r.x + padding, y, inner_width, 200.0), first, &fonts.ui, colors::MUTED) + 12.0;
        y += canvas.paragraph(rect(r.x + padding, y, inner_width, 200.0), second, &fonts.ui, colors::MUTED) + 12.0;
        let mut buttons = Row::new(rect(r.x + padding, y, inner_width, button_height(&fonts.ui)), 8.0);
        let cancel = buttons.right(button_width("Cancel (Esc)", &fonts.ui));
        button(canvas, cancel, "Cancel (Esc)", &fonts.ui, ButtonStyle::Outline, true, Hit::Button(Button::CacheDropCancel));
        let browse = buttons.right(button_width("Browse…", &fonts.ui));
        button(canvas, browse, "Browse…", &fonts.ui, ButtonStyle::Outline, true, Hit::Button(Button::CacheDropBrowse));
    }
}
