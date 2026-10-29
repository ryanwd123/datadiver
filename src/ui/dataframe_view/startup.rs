//! Opening windows, restoring their placement, and scripted startups for
//! screenshots and benchmarks.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::*;
use crate::infrastructure::source_database::LoadMode;
use crate::ui::font_picker::FontTarget;
use crate::ui::platform::{Runtime, WindowOptions};
use crate::ui::window_placement;

pub struct ViewerOptions {
    pub benchmark_ui: bool,
    pub width: f32,
    pub height: f32,
    pub restore_window: bool,
    pub capture: Option<PathBuf>,
    pub capture_delay: Duration,
    pub exit_after: Option<Duration>,
    pub search_text: String,
    pub slicers: Vec<String>,
    pub load_path: Option<PathBuf>,
    pub load_view: Option<DocumentDefinition>,
    pub repository: ViewRepository,
    pub show: Vec<String>,
    /// A location for a new window to open, such as a drill-down.
    pub(crate) open_entry: Option<(FileEntry, Option<DrillDown>)>,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        Self {
            benchmark_ui: false,
            width: 1400.0,
            height: 900.0,
            restore_window: true,
            capture: None,
            capture_delay: Duration::from_millis(300),
            exit_after: None,
            search_text: String::new(),
            slicers: Vec::new(),
            load_path: None,
            load_view: None,
            repository: ViewRepository::new(StorePaths::operating_system()),
            show: Vec::new(),
            open_entry: None,
        }
    }
}

/// One step of a scripted startup.
pub(crate) enum Step {
    Run(Box<dyn FnOnce(&mut DataFrameView, &mut Cx<DataFrameView>) -> Result<(), String>>),
    /// Waits for the condition, failing with the message after the timeout.
    Wait(Box<dyn Fn(&DataFrameView) -> Result<bool, String>>, Duration, &'static str),
    /// Waits for the next painted frame, recording `metric` from `start`.
    Frame(&'static str, Rc<Cell<Instant>>),
    Sleep(Duration),
}

type ScriptDone = Rc<dyn Fn(&mut DataFrameView, &mut Cx<DataFrameView>, Result<(), String>)>;

impl DataFrameView {
    /// Runs `steps` in order, polling waits, then `done` with the outcome.
    /// `waiting_since` is when the first step began waiting, if it did.
    pub(crate) fn run_script(&mut self, mut steps: VecDeque<Step>, waiting_since: Option<Instant>, done: ScriptDone, cx: &mut Cx<Self>) {
        let mut waiting_since = waiting_since;
        while let Some(step) = steps.pop_front() {
            match step {
                Step::Run(run) => {
                    if let Err(error) = run(self, cx) {
                        done(self, cx, Err(error));
                        return;
                    }
                    self.after_input(cx);
                }
                Step::Wait(condition, timeout, message) => match condition(self) {
                    Ok(true) => waiting_since = None,
                    Ok(false) => {
                        let since = *waiting_since.get_or_insert_with(Instant::now);
                        if since.elapsed() > timeout {
                            done(self, cx, Err(message.into()));
                            return;
                        }
                        steps.push_front(Step::Wait(condition, timeout, message));
                        cx.timer(Duration::from_millis(5), move |view, cx| view.run_script(steps, Some(since), done, cx))
                            .detach();
                        return;
                    }
                    Err(error) => {
                        done(self, cx, Err(error));
                        return;
                    }
                },
                Step::Frame(metric, start) => {
                    let frame = self.frames.get();
                    let frames = self.frames.clone();
                    steps.push_front(Step::Run(Box::new(move |_, _| {
                        crate::infrastructure::performance::record(metric, start.get());
                        Ok(())
                    })));
                    steps.push_front(Step::Wait(
                        Box::new(move |_| Ok(frames.get() > frame)),
                        Duration::from_secs(60),
                        "timed out waiting for a visible window frame",
                    ));
                    cx.notify();
                }
                Step::Sleep(delay) => {
                    cx.timer(delay, move |view, cx| view.run_script(steps, None, done, cx))
                        .detach();
                    return;
                }
            }
        }
        done(self, cx, Ok(()));
    }

    pub(crate) fn save_geometry(&self) {
        if let Some(geometry) = &self.window_geometry
            && let Err(error) = self.repository.save_window_geometry(geometry)
        {
            eprintln!("cannot save window placement: {error}");
        }
    }

    pub(crate) fn update_geometry(&mut self, cx: &Cx<Self>) {
        if self.window_geometry.is_some()
            && let Some(placement) = cx.placement()
        {
            self.window_geometry = Some(window_placement::geometry(placement));
        }
    }

    /// Saves this window's placement now and whenever it moves.
    pub(crate) fn remember_window(&mut self, cx: &mut Cx<Self>) {
        if let Some(placement) = cx.placement() {
            self.window_geometry = Some(window_placement::geometry(placement));
            self.save_geometry();
        }
    }

    pub(crate) fn window_moved(&mut self, cx: &mut Cx<Self>) {
        cx.notify();
        if self.window_geometry.is_none() {
            return;
        }
        let Some(placement) = cx.placement() else {
            return;
        };
        let geometry = window_placement::geometry(placement);
        if self.window_geometry.as_ref() == Some(&geometry) {
            return;
        }
        self.window_geometry = Some(geometry);
        self.geometry_save = Some(cx.timer(Duration::from_millis(250), |view, _| view.save_geometry()));
    }

    /// Opens the app's first window.
    pub fn open(runtime: &Runtime<Self>, frame: FrameSnapshot, source_path: Option<String>, options: ViewerOptions) {
        let (window, build) = Self::window(true, frame, source_path, options);
        runtime.open_window(window, build);
    }

    /// Opens another window from a window's work.
    pub(crate) fn open_in(cx: &mut Cx<Self>, frame: FrameSnapshot, source_path: Option<String>, options: ViewerOptions) {
        let primary = cx.window_count() == 0;
        let (window, build) = Self::window(primary, frame, source_path, options);
        cx.open_window(window, build);
    }

    #[allow(clippy::type_complexity)]
    fn window(primary: bool, frame: FrameSnapshot, source_path: Option<String>, options: ViewerOptions) -> (WindowOptions, impl FnOnce(&mut Cx<Self>) -> Self) {
        let mut window = WindowOptions {
            title: "DataDiver".into(),
            width: options.width,
            height: options.height,
            placement: None,
            visible: true,
        };
        if primary && options.restore_window && options.capture.is_none() {
            match options.repository.load_history() {
                Ok(history) => {
                    let displays = crate::ui::platform::window::displays();
                    window.placement = history
                        .primary_window
                        .as_ref()
                        .and_then(|saved| window_placement::restored_bounds(saved, &displays));
                }
                Err(error) => eprintln!("cannot restore window placement: {error}"),
            }
        }
        let build = move |cx: &mut Cx<Self>| {
            let mut view = Self::new(
                frame,
                source_path,
                options.search_text.clone(),
                options.load_view.clone(),
                options.repository.clone(),
                cx,
            );
            view.start(primary, options, cx);
            view
        };
        (window, build)
    }

    /// What a new window does once it exists.
    fn start(&mut self, primary: bool, options: ViewerOptions, cx: &mut Cx<Self>) {
        let remember = primary && options.capture.is_none();
        if remember {
            cx.defer(|view, cx| view.remember_window(cx));
        }
        for column in &options.slicers {
            self.add_slicer(column.clone(), cx);
        }
        if let Some(database_view) = options.load_view.as_ref().and_then(|d| d.database_view.clone()) {
            self.load_database_view(database_view, false, cx);
        } else if let Some((entry, drill)) = options.open_entry.clone() {
            self.navigate_to_location(entry, false, false, true, None, drill, cx);
        } else if let Some(path) = options.load_path.clone() {
            // A saved view's source always comes from the cache.
            let entry = if options.load_view.is_some() {
                FileEntry::Source(path)
            } else {
                FileEntry::Source(path)
            };
            match entry {
                FileEntry::Source(path) => self.load_source(path, false, LoadMode::Check, cx),
                entry => self.navigate_to_location(entry, false, false, true, None, None, cx),
            }
        } else if !options.search_text.is_empty() {
            self.schedule_search(options.search_text.clone(), cx);
        }
        if options.benchmark_ui {
            self.benchmark = true;
            let steps = super::performance::steps();
            self.run_script(
                steps,
                None,
                Rc::new(|_, cx, result| {
                    if let Err(error) = result {
                        eprintln!("UI benchmark failed: {error}");
                    }
                    cx.quit();
                }),
                cx,
            );
            return;
        }
        if options.show.is_empty() && options.capture.is_none() {
            if let Some(delay) = options.exit_after {
                cx.timer(delay, |_, cx| cx.quit()).detach();
            }
            return;
        }
        let steps = Self::capture_steps(options);
        self.run_script(
            steps,
            None,
            Rc::new(|_, cx, result| {
                if let Err(error) = result {
                    eprintln!("{error}");
                    cx.quit();
                }
            }),
            cx,
        );
    }

    fn ready(&self) -> bool {
        !self.loading && !self.busy && !self.sql_running && self.grid.widths_ready()
    }

    fn capture_steps(options: ViewerOptions) -> VecDeque<Step> {
        let mut steps = VecDeque::new();
        steps.push_back(Step::Wait(
            Box::new(|view: &DataFrameView| Ok(view.ready())),
            Duration::from_secs(20),
            "capture timed out waiting for the requested data",
        ));
        for target in options.show.clone() {
            steps.push_back(Step::Run(Box::new(move |view, cx| {
                view.show_target(&target, cx);
                Ok(())
            })));
        }
        if options.show.iter().any(|target| {
            target == "info" || target == "palette:files" || target == "palette:slicers" || target == "editor:aggregation-error"
        }) {
            steps.push_back(Step::Wait(
                Box::new(|view: &DataFrameView| Ok(!(view.information_busy || view.file_loading || view.busy))),
                Duration::from_secs(20),
                "capture timed out",
            ));
        }
        if let Some(path) = options.capture {
            steps.push_back(Step::Wait(
                Box::new(|view: &DataFrameView| {
                    Ok(view.ready() && view.sql_panel.as_ref().is_none_or(|panel| !panel.busy))
                }),
                Duration::from_secs(20),
                "capture timed out waiting for the SQL editor",
            ));
            steps.push_back(Step::Sleep(options.capture_delay));
            let exit = options.exit_after.is_none();
            steps.push_back(Step::Run(Box::new(move |_, cx| {
                let id = cx.window_id();
                // Painting borrows the view, so it waits until this step returns.
                cx.with_runtime(move |runtime| {
                    if let Err(error) = crate::ui::platform::headless::capture(runtime, id, &path) {
                        eprintln!("capture failed: {error}");
                    }
                    if exit {
                        runtime.quit();
                    }
                });
                Ok(())
            })));
        }
        if let Some(delay) = options.exit_after {
            steps.push_back(Step::Sleep(delay));
            steps.push_back(Step::Run(Box::new(|_, cx| {
                cx.quit();
                Ok(())
            })));
        }
        steps
    }

    /// Opens a part of the UI by name, for `--show`.
    pub(crate) fn show_target(&mut self, target: &str, cx: &mut Cx<Self>) {
        match target {
            "editor:startup-sql" => self.navigate_to_file(FileEntry::StartupSql, false, false, false, cx),
            "editor:sql" | "editor:sql-results" | "editor:sql-completion" | "editor:sql-error" | "editor:sql-ask"
            | "editor:sql-ask-log" | "editor:sql-ask-mention" => self.capture_sql_editor(target, cx),
            "info" => self.open_information(cx),
            "help" => self.open_help(cx),
            "keymap" => self.open_keymap_editor(cx),
            "actions" | "action-menu" => self.capture_actions(target, cx),
            "ai-providers" => self.open_ai_settings(cx),
            "database-tree" => self.toggle_database_tree(cx),
            "search-columns" => {
                let id = self.search_input.id;
                self.set_focus(Some(id), cx);
                self.set_search_text("\"".into(), cx);
                self.sync_completion_columns();
                self.search_completion.refresh(&self.search_input);
            }
            "slicer-search" => {
                let mut focus = self.focus;
                self.slicer_sidebar.open_first_search("a", &mut focus);
                self.focus = focus;
            }
            "theme" | "theme-editor" => self.open_theme_editor(cx),
            "space" => self.dispatch_command(CommandId::Space, cx),
            "cache-drop" => self.set_cache_drop_open(true, cx),
            "cell-detail" => self.toggle_cell_detail(cx),
            "save" => self.open_save_dialog(cx),
            "palette:copy-as" => self.open_copy_as(cx),
            "palette:commands" => self.open_command_palette(cx),
            "palette:fonts" => self.open_font_picker(FontTarget::Grid, cx),
            "palette:editor-fonts" => self.open_font_picker(FontTarget::Editor, cx),
            "palette:slicers" => self.open_slicer_picker(cx),
            "palette:files" => self.open_file_palette(cx),
            "palette:folders" => self.open_folder_palette(cx),
            "palette:recovered" => self.open_recovery_palette(cx),
            "palette:favorites" => self.open_favorites_palette(cx),
            "palette:columns" => self.open_column_jump(cx),
            "palette:column-chooser" => self.open_column_chooser(cx),
            "editor:group" | "editor:group-error" | "editor:pivot" | "editor:pivot-error" => {
                self.capture_dimension_editor(target, cx)
            }
            "panel:group" => self.open_pivot_panel(PivotFieldKind::GroupBy, cx),
            "panel:aggregation" | "aggregation-menu" => self.open_aggregation_panel(cx),
            "panel:aggregation-functions" => {
                self.open_aggregation_panel(cx);
                let mut focus = self.focus;
                if let Some(Dialog::Aggregation(panel)) = &mut self.dialog {
                    panel.show_functions(true, &mut focus);
                }
                self.focus = focus;
            }
            "editor:aggregation" | "form:aggregation" | "form:condition" => {
                self.open_aggregation_panel(cx);
                let mut focus = self.focus;
                if let Some(Dialog::Aggregation(panel)) = &mut self.dialog {
                    panel.start_custom(&mut focus);
                }
                self.focus = focus;
            }
            "editor:aggregation-completion" | "editor:group-completion" => {
                let mut focus = self.focus;
                if target == "editor:group-completion" {
                    self.open_pivot_panel(PivotFieldKind::GroupBy, cx);
                    if let Some(Dialog::Pivot(panel)) = &mut self.dialog {
                        panel.start_custom(&mut focus);
                        panel.sql.set_text("sum(");
                        panel.sql.set_selection(4..4);
                        panel.sql.type_text("\"");
                        panel.focus_sql(&mut focus);
                    }
                } else {
                    self.open_aggregation_panel(cx);
                    if let Some(Dialog::Aggregation(panel)) = &mut self.dialog {
                        panel.start_custom(&mut focus);
                        panel.sql.set_text("sum(");
                        panel.sql.set_selection(4..4);
                        // Typed like a keystroke, so completion opens.
                        panel.sql.type_text("\"");
                        panel.focus_sql(&mut focus);
                    }
                }
                self.focus = focus;
            }
            "editor:aggregation-error" => {
                self.open_aggregation_panel(cx);
                let mut focus = self.focus;
                if let Some(Dialog::Aggregation(panel)) = &mut self.dialog {
                    panel.start_custom(&mut focus);
                    panel.sql.set_text("sum(\"missing column\")");
                    panel.apply();
                }
                self.focus = focus;
                self.handle_panel_events(cx);
            }
            "panel:pivot" => self.open_pivot_panel(PivotFieldKind::PivotBy, cx),
            "panel:order-by" => self.open_order_by_panel(cx),
            _ => {}
        }
        cx.notify();
    }
}
