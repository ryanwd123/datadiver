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
  sources [filter]               Tables and views of the profile database (watched files, attached
                                 databases), the startup script, and what it attaches
  schema <file | relation>       Columns, row count and sample rows of a data file or profile relation
  query [board.ddash] <sql>      Runs SQL as a dashboard page would
        [--source alias=path] [--source alias=profile:schema.name] [--params JSON] [--limit N]
        [--format json | tsv | table]   table is easiest to read; tsv to pipe on
  check <board.ddash>            Opens the dashboard out of sight, waits for its queries, and reports
        [--capture shot.png] [--full-page] [--size WxH] [--timeout SECONDS]
        [--run \"js\"]...             run JavaScript in the page once it settles, such as clicking a
                                 control, then wait for its queries; repeat for several steps
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

/// Options that take no value.
const FLAGS: &[&str] = &["full-page"];

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
                if FLAGS.contains(&name) {
                    options.push((name.to_owned(), String::new()));
                    continue;
                }
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

    /// Whether a [`FLAGS`] option was given.
    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(key, _)| key == name)
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
    // `sources mypg` lists only the relations whose name holds "mypg".
    let filter = match arguments.positional.as_slice() {
        [] => None,
        [filter] => Some(filter.to_lowercase()),
        _ => return Err("sources takes at most one filter, such as a database or schema name".into()),
    };
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
        if filter.as_ref().is_some_and(|filter| !name.to_lowercase().contains(filter)) {
            continue;
        }
        // `name TYPE` keeps a profile with many watched files readable;
        // `dashboard schema` shows a relation in full.
        let columns = connection
            .prepare(&format!("DESCRIBE {name}"))
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok(format!("{} {}", row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            });
        let kind = match relation.kind {
            crate::domain::documents::RelationKind::View => "view",
            _ => "table",
        };
        let mut entry = json!({"name": name, "kind": kind});
        match columns {
            Ok(columns) => entry["columns"] = json!(columns),
            Err(error) => entry["error"] = json!(error.to_string().lines().next().unwrap_or("")),
        }
        listed.push(entry);
    }
    let databases = connection
        .prepare(
            "SELECT database_name, type, path FROM duckdb_databases() \
             WHERE NOT internal AND database_name <> current_database() AND database_name <> ? ORDER BY 1",
        )
        .and_then(|mut statement| {
            statement
                .query_map([crate::infrastructure::source_database::FILES_CATALOG], |row| {
                    let kind = row.get::<_, String>(1)?;
                    let mut database = json!({"name": row.get::<_, String>(0)?, "type": kind});
                    // A file's path helps; a server's connection string can hold a password.
                    if matches!(kind.as_str(), "duckdb" | "sqlite") {
                        database["path"] = json!(row.get::<_, Option<String>>(2)?);
                    }
                    Ok(database)
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    let startup = repository.paths.config.join(crate::infrastructure::source_database::STARTUP_SCRIPT);
    let mut report = json!({
        "profile": repository.paths.config,
        "startupScript": startup,
        "attachedDatabases": databases,
        "note": STARTUP_NOTE,
        "relations": listed,
    });
    if let Some(error) = database.startup_error() {
        report["startupError"] = json!(error);
    }
    print_json(&report);
    Ok(0)
}

/// How an agent gets another database into the profile, printed with the sources.
const STARTUP_NOTE: &str = "Use a relation as a dashboard source with {\"profile\": \"<name>\"}. \
Watched files are views in the files schema. To add a database (Postgres, MySQL, SQLite, another DuckDB file), \
add its ATTACH to startupScript, which runs whenever the profile opens; ask the user first, since it changes their profile.";

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
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    print_json(&json!({
        "source": engine.sources[0],
        "columns": columns
            .rows
            .iter()
            .map(|row| format!("{} {}{}", text(&row[0]), text(&row[1]), if text(&row[2]) == "NO" { " NOT NULL" } else { "" }))
            .collect::<Vec<_>>(),
        "rowCount": count.rows.first().and_then(|row| row.first()),
        "sampleColumns": sample.columns.iter().map(|column| column.name.clone()).collect::<Vec<_>>(),
        "sample": sample.rows.iter().map(|row| row.iter().map(|value| shorten(value, SAMPLE_TEXT)).collect::<Vec<_>>()).collect::<Vec<_>>(),
    }));
    Ok(0)
}

/// The most characters of one text value a sample shows.
const SAMPLE_TEXT: usize = 160;

/// Text longer than `limit` characters, cut and marked with its full length;
/// other values as they are. A sample shows what a column holds, not all of it.
fn shorten(value: &Value, limit: usize) -> Value {
    match value {
        Value::String(text) if text.chars().count() > limit => {
            let cut = text.chars().take(limit).collect::<String>();
            Value::String(format!("{cut}… ({} characters)", text.chars().count()))
        }
        other => other.clone(),
    }
}

/// A value as one cell of TSV or a table: NULL for null, text without its
/// quotes, and tabs and line breaks escaped so each row stays one line.
fn cell(value: &Value) -> String {
    let text = match value {
        Value::Null => "NULL".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    text.replace('\\', "\\\\").replace('\t', "\\t").replace('\r', "\\r").replace('\n', "\\n")
}

/// A result as tab-separated values: a header line, then one line per row.
fn tsv(result: &crate::infrastructure::dashboard::QueryResult) -> String {
    let mut out = result.columns.iter().map(|column| cell(&Value::String(column.name.clone()))).collect::<Vec<_>>().join("\t");
    for row in &result.rows {
        out.push('\n');
        out.push_str(&row.iter().map(cell).collect::<Vec<_>>().join("\t"));
    }
    out
}

/// A result as an aligned text table, long values cut to keep it readable.
fn table(result: &crate::infrastructure::dashboard::QueryResult) -> String {
    const WIDTH: usize = 48;
    let fit = |text: String| {
        if text.chars().count() > WIDTH {
            format!("{}…", text.chars().take(WIDTH - 1).collect::<String>())
        } else {
            text
        }
    };
    let header = result.columns.iter().map(|column| fit(column.name.clone())).collect::<Vec<_>>();
    let rows = result.rows.iter().map(|row| row.iter().map(|value| fit(cell(value))).collect::<Vec<_>>()).collect::<Vec<_>>();
    let widths = (0..header.len())
        .map(|index| std::iter::once(&header[index]).chain(rows.iter().map(|row| &row[index])).map(|text| text.chars().count()).max().unwrap_or(0))
        .collect::<Vec<_>>();
    let numeric = result.columns.iter().map(|column| {
        matches!(
            column.data_type.split('(').next().unwrap_or("").trim(),
            "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "UBIGINT" | "UHUGEINT" | "FLOAT" | "DOUBLE" | "DECIMAL"
        )
    });
    let numeric = numeric.collect::<Vec<_>>();
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let pad = widths[index] - text.chars().count();
                if numeric[index] { format!("{}{text}", " ".repeat(pad)) } else { format!("{text}{}", " ".repeat(pad)) }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let mut out = vec![line(&header), widths.iter().map(|width| "-".repeat(*width)).collect::<Vec<_>>().join("  ")];
    out.extend(rows.iter().map(|row| line(row)));
    out.join("\n")
}

fn query(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&["source", "params", "limit", "format"])?;
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
    let format = arguments.option("format").unwrap_or("json");
    if !matches!(format, "json" | "tsv" | "table") {
        return Err(format!("--format is json, tsv or table, not \"{format}\""));
    }
    let result = engine.query(&request)?;
    if format == "json" {
        print_json(&serde_json::to_value(result).map_err(|error| error.to_string())?);
        return Ok(0);
    }
    println!("{}", if format == "tsv" { tsv(&result) } else { table(&result) });
    // The rows go to stdout alone, so they can be piped; the summary goes to stderr.
    eprintln!(
        "({} row{}, {} ms{})",
        result.rows.len(),
        if result.rows.len() == 1 { "" } else { "s" },
        result.elapsed_ms,
        if result.truncated { "; the row limit cut the result, pass --limit for more" } else { "" }
    );
    Ok(0)
}

fn check(arguments: &Arguments) -> Result<i32, String> {
    arguments.check_options(&["capture", "timeout", "size", "quiet", "run", "full-page"])?;
    let [dashboard] = arguments.positional.as_slice() else {
        return Err("check needs one .ddash file".into());
    };
    let full_page = arguments.flag("full-page");
    if full_page && arguments.option("capture").is_none() {
        return Err("--full-page sizes the --capture; add --capture shot.png".into());
    }
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
            scripts: arguments.all("run").map(str::to_owned).collect(),
            full_page,
        }),
        ..Default::default()
    };
    if let Some(size) = arguments.option("size") {
        let (width, height) = size.split_once(['x', 'X']).ok_or("--size needs WxH, such as 1400x900")?;
        options.width = width.parse().map_err(|_| "--size needs WxH, such as 1400x900")?;
        options.height = height.parse().map_err(|_| "--size needs WxH, such as 1400x900")?;
    }
    remove_stale_check_storage();
    crate::ui::platform::window::run::<crate::DataFrameView>(move |runtime| {
        crate::DataFrameView::open_another(runtime, options);
    });
    // Edge's processes hold the folder's files for about a second after the
    // window closes; waiting for them would slow every check, so a folder
    // this can't delete yet goes with the next check's sweep.
    let _ = std::fs::remove_dir_all(crate::ui::dataframe_view::dashboard::check_storage());
    crate::infrastructure::source_database::SourceDatabase::checkpoint_profiles()?;
    Ok(crate::ui::dataframe_view::dashboard::CHECK_EXIT.load(std::sync::atomic::Ordering::Relaxed))
}

/// Deletes the WebView2 folders of earlier checks whose process has ended,
/// leaving those of checks still running beside this one.
fn remove_stale_check_storage() {
    let prefix = crate::ui::dataframe_view::dashboard::CHECK_STORAGE_PREFIX;
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name.to_string_lossy().strip_prefix(prefix).and_then(|pid| pid.parse::<u32>().ok());
        if let Some(pid) = pid
            && !process_running(pid)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn process_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut code = 0u32;
    let running = unsafe { GetExitCodeProcess(process, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    let _ = unsafe { CloseHandle(process) };
    running
}

#[cfg(test)]
mod tests {
    #[test]
    fn rows_print_as_tsv_and_tables_one_line_each() {
        use crate::infrastructure::dashboard::{QueryResult, ResultColumn};
        use serde_json::json;
        let column = |name: &str, data_type: &str| ResultColumn { name: name.into(), data_type: data_type.into() };
        let result = QueryResult {
            columns: vec![column("name", "VARCHAR"), column("total", "DOUBLE")],
            rows: vec![vec![json!("a\tb\nc"), json!(1.5)], vec![json!(null), json!(12)]],
            truncated: false,
            elapsed_ms: 1.0,
        };
        assert_eq!(super::tsv(&result), "name\ttotal\na\\tb\\nc\t1.5\nNULL\t12");
        assert_eq!(super::table(&result), "name     total\n-------  -----\na\\tb\\nc    1.5\nNULL        12");
        assert_eq!(super::shorten(&json!("x".repeat(200)), 5), json!("xxxxx… (200 characters)"));
        assert_eq!(super::shorten(&json!(7), 5), json!(7));
    }

    #[test]
    fn stale_check_folders_are_those_of_ended_processes() {
        assert!(super::process_running(std::process::id()));
        assert!(!super::process_running(u32::MAX - 3));
    }

    #[test]
    fn help_sends_agents_to_the_dashboard_guide() {
        let help = super::help_text();
        assert!(help.contains("datadiver dashboard guide"));
        assert!(help.contains(env!("CARGO_PKG_VERSION")));
        assert!(super::AGENT_GUIDE.starts_with("# Building a DataDiver dashboard"));
    }
}
