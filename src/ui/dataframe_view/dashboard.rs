//! A dashboard in a window: its page in a WebView where the grid would be,
//! the file tree beside it, and DataDiver's palettes over it.
//!
//! The page talks to DataDiver through the script in `dashboard_bridge.js`.
//! Its queries run on the worker pool through the dashboard's
//! [`DashboardEngine`]; their results go back as `reply` messages.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde_json::{Value, json};

use super::*;
use super::dialogs::PaletteKind;
use crate::infrastructure::dashboard::{CommandSpec, DashboardEngine, Manifest, QueryRequest, QueryResult, SourceStatus, read_dashboard};
use crate::ui::hit::Splitter;
use crate::ui::platform::webview::{Resource, Snapshot, WebView, WebViewCallbacks, WebViewOptions, WebViewState};
use crate::ui::platform::{Align, Canvas, Cursor, KeyEvent, Rect, rect, text};
use crate::ui::widgets::{button_height, colors};

/// The page's address. `.example` names are never real hosts.
pub(crate) const ORIGIN: &str = "https://dashboard.datadiver.example";
/// Hosts a page may load scripts, styles and fonts from.
const ALLOWED_HOSTS: &[&str] = &[
    "cdnjs.cloudflare.com",
    "cdn.jsdelivr.net",
    "unpkg.com",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
];
const QUERY_LOG: usize = 200;
const CONSOLE_LOG: usize = 100;
const BRIDGE: &str = include_str!("dashboard_bridge.js");

/// One query the page ran.
#[derive(Clone, Debug)]
pub(crate) struct QueryRecord {
    pub sql: String,
    pub params: Option<Value>,
    pub elapsed_ms: f64,
    pub rows: usize,
    pub truncated: bool,
    pub error: Option<String>,
}

/// A message the page logged with `console.error` or `console.warn`, or an uncaught error.
#[derive(Clone, Debug)]
pub(crate) struct ConsoleRecord {
    pub level: String,
    pub message: String,
}

/// What the page is served: the dashboard's HTML, swapped on reload.
#[derive(Default)]
struct Document {
    html: String,
    folder: PathBuf,
}

pub(crate) struct DashboardState {
    pub path: PathBuf,
    pub manifest: Manifest,
    /// Set once the sources are checked.
    pub engine: Option<Arc<DashboardEngine>>,
    /// Why the dashboard cannot show, such as a manifest error.
    pub error: Option<String>,
    web: Option<WebView>,
    document: Rc<RefCell<Document>>,
    /// The page's `<title>`, used when the manifest has no title.
    page_title: Option<String>,
    /// Drawn in place of the page while something covers it.
    snapshot: Option<Snapshot>,
    covered: bool,
    /// The page had the keyboard when it was covered, or should get it.
    wants_focus: bool,
    /// Queries that came before the sources were ready.
    waiting: Vec<(u64, QueryRequest)>,
    pub running: usize,
    pub queries: Vec<QueryRecord>,
    pub console: Vec<ConsoleRecord>,
    pub loaded: bool,
    /// Bumped each time a page loads, so answers for the old page are dropped.
    generation: u64,
    /// Bumped each time the manifest is read again, so sources opened for an
    /// older manifest are dropped. Separate from `generation`: the page
    /// often loads before its sources are open, and they must still arrive.
    sources_generation: u64,
    /// The file's modified time when read, to reload after it changes.
    modified: Option<SystemTime>,
    pub opened_at: Instant,
    /// When the page last had no query running, for `dashboard check`.
    pub quiet_since: Option<Instant>,
}

impl DashboardState {
    pub fn title(&self) -> String {
        self.manifest
            .title
            .clone()
            .or_else(|| self.page_title.clone().filter(|title| !title.trim().is_empty()))
            .unwrap_or_else(|| {
                self.path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Dashboard".into())
            })
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.manifest.commands
    }

    /// The page has loaded and has no query running or waiting.
    pub fn settled(&self) -> bool {
        (self.loaded && self.engine.is_some() && self.running == 0 && self.waiting.is_empty()) || self.error.is_some()
    }

    /// Gives the page the keyboard once nothing covers it.
    pub fn request_focus(&mut self) {
        self.wants_focus = true;
    }

    fn page_url(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "dashboard.ddash".into());
        format!("{ORIGIN}/{}", percent_encode(&name))
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""), 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The content security policy of the page: its own files, the allowed
/// hosts for scripts, styles and fonts, and no other network.
fn content_security_policy() -> String {
    let cdn = "https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com";
    format!(
        "default-src 'self' data: blob:; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: {cdn}; \
         style-src 'self' 'unsafe-inline' {cdn} https://fonts.googleapis.com; \
         font-src 'self' data: {cdn} https://fonts.gstatic.com; \
         img-src 'self' data: blob:; \
         connect-src 'self'; frame-src 'none'; form-action 'none'; base-uri 'self'"
    )
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" | "ddash" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" | "md" | "csv" | "tsv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// What the page gets for a request to `url`: the dashboard itself, a file
/// beside it, a file from an allowed host, or nothing.
fn resource_for(document: &Document, url: &str) -> Resource {
    let Some(rest) = url.strip_prefix(ORIGIN) else {
        let host = url
            .strip_prefix("https://")
            .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""))
            .unwrap_or("");
        if ALLOWED_HOSTS.contains(&host) {
            return Resource::Pass;
        }
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ws") {
            return Resource::Blocked;
        }
        return Resource::Pass;
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let path = percent_decode(path.trim_start_matches('/'));
    let headers = vec![("Content-Security-Policy".to_owned(), content_security_policy()), ("Cache-Control".to_owned(), "no-store".to_owned())];
    let page = Path::new(&path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(crate::infrastructure::dashboard::EXTENSION));
    if path.is_empty() || page {
        return Resource::Serve {
            content_type: "text/html; charset=utf-8".into(),
            body: document.html.as_bytes().to_vec(),
            headers,
        };
    }
    // Files beside the dashboard, such as a chart library saved for offline use.
    let relative = Path::new(&path);
    if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Resource::Missing;
    }
    let file = document.folder.join(relative);
    match std::fs::read(&file) {
        Ok(body) => Resource::Serve {
            content_type: content_type(&file).into(),
            body,
            headers,
        },
        Err(_) => Resource::Missing,
    }
}

/// How `datadiver dashboard check` runs a dashboard.
#[derive(Clone, Debug)]
pub struct DashboardCheck {
    /// Where to save a picture of the page once it settles.
    pub capture: Option<PathBuf>,
    /// The longest the check waits for the page to settle.
    pub timeout: std::time::Duration,
    /// How long no query may run before the page counts as settled.
    pub quiet: std::time::Duration,
}

/// The exit code `dashboard check` ends with: 0 when nothing failed.
pub static CHECK_EXIT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// A row of the command palette while a dashboard shows.
#[derive(Clone, Debug)]
pub(crate) enum DashboardPaletteEntry {
    App(CommandId),
    /// A command from the dashboard's manifest, by id.
    Page(String),
}

/// Commands that make sense while a dashboard shows; the rest act on the grid.
pub(crate) fn dashboard_allows(command: CommandId) -> bool {
    matches!(
        command,
        CommandId::Files
            | CommandId::Commands
            | CommandId::DatabaseTree
            | CommandId::Open
            | CommandId::NewWindow
            | CommandId::CloseWindow
            | CommandId::Reload
            | CommandId::ReloadReset
            | CommandId::Help
            | CommandId::About
            | CommandId::Settings
            | CommandId::Keymap
            | CommandId::Theme
            | CommandId::Fonts
            | CommandId::AddFolder
            | CommandId::AddWatchedFiles
            | CommandId::OpenSettingsDirectory
            | CommandId::StartupSql
            | CommandId::AiProviders
            | CommandId::DashboardDevTools
            | CommandId::DashboardQueries
            | CommandId::RegisterDashboards
            | CommandId::UnregisterDashboards
            | CommandId::AddToPath
            | CommandId::RemoveFromPath
    )
}

impl DataFrameView {
    /// Shows the dashboard at `path` in this window, in place of the grid.
    pub(crate) fn open_dashboard(&mut self, path: PathBuf, cx: &mut Cx<Self>) {
        let path = path.canonicalize().map(strip_verbatim).unwrap_or(path);
        let (html, manifest) = match read_dashboard(&path) {
            Ok(read) => read,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.close_dashboard();
        self.close_dialog(cx);
        let document = Rc::new(RefCell::new(Document {
            html,
            folder: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        }));
        self.dashboard = Some(Box::new(DashboardState {
            modified: modified_time(&path),
            path: path.clone(),
            manifest,
            engine: None,
            error: None,
            web: None,
            document,
            page_title: None,
            snapshot: None,
            covered: false,
            wants_focus: true,
            waiting: Vec::new(),
            running: 0,
            queries: Vec::new(),
            console: Vec::new(),
            loaded: false,
            generation: 0,
            sources_generation: 0,
            opened_at: Instant::now(),
            quiet_since: None,
        }));
        self.focus = None;
        self.error = None;
        self.start_dashboard_engine(cx);
        self.start_dashboard_web(cx);
        self.set_window_title(cx);
        // An agent's checks are not dashboards the user opened.
        if !self.dashboard_checking {
            self.remember_dashboard(&path);
        }
        cx.notify();
    }

    fn remember_dashboard(&self, path: &Path) {
        let path = path.to_string_lossy().into_owned();
        if let Err(error) = self.repository.update_history(|history| {
            history.dashboards.retain(|known| !known.eq_ignore_ascii_case(&path));
            history.dashboards.insert(0, path);
            history.dashboards.truncate(20);
        }) {
            eprintln!("cannot remember the dashboard: {error}");
        }
    }

    pub(crate) fn set_window_title(&self, cx: &mut Cx<Self>) {
        let title = match &self.dashboard {
            Some(dashboard) => format!("{} - DataDiver", dashboard.title()),
            None => "DataDiver".into(),
        };
        cx.set_title(&title);
    }

    fn start_dashboard_engine(&mut self, cx: &mut Cx<Self>) {
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        dashboard.engine = None;
        let database = self.database.clone();
        let repository = self.repository.clone();
        let manifest = dashboard.manifest.clone();
        let path = dashboard.path.clone();
        let generation = dashboard.sources_generation;
        cx.spawn(
            move || {
                crate::infrastructure::dashboard::prepare_profile(&database, &repository)?;
                DashboardEngine::open(database, &manifest, &path).map(Arc::new)
            },
            move |view: &mut Self, cx, engine: Result<Arc<DashboardEngine>, String>| {
                let Some(dashboard) = view.dashboard.as_mut().filter(|dashboard| dashboard.sources_generation == generation) else {
                    return;
                };
                match engine {
                    Ok(engine) => {
                        dashboard.engine = Some(engine.clone());
                        dashboard.error = None;
                        let errors = engine.source_errors();
                        if !errors.is_empty() {
                            view.error = Some(format!("Some dashboard sources cannot be read: {}", errors.join("; ")));
                        }
                        view.send_dashboard_config();
                        let waiting = std::mem::take(&mut view.dashboard.as_mut().expect("dashboard").waiting);
                        for (id, request) in waiting {
                            view.run_dashboard_query(id, request, cx);
                        }
                    }
                    Err(error) => {
                        dashboard.error = Some(format!("Cannot open the dashboard's sources: {error}"));
                        let waiting = std::mem::take(&mut dashboard.waiting);
                        let message = dashboard.error.clone().unwrap_or_default();
                        for (id, _) in waiting {
                            view.reply_dashboard(id, Err(message.clone()));
                        }
                    }
                }
                cx.notify();
            },
        )
        .detach();
    }

    fn start_dashboard_web(&mut self, cx: &mut Cx<Self>) {
        let Some(hwnd) = cx.hwnd() else {
            // A window still being built has no handle yet; a test's never does.
            cx.defer(|view, cx| {
                if cx.hwnd().is_some() && view.dashboard.as_ref().is_some_and(|dashboard| dashboard.web.is_none()) {
                    view.start_dashboard_web(cx);
                    view.set_window_title(cx);
                }
            });
            return;
        };
        let window = cx.window_id();
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        let document = dashboard.document.clone();
        let background = colors::BACKGROUND;
        let options = WebViewOptions {
            user_data: self.repository.paths.data.join("webview2"),
            url: dashboard.page_url(),
            script: BRIDGE.into(),
            background: ((background.r * 255.0) as u32) << 16 | ((background.g * 255.0) as u32) << 8 | (background.b * 255.0) as u32,
        };
        let callbacks = WebViewCallbacks {
            message: Box::new(move |json| {
                crate::ui::platform::window::post::<Self>(window, move |view, cx| view.dashboard_message(&json, cx));
            }),
            accelerator: Box::new(move |event| {
                let event = event.clone();
                crate::ui::platform::window::with_view::<Self, _>(window, move |view, cx| {
                    let handled = view.dashboard_accelerator(&event, cx);
                    view.after_input(cx);
                    handled
                })
                .unwrap_or(false)
            }),
            resource: Box::new(move |url| resource_for(&document.borrow(), url)),
            navigation: Box::new(|url| url.starts_with(ORIGIN) || url == "about:blank"),
            state: Box::new(move |state| {
                crate::ui::platform::window::post::<Self>(window, move |view, cx| view.dashboard_state(state, cx));
            }),
        };
        match WebView::create(hwnd, options, callbacks) {
            Ok(web) => dashboard.web = Some(web),
            Err(error) => dashboard.error = Some(error),
        }
    }

    fn dashboard_state(&mut self, state: WebViewState, cx: &mut Cx<Self>) {
        let active = self.window_active;
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        match state {
            WebViewState::Created => {
                if active && dashboard.wants_focus && !dashboard.covered
                    && let Some(web) = &dashboard.web
                {
                    web.focus();
                    dashboard.wants_focus = false;
                }
            }
            WebViewState::Loaded(outcome) => {
                dashboard.loaded = true;
                if let Err(error) = outcome {
                    dashboard.error = Some(format!("The dashboard did not load: {error}"));
                }
            }
            WebViewState::Title(title) => {
                dashboard.page_title = Some(title);
                self.set_window_title(cx);
            }
            WebViewState::Failed(error) => dashboard.error = Some(error),
        }
        cx.notify();
    }

    /// A message from the page's bridge script.
    fn dashboard_message(&mut self, json: &str, cx: &mut Cx<Self>) {
        let Ok(message) = serde_json::from_str::<Value>(json) else {
            return;
        };
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        match message.get("type").and_then(Value::as_str) {
            Some("hello") => {
                // A new page: answers for the old one are dropped.
                dashboard.generation += 1;
                dashboard.running = 0;
                dashboard.waiting.clear();
                dashboard.quiet_since = Some(Instant::now());
                self.send_dashboard_config();
            }
            Some("query") => {
                let id = message.get("id").and_then(Value::as_u64).unwrap_or(0);
                let request = QueryRequest {
                    sql: message.get("sql").and_then(Value::as_str).unwrap_or("").to_owned(),
                    params: message.get("params").filter(|params| !params.is_null()).cloned(),
                    limit: message.get("limit").and_then(Value::as_u64).map(|limit| limit as usize),
                };
                if dashboard.engine.is_some() {
                    self.run_dashboard_query(id, request, cx);
                } else if let Some(error) = dashboard.error.clone() {
                    self.reply_dashboard(id, Err(error));
                } else {
                    dashboard.waiting.push((id, request));
                    dashboard.quiet_since = None;
                }
            }
            Some("log") => {
                let level = message.get("level").and_then(Value::as_str).unwrap_or("error").to_owned();
                let text = message.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
                if dashboard.console.len() == CONSOLE_LOG {
                    dashboard.console.remove(0);
                }
                dashboard.console.push(ConsoleRecord { level, message: text });
            }
            _ => {}
        }
        cx.notify();
    }

    fn send_dashboard_config(&self) {
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let sources: Vec<SourceStatus> = match &dashboard.engine {
            Some(engine) => engine.sources.clone(),
            None => Vec::new(),
        };
        let config = json!({
            "type": "config",
            "version": crate::infrastructure::dashboard::BRIDGE_VERSION,
            "title": dashboard.title(),
            "commands": dashboard.manifest.commands,
            "sources": sources,
        });
        if let Some(web) = &dashboard.web {
            web.post_json(&config.to_string());
        }
    }

    fn run_dashboard_query(&mut self, id: u64, request: QueryRequest, cx: &mut Cx<Self>) {
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        let Some(engine) = dashboard.engine.clone() else {
            return;
        };
        dashboard.running += 1;
        dashboard.quiet_since = None;
        let generation = dashboard.generation;
        let asked = request.clone();
        cx.spawn(
            move || engine.query(&request),
            move |view: &mut Self, cx, result: Result<QueryResult, String>| {
                let Some(dashboard) = view.dashboard.as_mut() else {
                    return;
                };
                if dashboard.generation != generation {
                    return;
                }
                dashboard.running = dashboard.running.saturating_sub(1);
                if dashboard.running == 0 {
                    dashboard.quiet_since = Some(Instant::now());
                }
                let record = QueryRecord {
                    sql: asked.sql,
                    params: asked.params,
                    elapsed_ms: result.as_ref().map(|result| result.elapsed_ms).unwrap_or(0.0),
                    rows: result.as_ref().map(|result| result.rows.len()).unwrap_or(0),
                    truncated: result.as_ref().is_ok_and(|result| result.truncated),
                    error: result.as_ref().err().cloned(),
                };
                if dashboard.queries.len() == QUERY_LOG {
                    dashboard.queries.remove(0);
                }
                dashboard.queries.push(record);
                view.reply_dashboard(id, result);
                cx.notify();
            },
        )
        .detach();
    }

    fn reply_dashboard(&self, id: u64, result: Result<QueryResult, String>) {
        let Some(web) = self.dashboard.as_ref().and_then(|dashboard| dashboard.web.as_ref()) else {
            return;
        };
        let reply = match result {
            Ok(result) => json!({"type": "reply", "id": id, "result": result}),
            Err(error) => json!({"type": "reply", "id": id, "error": error}),
        };
        web.post_json(&reply.to_string());
    }

    /// A key pressed in the page with Ctrl, Alt, or one that types nothing.
    /// DataDiver keeps the ones its own commands use, such as Ctrl+P.
    fn dashboard_accelerator(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let m = event.modifiers;
        let function_key = event.key.len() > 1 && event.key.starts_with('f') && event.key[1..].chars().all(|c| c.is_ascii_digit());
        if !(m.control || m.alt || function_key) {
            return false;
        }
        let Ok(stroke) = crate::application::keymap::canonical_keystroke(&event.stroke()) else {
            return false;
        };
        let action = self
            .keymap
            .binding_for(&stroke, KeyContext::Application)
            .or_else(|| self.keymap.binding_for(&stroke, KeyContext::Window));
        let Some(action) = action else {
            return false;
        };
        let command = crate::application::keymap::ACTIONS
            .iter()
            .find(|spec| spec.id == action)
            .and_then(|spec| spec.command);
        if !command.is_some_and(dashboard_allows) {
            return false;
        }
        if let Some(dashboard) = &mut self.dashboard {
            dashboard.wants_focus = true;
        }
        self.dispatch_keymap_action(action, cx);
        cx.notify();
        true
    }

    /// The command palette while a dashboard shows: the dashboard's own
    /// commands first, then the DataDiver commands that work here.
    pub(crate) fn open_dashboard_command_palette(&mut self, cx: &mut Cx<Self>) {
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let mut entries = Vec::new();
        let mut items = Vec::new();
        for command in dashboard.commands() {
            let mut item = crate::ui::widgets::list::Item::new(command.title.clone()).keywords(format!("{} dashboard", command.id));
            if let Some(key) = &command.key {
                item = item.detail(crate::domain::keystroke::display(key));
            }
            items.push(item.section(0));
            entries.push(DashboardPaletteEntry::Page(command.id.clone()));
        }
        let commands = active_commands().into_iter().filter(|command| dashboard_allows(command.id)).collect::<Vec<_>>();
        for (command, label, shortcut) in crate::ui::command_palette::matching_commands(&commands, &self.keymap, "") {
            let item = crate::ui::widgets::list::Item::new(label).keywords(command.key);
            let item = if shortcut.is_empty() { item } else { item.detail(shortcut) };
            items.push(item.section(1));
            entries.push(DashboardPaletteEntry::App(command.id));
        }
        let title = dashboard.title();
        self.palette("Commands", "Find a command", items, PaletteKind::DashboardCommands(entries), cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.palette.list.set_sections(vec![title, "DataDiver".into()]);
        }
    }

    /// The page's queries, newest first, with its errors and warnings;
    /// choosing one copies it.
    pub(crate) fn open_dashboard_queries(&mut self, cx: &mut Cx<Self>) {
        let Some(dashboard) = &self.dashboard else {
            self.error = Some("No dashboard is open in this window".into());
            cx.notify();
            return;
        };
        let mut copies = Vec::new();
        let mut items = Vec::new();
        for record in dashboard.console.iter().rev() {
            let first = record.message.lines().next().unwrap_or("").to_owned();
            items.push(crate::ui::widgets::list::Item::new(first).detail(record.level.clone()).section(0));
            copies.push(record.message.clone());
        }
        for record in dashboard.queries.iter().rev() {
            let one_line = record.sql.split_whitespace().collect::<Vec<_>>().join(" ");
            let detail = match &record.error {
                Some(error) => format!("failed: {}", error.lines().next().unwrap_or("")),
                None => format!(
                    "{} ms · {} row{}{}",
                    record.elapsed_ms,
                    record.rows,
                    if record.rows == 1 { "" } else { "s" },
                    if record.truncated { " · row limit" } else { "" }
                ),
            };
            let mut item = crate::ui::widgets::list::Item::new(one_line).detail(detail).section(1);
            item.muted = false;
            items.push(item);
            let mut copy = record.sql.clone();
            if let Some(params) = &record.params {
                copy.push_str(&format!("\n-- parameters: {params}"));
            }
            if let Some(error) = &record.error {
                copy.push_str(&format!("\n-- error: {}", error.replace('\n', "\n-- ")));
            }
            copies.push(copy);
        }
        self.palette("Dashboard queries", "Find a query; Enter copies it", items, PaletteKind::DashboardQueries(copies), cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.palette.list.set_sections(vec!["Page messages".into(), "Queries, newest first".into()]);
            dialog.palette.list.empty_text = "The page has run no queries yet".into();
        }
    }

    /// Runs a command the dashboard added, as its key would.
    pub(crate) fn run_dashboard_command(&mut self, id: &str) {
        if let Some(web) = self.dashboard.as_ref().and_then(|dashboard| dashboard.web.as_ref()) {
            web.post_json(&json!({"type": "command", "id": id}).to_string());
            web.focus();
        }
    }

    pub(crate) fn reload_dashboard(&mut self, cx: &mut Cx<Self>) {
        let Some(dashboard) = &mut self.dashboard else {
            return;
        };
        let path = dashboard.path.clone();
        match read_dashboard(&path) {
            Ok((html, manifest)) => {
                dashboard.document.borrow_mut().html = html;
                dashboard.manifest = manifest;
                dashboard.modified = modified_time(&path);
                dashboard.generation += 1;
                dashboard.sources_generation += 1;
                dashboard.running = 0;
                dashboard.loaded = false;
                dashboard.error = None;
                dashboard.page_title = None;
                dashboard.queries.clear();
                dashboard.console.clear();
                dashboard.opened_at = Instant::now();
                dashboard.quiet_since = None;
                self.error = None;
                self.start_dashboard_engine(cx);
                if let Some(web) = self.dashboard.as_ref().and_then(|dashboard| dashboard.web.as_ref()) {
                    web.reload();
                }
                self.set_window_title(cx);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Reloads the dashboard if its file changed, such as after an edit
    /// made while the window was in the background.
    pub(crate) fn check_dashboard_file(&mut self, cx: &mut Cx<Self>) {
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let modified = modified_time(&dashboard.path);
        if modified.is_some() && modified != dashboard.modified {
            self.reload_dashboard(cx);
        }
    }

    pub(crate) fn close_dashboard(&mut self) {
        if let Some(dashboard) = self.dashboard.take()
            && let Some(web) = &dashboard.web
        {
            web.close();
        }
    }

    /// Opens the dashboard for `dashboard check`, then waits for it to settle.
    pub(crate) fn start_dashboard_check(&mut self, path: PathBuf, check: DashboardCheck, cx: &mut Cx<Self>) {
        self.dashboard_checking = true;
        self.open_dashboard(path.clone(), cx);
        if self.dashboard.is_none() {
            let report = json!({"dashboard": path, "ok": false, "error": self.error.clone().unwrap_or_default()});
            println!("{}", crate::cli::agent_json(&report));
            CHECK_EXIT.store(1, std::sync::atomic::Ordering::Relaxed);
            cx.quit();
            return;
        }
        self.poll_dashboard_check(check, cx);
    }

    fn poll_dashboard_check(&mut self, check: DashboardCheck, cx: &mut Cx<Self>) {
        let Some(dashboard) = &self.dashboard else {
            cx.quit();
            return;
        };
        let elapsed = dashboard.opened_at.elapsed();
        let failed = dashboard.error.is_some();
        let settled = dashboard.loaded
            && dashboard.engine.is_some()
            && dashboard.running == 0
            && dashboard.waiting.is_empty()
            && dashboard.quiet_since.is_some_and(|since| since.elapsed() >= check.quiet);
        if !(settled || failed || elapsed >= check.timeout) {
            cx.timer(std::time::Duration::from_millis(50), move |view, cx| view.poll_dashboard_check(check, cx)).detach();
            return;
        }
        let timed_out = !settled && !failed;
        match (&check.capture, dashboard.web.as_ref().filter(|web| web.is_ready())) {
            (Some(path), Some(web)) => {
                let window = cx.window_id();
                let path_shown = path.clone();
                let late = path.clone();
                web.capture_png(
                    path.clone(),
                    Box::new(move |result| {
                        crate::ui::platform::window::post::<Self>(window, move |view, cx| {
                            view.finish_dashboard_check(timed_out, Some((path_shown, result)), cx);
                        });
                    }),
                );
                // A capture that never answers must not keep the check running.
                cx.timer(std::time::Duration::from_secs(10), move |view, cx| {
                    view.finish_dashboard_check(timed_out, Some((late, Err("the capture did not finish in 10 seconds".into()))), cx);
                })
                .detach();
            }
            _ => self.finish_dashboard_check(timed_out, None, cx),
        }
    }

    fn finish_dashboard_check(&mut self, timed_out: bool, capture: Option<(PathBuf, Result<(), String>)>, cx: &mut Cx<Self>) {
        // The capture's answer and its time limit both end up here; the first reports.
        if !std::mem::take(&mut self.dashboard_checking) {
            return;
        }
        let Some(dashboard) = &self.dashboard else {
            cx.quit();
            return;
        };
        let sources = dashboard.engine.as_ref().map(|engine| engine.sources.clone()).unwrap_or_default();
        let queries = dashboard
            .queries
            .iter()
            .map(|query| {
                json!({
                    "sql": query.sql,
                    "params": query.params,
                    "elapsedMs": query.elapsed_ms,
                    "rows": query.rows,
                    "truncated": query.truncated,
                    "error": query.error,
                })
            })
            .collect::<Vec<_>>();
        let messages = dashboard
            .console
            .iter()
            .map(|record| json!({"level": record.level, "message": record.message}))
            .collect::<Vec<_>>();
        // DataDiver keeps keys its own commands use; the page never sees them.
        let conflicts = dashboard
            .manifest
            .commands
            .iter()
            .filter_map(|command| {
                let key = command.key.as_ref()?;
                let action = self
                    .keymap
                    .binding_for(key, KeyContext::Application)
                    .or_else(|| self.keymap.binding_for(key, KeyContext::Window))?;
                let spec = crate::application::keymap::ACTIONS.iter().find(|spec| spec.id == action)?;
                spec.command.filter(|command| dashboard_allows(*command))?;
                Some(json!({"command": command.id, "key": key, "takenBy": spec.name}))
            })
            .collect::<Vec<_>>();
        let failed_sources = sources.iter().any(|source| source.error.is_some());
        let failed_queries = dashboard.queries.iter().any(|query| query.error.is_some());
        let page_errors = dashboard.console.iter().any(|record| record.level == "error");
        let capture_error = capture.as_ref().and_then(|(_, result)| result.as_ref().err().cloned());
        let ok = !timed_out
            && dashboard.error.is_none()
            && !failed_sources
            && !failed_queries
            && !page_errors
            && conflicts.is_empty()
            && capture_error.is_none();
        let mut report = json!({
            "dashboard": dashboard.path,
            "title": dashboard.title(),
            "ok": ok,
            "sources": sources,
            "queries": queries,
            "pageMessages": messages,
            "keyConflicts": conflicts,
            "timedOut": timed_out,
            "seconds": (dashboard.opened_at.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        });
        if let Some(error) = &dashboard.error {
            report["error"] = json!(error);
        }
        if let Some(error) = &self.error {
            report["notice"] = json!(error);
        }
        if let Some((path, result)) = capture {
            report["capture"] = match result {
                Ok(()) => json!(path),
                Err(error) => json!({"path": path, "error": error}),
            };
        }
        println!("{}", crate::cli::agent_json(&report));
        CHECK_EXIT.store(if ok { 0 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
        cx.quit();
    }

    /// Makes Explorer open `.ddash` files with this DataDiver, or stops it.
    pub(crate) fn register_dashboards(&mut self, register: bool, cx: &mut Cx<Self>) {
        use crate::infrastructure::file_association;
        let outcome = if register {
            file_association::current_exe().and_then(|exe| file_association::register(&exe))
        } else {
            file_association::unregister()
        };
        match outcome {
            Ok(()) if register => self.copy_status = Some("Double-clicking a .ddash file now opens it in DataDiver".into()),
            Ok(()) => self.copy_status = Some(".ddash files no longer open with DataDiver".into()),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Puts this DataDiver's folder on the user's PATH, or takes it off.
    pub(crate) fn add_to_path(&mut self, add: bool, cx: &mut Cx<Self>) {
        use crate::infrastructure::user_path;
        // Telling other windows about the change can wait on a slow one.
        cx.spawn(
            move || if add { user_path::add() } else { user_path::remove() },
            move |view: &mut Self, cx, outcome: Result<bool, String>| {
                match outcome {
                    Ok(changed) => view.copy_status = Some(user_path::outcome_message(add, changed)),
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn open_dashboard_dev_tools(&mut self) {
        match self.dashboard.as_ref().and_then(|dashboard| dashboard.web.as_ref()) {
            Some(web) => web.open_dev_tools(),
            None => self.error = Some("No dashboard is open in this window".into()),
        }
    }

    /// The window moved: the page's popups must follow it.
    pub(crate) fn dashboard_window_moved(&self) {
        if let Some(web) = self.dashboard.as_ref().and_then(|dashboard| dashboard.web.as_ref()) {
            web.parent_moved();
        }
    }

    /// Whether something drawn by DataDiver covers the page.
    fn dashboard_covered(&self) -> bool {
        self.modal_dialog() || self.notices.log_open || self.cache_drop_open || self.keymap_editor.is_some()
    }

    /// Hides the page while DataDiver draws over it, and shows it again after.
    pub(crate) fn sync_dashboard_cover(&mut self, cx: &mut Cx<Self>) {
        let covered = self.dashboard_covered();
        let hwnd = cx.hwnd();
        if let Some(dashboard) = &mut self.dashboard
            && covered != dashboard.covered
        {
            dashboard.covered = covered;
            if let Some(web) = &dashboard.web {
                if covered {
                    // The page can't draw under DataDiver's dialogs, so a
                    // picture of it stands in while one is open.
                    dashboard.snapshot = web.snapshot();
                    web.set_visible(false);
                    dashboard.wants_focus |= hwnd.is_some_and(crate::ui::platform::window::child_has_focus);
                    if let Some(hwnd) = hwnd {
                        crate::ui::platform::window::take_focus(hwnd);
                    }
                } else {
                    web.set_visible(true);
                    dashboard.snapshot = None;
                }
            }
        }
        if !covered
            && let Some(dashboard) = &mut self.dashboard
            && dashboard.wants_focus
            && self.window_active
            && self.focus.is_none()
            && let Some(web) = &dashboard.web
            && web.is_ready()
        {
            web.focus();
            dashboard.wants_focus = false;
        }
    }

    /// Draws the window while it shows a dashboard.
    pub(crate) fn render_dashboard(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        let fonts = self.fonts.clone();
        let font = &fonts.ui;
        let line = text::line_height(font);
        let bounds = canvas.bounds();
        let modal = self.modal_dialog() || self.cache_drop_open;
        let mouse = canvas.mouse();
        if modal {
            canvas.set_mouse(None);
        }
        // Header: what is open, and the log button. No search: the page has its own controls.
        let header = rect(bounds.x, bounds.y, bounds.w, button_height(font) + 12.0);
        let log = rect(header.right() - 12.0 - button_height(font), header.y + 6.0, button_height(font), button_height(font));
        self.notices.paint_button(canvas, log, font);
        let title = self.dashboard.as_ref().map(|dashboard| dashboard.title()).unwrap_or_default();
        canvas.text(
            rect(header.x + 12.0, header.y, (log.x - header.x - 24.0).max(0.0), header.h),
            &title,
            &font.semibold(),
            colors::FOREGROUND,
            Align::Left,
        );
        // Status bar.
        let status_height = line + 8.0;
        let status = rect(bounds.x, bounds.bottom() - status_height, bounds.w, status_height);
        self.paint_dashboard_status(canvas, status, cx);
        // Body: the file tree, then the page.
        let mut body = rect(bounds.x, header.bottom(), bounds.w, (status.y - header.bottom()).max(0.0));
        if self.database_tree_open {
            let focus = self.focus;
            let width = self.database_tree_width.clamp(160.0, (body.w - 240.0).clamp(160.0, 900.0));
            let (tree, rest) = body.split_left(width);
            self.database_tree.paint(canvas, tree, &fonts, focus);
            let splitter = rect(tree.right() - 3.0, tree.y, 6.0, tree.h);
            canvas.hit_cursor(splitter, Hit::Splitter(Splitter::DatabaseTree), Cursor::ResizeColumns);
            body = rest;
        }
        canvas.fill(rect(body.x, body.y, body.w, 1.0), colors::BORDER);
        let page = rect(body.x, body.y + 1.0, body.w, (body.h - 1.0).max(0.0));
        let scale = canvas.scale();
        if let Some(dashboard) = &self.dashboard {
            if let Some(web) = &dashboard.web {
                web.set_bounds(windows::Win32::Foundation::RECT {
                    left: (page.x * scale).round() as i32,
                    top: (page.y * scale).round() as i32,
                    right: (page.right() * scale).round() as i32,
                    bottom: (page.bottom() * scale).round() as i32,
                });
            }
            if let Some(snapshot) = &dashboard.snapshot {
                snapshot.draw(canvas, page);
            }
            if let Some(error) = &dashboard.error {
                // Shown where the page would be; the browser may be absent.
                let message = rect(page.x + 24.0, page.y + 24.0, (page.w - 48.0).max(0.0), (page.h - 48.0).max(0.0));
                canvas.paragraph(message, error, font, colors::DANGER);
            }
        }
        canvas.hit(page, Hit::Surface);
        self.notices.paint_log(canvas, bounds, header.bottom(), &fonts);
        if modal {
            canvas.set_mouse(mouse);
        }
        if self.cache_drop_open {
            self.paint_cache_drop(canvas);
        }
        self.paint_dialog(canvas, cx);
    }

    fn paint_dashboard_status(&mut self, canvas: &mut Canvas<Hit>, r: Rect, cx: &mut Cx<Self>) {
        let fonts = self.fonts.clone();
        let font = &fonts.small;
        canvas.fill(rect(r.x, r.y, r.w, 1.0), colors::BORDER);
        let inner = r.inset_xy(12.0, 0.0);
        // A message, which would pop up over the page, shows here instead.
        if let Some(toast) = self.notices.toast() {
            let color = match toast.kind {
                crate::ui::notices::NoticeKind::Error => colors::DANGER,
                crate::ui::notices::NoticeKind::Status => colors::FOREGROUND,
            };
            let message = toast.text.lines().next().unwrap_or("").to_owned();
            canvas.text(inner, &message, font, color, Align::Left);
            let _ = cx;
            return;
        }
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let mut parts = Vec::new();
        let ready = dashboard.engine.as_ref().map(|engine| engine.sources.iter().filter(|source| source.error.is_none()).count());
        parts.push(match ready {
            Some(ready) => format!("{ready} of {} sources", dashboard.manifest.sources.len()),
            None if dashboard.error.is_none() => "Opening sources…".into(),
            None => "Sources failed".into(),
        });
        if dashboard.running > 0 {
            parts.push(format!("{} running", dashboard.running));
        }
        let failed = dashboard.queries.iter().filter(|query| query.error.is_some()).count();
        if let Some(last) = dashboard.queries.last() {
            parts.push(match &last.error {
                Some(_) => "Last query failed".to_owned(),
                None => format!(
                    "Last query {} ms, {} row{}{}",
                    last.elapsed_ms,
                    last.rows,
                    if last.rows == 1 { "" } else { "s" },
                    if last.truncated { " (row limit reached)" } else { "" }
                ),
            });
        }
        let errors = dashboard.console.iter().filter(|record| record.level == "error").count();
        let color = if failed > 0 || errors > 0 { colors::DANGER } else { colors::MUTED };
        if failed > 0 {
            parts.push(format!("{failed} failed quer{}", if failed == 1 { "y" } else { "ies" }));
        }
        if errors > 0 {
            parts.push(format!("{errors} page error{}", if errors == 1 { "" } else { "s" }));
        }
        canvas.text(inner, &parts.join("  ·  "), font, color, Align::Left);
        let hint = "Ctrl+Shift+P: dashboard commands";
        let width = text::measure(hint, font).0 + 4.0;
        if width * 2.0 < inner.w {
            canvas.text(rect(inner.right() - width, inner.y, width, inner.h), hint, font, colors::MUTED, Align::Left);
        }
    }
}

/// `\\?\C:\x` as `C:\x`, which reads better and which DuckDB understands.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(folder: &Path) -> Document {
        Document {
            html: "<html>board</html>".into(),
            folder: folder.to_path_buf(),
        }
    }

    #[test]
    fn the_page_gets_its_own_files_and_allowed_hosts_only() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("chart.js"), "chart()").unwrap();
        std::fs::write(folder.path().join("my notes.txt"), "x").unwrap();
        let document = document(folder.path());
        let served = |url: &str| match resource_for(&document, url) {
            Resource::Serve { body, content_type, .. } => format!("{content_type}: {}", String::from_utf8_lossy(&body)),
            Resource::Pass => "pass".into(),
            Resource::Missing => "missing".into(),
            Resource::Blocked => "blocked".into(),
        };
        assert_eq!(served(&format!("{ORIGIN}/sales.ddash")), "text/html; charset=utf-8: <html>board</html>");
        assert_eq!(served(&format!("{ORIGIN}/sales.ddash?x=1")), "text/html; charset=utf-8: <html>board</html>");
        assert_eq!(served(&format!("{ORIGIN}/chart.js")), "text/javascript; charset=utf-8: chart()");
        assert_eq!(served(&format!("{ORIGIN}/my%20notes.txt")), "text/plain; charset=utf-8: x");
        assert_eq!(served(&format!("{ORIGIN}/../secret.csv")), "missing");
        assert_eq!(served(&format!("{ORIGIN}/%2E%2E/secret.csv")), "missing");
        assert_eq!(served(&format!("{ORIGIN}/C:/Windows/win.ini")), "missing");
        assert_eq!(served("https://cdn.jsdelivr.net/npm/chart.js@4.4.1"), "pass");
        assert_eq!(served("https://evil.example.com/steal?data=1"), "blocked");
        assert_eq!(served("http://localhost:8080/"), "blocked");
        assert_eq!(served("https://cdn.jsdelivr.net.evil.com/x.js"), "blocked");
        assert_eq!(served("data:image/png;base64,AAAA"), "pass");
    }

    #[test]
    fn sources_that_open_after_the_page_loads_still_arrive() {
        use super::super::test_support::{Viewer, empty_frame, repository};
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("sales.csv"), "region,amount\nnorth,1\nsouth,2\n").unwrap();
        let board = home.path().join("board.ddash");
        std::fs::write(&board, r#"<script type="application/json" id="datadiver">{"sources": {"sales": "sales.csv"}}</script>"#).unwrap();
        let viewer = Viewer::open(empty_frame(), None, None, repository(home.path()));
        viewer.update(|view, cx| {
            view.navigate_to_file(FileEntry::Source(board.clone()), false, false, false, cx);
            // With a real profile, syncing the watched files' views takes a
            // while: the page loads and asks before its sources are open.
            view.dashboard_message(r#"{"type":"hello","version":1}"#, cx);
            view.dashboard_message(r#"{"type":"query","id":1,"sql":"select count(*) as n from sales"}"#, cx);
        });
        viewer.settle();
        viewer.read(|view| {
            let dashboard = view.dashboard.as_ref().expect("the dashboard is open");
            assert!(dashboard.engine.is_some(), "the sources were dropped: {:?}", dashboard.error);
            assert_eq!(dashboard.queries.len(), 1);
            assert_eq!(dashboard.queries[0].error, None);
            assert_eq!(dashboard.queries[0].rows, 1);
            assert_eq!(dashboard.running, 0);
        });
    }

    #[test]
    fn paths_round_trip_through_the_page_address() {
        assert_eq!(percent_decode(&percent_encode("Q1 sales & costs.ddash")), "Q1 sales & costs.ddash");
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\boards\a.ddash")), PathBuf::from(r"C:\boards\a.ddash"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\a.ddash")), PathBuf::from(r"\\server\share\a.ddash"));
    }
}
