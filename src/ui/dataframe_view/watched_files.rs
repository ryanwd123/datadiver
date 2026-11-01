use super::*;
use crate::application::state::ApplicationState;
use crate::infrastructure::source_database::{database_file_kind, source_reader};
use crate::ui::hit::Button;
use crate::ui::platform::{Align, Canvas, rect, system, text};
use crate::ui::widgets::{ButtonStyle, Row, button, button_height, button_width, colors};

/// The watched files and the files in watched folders.
fn watched_sources(settings: &crate::infrastructure::json_store::Settings) -> Vec<PathBuf> {
    let folders = settings.watched_folders.iter().map(PathBuf::from).collect::<Vec<_>>();
    let mut files = crate::infrastructure::catalog::scan_sources(&folders);
    files.extend(settings.watched_files.iter().map(PathBuf::from));
    files
}

/// Why `path` cannot be a watched file, if it cannot.
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

    /// Adds files to `watched_files`, which lists them under Sources for
    /// quick opening. Nothing is read until a file is opened.
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
        let errors = rejected.into_iter().filter_map(Result::err).collect::<Vec<_>>();
        self.error = (!errors.is_empty()).then(|| errors.join("
"));
        if files.is_empty() {
            cx.notify();
            return;
        }
        match ApplicationState::load(self.repository.clone()).and_then(|mut state| state.add_watched_files(&files)) {
            Ok(()) => {
                let names = files.iter().map(|path| source_file_name(path)).collect::<Vec<_>>();
                self.copy_status = Some(format!("Watching {}", names.join(", ")));
                self.refresh_database_tree(cx);
                self.sync_file_views(false, cx);
            }
            Err(error) => self.error = Some(format!("Add watched files failed: {error}")),
        }
        cx.notify();
    }

    /// Brings the profile database's `files` views in step with the watched
    /// files and folders, or drops them when the setting is off. Runs on a
    /// worker, since it lists the folders and a new view reads its file's
    /// columns. `report` says how it went; otherwise it stays quiet, since
    /// it also runs when a window opens.
    pub(crate) fn sync_file_views(&mut self, report: bool, cx: &mut Cx<Self>) {
        let repository = self.repository.clone();
        let database = self.database.clone();
        cx.spawn(
            move || {
                let settings = repository.load_settings().map_err(|error| error.to_string())?;
                let files = if settings.file_views { watched_sources(&settings) } else { Vec::new() };
                database.sync_file_views(&files).map(|failed| (settings.file_views, failed))
            },
            move |view: &mut Self, cx, result| {
                if !report {
                    return;
                }
                match result {
                    Ok((on, failed)) if failed.is_empty() => {
                        view.copy_status = Some(
                            if on { "Watched files have views in the files schema" } else { "Dropped the views of the watched files" }.into(),
                        );
                    }
                    Ok((_, failed)) => view.error = Some(format!("Could not make views of some watched files:\n{}", failed.join("\n"))),
                    Err(error) => view.error = Some(format!("Could not update the views of the watched files: {error}")),
                }
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
        let first = "Watched files are listed under Sources › Watched files for quick opening. Each opens with its own query engine, Polars unless you choose DuckDB.";
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
        canvas.text(rect(r.x + padding, y, inner_width, title_height), "Drop files to watch", &fonts.large, colors::FOREGROUND, Align::Left);
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
