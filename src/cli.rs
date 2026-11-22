//! `datadiver dashboard …`: commands for building dashboards from a shell,
//! by hand or by an agent such as Claude. Everything prints JSON except the
//! guide, so an agent can read the results.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::infrastructure::dashboard::{DashboardEngine, Manifest, QueryRequest, SourceKind, SourceSpec, prepare_profile, read_dashboard};
use crate::infrastructure::json_store::{StorePaths, ViewRepository};
use crate::infrastructure::source_database::SourceDatabase;

/// What `datadiver dashboard guide` prints.
pub const AGENT_GUIDE: &str = include_str!("../docs/dashboards/agent-guide.md");

const USAGE: &str = "\
datadiver dashboard <command>

  guide                          How to build a dashboard: the file, the manifest and window.datadiver
  sources                        Tables and views of the profile database, such as watched files
  schema <file | relation>       Columns, row count and sample rows of a data file or profile relation
  query [board.ddash] <sql>      Runs SQL as a dashboard page would
        [--source alias=path] [--source alias=profile:schema.name] [--params JSON] [--limit N]
  check <board.ddash>            Opens the dashboard out of sight, waits for its queries, and reports
        [--capture shot.png] [--timeout SECONDS] [--size WxH]
  register | unregister          Opens .ddash files with this DataDiver when double-clicked, or stops

Every command takes --home DIR to use another profile folder.";

/// What `datadiver --help` prints. It starts with where agents go next, since
/// `--help` is the first thing an agent tries on an unknown program.
pub fn help_text() -> String {
    format!(
        "\
DataDiver {version}: fast data exploration with DuckDB and Polars, in a desktop window.

Agents and scripts: DataDiver can show dashboards, HTML pages whose charts and
controls query data through DataDiver. To build one, start with
    datadiver dashboard guide        the full guide: file format, data sources, window.datadiver
    datadiver dashboard help         every dashboard command (sources, schema, query, check)
Commands under `datadiver dashboard` print JSON and never open a visible window.

Usage:
  datadiver [PATH]                 open a data file (CSV, TSV, Parquet, XLSX), or a .ddash dashboard
  datadiver dashboard <command>    dashboard tools; see `datadiver dashboard help`
  datadiver --help | --version

Options:
  --file PATH       open a CSV (including .csv.gz), TSV, XLSX, or Parquet source
  --view PATH       open a saved JSON view
  --search TEXT     filter displayed rows
  --slicer COLUMN   add a source-column slicer
  --rows N          open synthetic data
  --window WxH      set window size
  --home DIR        use another profile folder
  --ephemeral       use a throwaway profile
  --no-restore      don't reopen the last session
  --exit-after MS   close on a timer
  --capture PATH    save a PNG of the window",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// Answers `--help`, `--version` and their usual spellings, if `argument` is
/// one; returns whether it was.
pub fn run_help(argument: &str) -> bool {
    match argument {
        "--help" | "-h" | "-?" | "/?" | "help" => {
            attach_console();
            println!("{}", help_text());
            true
        }
        "--version" | "-V" | "version" => {
            attach_console();
            println!("datadiver {}", env!("CARGO_PKG_VERSION"));
            true
        }
        _ => false,
    }
}

/// Lets output reach the console of a shell that started DataDiver, which as
/// a windows program gets none of its own.
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    let redirected = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.is_ok_and(|handle| !handle.is_invalid() && !handle.0.is_null());
    if !redirected {
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

fn print_json(value: &Value) {
    println!("{}", agent_json(value));
}

/// JSON with one field per line and one list item per line, each item on a
/// line of its own: a result's rows read like a table, and stay short.
pub fn agent_json(value: &Value) -> String {
    fn compact(value: &Value) -> String {
        serde_json::to_string(value).unwrap_or_default()
    }
    fn write(value: &Value, depth: usize) -> String {
        let pad = |depth: usize| "  ".repeat(depth);
        match value {
            Value::Object(map) if !map.is_empty() => {
                let fields = map
                    .iter()
                    .map(|(key, value)| format!("{}{}: {}", pad(depth + 1), compact(&Value::String(key.clone())), write(value, depth + 1)))
                    .collect::<Vec<_>>();
                format!("{{\n{}\n{}}}", fields.join(",\n"), pad(depth))
            }
            Value::Array(items) if items.iter().any(|item| item.is_array() || item.is_object()) => {
                let items = items.iter().map(|item| format!("{}{}", pad(depth + 1), compact(item))).collect::<Vec<_>>();
                format!("[\n{}\n{}]", items.join(",\n"), pad(depth))
            }
            other => compact(other),
        }
    }
    write(value, 0)
}

struct Arguments {
    positional: Vec<String>,
    options: Vec<(String, String)>,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut iter = arguments.iter();
        while let Some(argument) = iter.next() {
            if let Some(name) = argument.strip_prefix("--") {
                let value = iter.next().ok_or_else(|| format!("--{name} needs a value"))?;
                options.push((name.to_owned(), value.clone()));
            } else {
                positional.push(argument.clone());
            }
        }
        Ok(Self { positional, options })
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options.iter().rev().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.options.iter().filter(move |(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    fn check_options(&self, allowed: &[&str]) -> Result<(), String> {
        match self.options.iter().find(|(key, _)| key != "home" && !allowed.contains(&key.as_str())) {
            Some((key, _)) => Err(format!("unknown option --{key}")),
            None => Ok(()),
        }
    }

    fn repository(&self) -> ViewRepository {
        ViewRepository::new(match self.option("home") {
            Some(home) => StorePaths::for_home(Path::new(home)),
            None => StorePaths::operating_system(),
        })
    }
}

/// Runs `datadiver dashboard <arguments>` and returns the exit code.
pub fn run_dashboard(arguments: &[String]) -> i32 {
    attach_console();
    let Some((command, rest)) = arguments.split_first() else {
        println!("{USAGE}");
        return 2;
    };
    let parsed = match Arguments::parse(rest) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return 2;
        }
    };
    let outcome = match command.as_str() {
        "guide" => {
            println!("{AGENT_GUIDE}");
            Ok(0)
        }
        "sources" => sources(&parsed),
        "schema" => schema(&parsed),
        "query" => query(&parsed),
        "check" => check(&parsed),
        "register" => crate::infrastructure::file_association::current_exe()
            .and_then(|exe| crate::infrastructure::file_association::register(&exe).map(|()| exe))
            .map(|exe| {
                println!("Double-clicking a .ddash file now opens it with {}", exe.display());
                0
            }),
        "unregister" => crate::infrastructure::file_association::unregister().map(|()| {
            println!(".ddash files no longer open with DataDiver");
            0
        }),
        "help" | "--help" | "-h" | "-?" | "/?" => {
            println!("{USAGE}");
            Ok(0)
        }
        other => Err(format!("unknown dashboard command \"{other}\"\n\n{USAGE}")),
    };
    match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn open_database(repository: &ViewRepository) -> Result<std::sync::Arc<SourceDatabase>, String> {
    let database = SourceDatabase::for_profile(&repository.paths.config);
    database.initialize()?;
    if let Some(error) = database.startup_error() {
        eprintln!("warning: the startup SQL script failed: {error}");
    }
    prepare_profile(&database, repository)?;
    Ok(database)
}

fn sources(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&[])?;
    let repository = arguments.repository();
    let database = open_database(&repository)?;
    let connection = database.connect()?;
    let mut listed = Vec::new();
    for relation in database.relations()? {
        let name = [relation.catalog.as_deref(), Some(relation.schema.as_str()), Some(relation.name.as_str())]
            .into_iter()
            .flatten()
            .map(diver_sql::quote_if_needed)
            .collect::<Vec<_>>()
            .join(".");
        let columns = connection
            .prepare(&format!("DESCRIBE {name}"))
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok(json!({"name": row.get::<_, String>(0)?, "type": row.get::<_, String>(1)?})))?
                    .collect::<Result<Vec<_>, _>>()
            });
        let mut entry = json!({
            "name": name,
            "kind": match relation.kind {
                crate::domain::documents::RelationKind::View => "view",
                _ => "table",
            },
            "manifest": {"profile": name},
        });
        match columns {
            Ok(columns) => entry["columns"] = json!(columns),
            Err(error) => entry["error"] = json!(error.to_string()),
        }
        listed.push(entry);
    }
    print_json(&json!({
        "profile": repository.paths.config,
        "note": "Use a relation as a dashboard source with {\"profile\": \"<name>\"}. Watched files are views in the files schema.",
        "relations": listed,
    }));
    Ok(0)
}

/// A source given on the command line: `alias=path` or `alias=profile:name`.
fn source_option(text: &str) -> Result<SourceSpec, String> {
    let (alias, target) = text.split_once('=').ok_or_else(|| format!("--source needs alias=path, not \"{text}\""))?;
    let kind = match target.strip_prefix("profile:") {
        Some(name) => SourceKind::Profile(name.to_owned()),
        None => SourceKind::File(target.to_owned()),
    };
    Ok(SourceSpec {
        alias: alias.to_owned(),
        kind,
    })
}

/// The dashboard a command reads its sources from: a `.ddash` file, or the
/// current folder when only `--source` options name them.
fn engine_for(arguments: &Arguments, dashboard: Option<&Path>, database: std::sync::Arc<SourceDatabase>) -> Result<DashboardEngine, String> {
    let (mut manifest, anchor) = match dashboard {
        Some(path) => (read_dashboard(path)?.1, path.to_path_buf()),
        None => (
            Manifest::default(),
            std::env::current_dir().map_err(|error| error.to_string())?.join("command-line.ddash"),
        ),
    };
    for text in arguments.all("source") {
        let spec = source_option(text)?;
        manifest.sources.retain(|known| !known.alias.eq_ignore_ascii_case(&spec.alias));
        manifest.sources.push(spec);
    }
    let engine = DashboardEngine::open(database, &manifest, &anchor)?;
    for error in engine.source_errors() {
        eprintln!("warning: source {error}");
    }
    Ok(engine)
}

fn schema(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&["rows"])?;
    let [target] = arguments.positional.as_slice() else {
        return Err("schema needs one data file or profile relation, such as data/orders.parquet or files.orders".into());
    };
    let repository = arguments.repository();
    let database = open_database(&repository)?;
    let looks_like_file = Path::new(target).extension().is_some() && (target.contains(['/', '\\']) || Path::new(target).exists());
    let kind = if looks_like_file { SourceKind::File(target.clone()) } else { SourceKind::Profile(target.clone()) };
    let manifest = Manifest {
        version: 1,
        sources: vec![SourceSpec { alias: "t".into(), kind }],
        ..Manifest::default()
    };
    let anchor = std::env::current_dir().map_err(|error| error.to_string())?.join("command-line.ddash");
    let engine = DashboardEngine::open(database, &manifest, &anchor)?;
    if let Some(error) = engine.sources[0].error.clone() {
        return Err(error);
    }
    let rows = arguments.option("rows").map(str::parse::<usize>).transpose().map_err(|_| "--rows needs a number")?.unwrap_or(5);
    let columns = engine.query(&QueryRequest::new("describe t"))?;
    let count = engine.query(&QueryRequest::new("select count(*) as rows from t"))?;
    let mut sample = QueryRequest::new("select * from t");
    sample.limit = Some(rows.max(1));
    let sample = engine.query(&sample)?;
    print_json(&json!({
        "source": engine.sources[0],
        "columns": columns.rows.iter().map(|row| json!({"name": row[0], "type": row[1], "null": row[2]})).collect::<Vec<_>>(),
        "rowCount": count.rows.first().and_then(|row| row.first()),
        "sample": {"columns": sample.columns, "rows": sample.rows},
    }));
    Ok(0)
}

fn query(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&["source", "params", "limit"])?;
    let (dashboard, sql) = match arguments.positional.as_slice() {
        [sql] => (None, sql),
        [dashboard, sql] => (Some(PathBuf::from(dashboard)), sql),
        _ => return Err("query needs SQL, and optionally a .ddash file before it".into()),
    };
    let repository = arguments.repository();
    let database = open_database(&repository)?;
    let engine = engine_for(arguments, dashboard.as_deref(), database)?;
    let mut request = QueryRequest::new(sql.clone());
    if let Some(params) = arguments.option("params") {
        request.params = Some(serde_json::from_str(params).map_err(|error| format!("--params is not valid JSON: {error}"))?);
    }
    if let Some(limit) = arguments.option("limit") {
        request.limit = Some(limit.parse().map_err(|_| "--limit needs a number")?);
    }
    let result = engine.query(&request)?;
    print_json(&serde_json::to_value(result).map_err(|error| error.to_string())?);
    Ok(0)
}

fn check(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&["capture", "timeout", "size", "quiet"])?;
    let [dashboard] = arguments.positional.as_slice() else {
        return Err("check needs one .ddash file".into());
    };
    let path = PathBuf::from(dashboard);
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let seconds = |name: &str, default: f64| -> Result<Duration, String> {
        let value = arguments.option(name).map(str::parse::<f64>).transpose().map_err(|_| format!("--{name} needs seconds"))?;
        Ok(Duration::from_secs_f64(value.unwrap_or(default).max(0.0)))
    };
    let mut options = crate::ui::dataframe_view::ViewerOptions {
        repository: arguments.repository(),
        restore_window: false,
        load_path: Some(path.canonicalize().unwrap_or(path)),
        dashboard_check: Some(crate::ui::dataframe_view::dashboard::DashboardCheck {
            capture: arguments.option("capture").map(PathBuf::from),
            timeout: seconds("timeout", 20.0)?,
            quiet: seconds("quiet", 1.0)?,
        }),
        ..Default::default()
    };
    if let Some(size) = arguments.option("size") {
        let (width, height) = size.split_once(['x', 'X']).ok_or("--size needs WxH, such as 1400x900")?;
        options.width = width.parse().map_err(|_| "--size needs WxH, such as 1400x900")?;
        options.height = height.parse().map_err(|_| "--size needs WxH, such as 1400x900")?;
    }
    crate::ui::platform::window::run::<crate::DataFrameView>(move |runtime| {
        crate::DataFrameView::open_another(runtime, options);
    });
    crate::infrastructure::source_database::SourceDatabase::checkpoint_profiles()?;
    Ok(crate::ui::dataframe_view::dashboard::CHECK_EXIT.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_sends_agents_to_the_dashboard_guide() {
        let help = super::help_text();
        assert!(help.contains("datadiver dashboard guide"));
        assert!(help.contains(env!("CARGO_PKG_VERSION")));
        assert!(super::AGENT_GUIDE.starts_with("# Building a DataDiver dashboard"));
    }
}
