//! Dashboards: an HTML page that DataDiver shows in a WebView and feeds with
//! query results.
//!
//! A dashboard is one `.ddash` file of HTML. A JSON manifest in it, in
//! `<script type="application/json" id="datadiver">`, names the data the
//! page may query: each source gets an alias, and the page's SQL reads the
//! aliases. The page itself holds no data; every number comes from a query
//! DuckDB runs on the profile database when the page asks.
//!
//! This module is the part without a window: reading the manifest, making a
//! view for each source, and running the page's read-only queries. The
//! window side is `ui::dataframe_view::dashboard`, and the command line's is
//! `dashboard_cli`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use duckdb::Connection;
use duckdb::types::Value as DuckValue;
use serde::Serialize;
use serde_json::{Map, Value};

use super::duckdb_store::quote_identifier;
use super::source_database::{SourceDatabase, source_reader};

/// The extension of dashboard files.
pub const EXTENSION: &str = "ddash";
/// The `id` of the manifest's script element.
pub const MANIFEST_ID: &str = "datadiver";
/// The rows a query returns unless it asks for another limit.
pub const DEFAULT_ROW_LIMIT: usize = 10_000;
/// The most rows one query may return.
pub const MAX_ROW_LIMIT: usize = 1_000_000;
/// The bridge's version, which pages can read as `datadiver.version`.
pub const BRIDGE_VERSION: u32 = 1;
/// Idle connections kept for the next queries.
const POOL_SIZE: usize = 4;
/// Integers JavaScript holds exactly: ±(2^53 − 1).
const SAFE_INTEGER: i128 = 9_007_199_254_740_991;

pub fn is_dashboard(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

/// Where a source's rows come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A data file, read the way the profile's file views read it. Relative
    /// paths start at the dashboard's folder; `~` is the home folder.
    File(String),
    /// A table or view in the profile database, such as a watched file's
    /// view `files.orders` or a table of a database the startup script
    /// attaches.
    Profile(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    pub alias: String,
    pub kind: SourceKind,
}

/// A command the dashboard adds to the command palette, with an optional key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// The key in DataDiver's keystroke form, such as `ctrl-shift-r`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub title: Option<String>,
    pub sources: Vec<SourceSpec>,
    pub commands: Vec<CommandSpec>,
}

/// The manifest's JSON in `html`, if the page has one.
pub fn manifest_json(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("<script") {
        let start = from + found;
        let tag_end = start + lower[start..].find('>')?;
        let tag = &lower[start..tag_end];
        from = tag_end + 1;
        if !has_attribute(tag, "id", MANIFEST_ID) {
            continue;
        }
        let close = from + lower[from..].find("</script")?;
        return Some(html[from..close].trim());
    }
    None
}

/// Whether the lowercased tag text has `name="value"`, quoted either way or not at all.
fn has_attribute(tag: &str, name: &str, value: &str) -> bool {
    [format!("{name}=\"{value}\""), format!("{name}='{value}'"), format!("{name}={value}")]
        .iter()
        .any(|attribute| {
            tag.match_indices(attribute.as_str()).any(|(at, _)| {
                let before = tag[..at].chars().last();
                let after = tag[at + attribute.len()..].chars().next();
                before.is_some_and(char::is_whitespace)
                    && after.is_none_or(|c| c.is_whitespace() || c == '/' || attribute.ends_with(['"', '\'']))
            })
        })
}

impl Manifest {
    /// Reads the manifest of a dashboard's HTML. A page without one is a
    /// dashboard with no sources.
    pub fn from_html(html: &str) -> Result<Self, String> {
        match manifest_json(html) {
            Some(json) => Self::parse(json),
            None => Ok(Self {
                version: 1,
                ..Self::default()
            }),
        }
    }

    pub fn parse(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|error| format!("The manifest is not valid JSON: {error}"))?;
        let Value::Object(object) = value else {
            return Err("The manifest must be a JSON object".into());
        };
        let mut manifest = Self {
            version: 1,
            ..Self::default()
        };
        for (key, value) in object {
            match key.as_str() {
                "version" => {
                    manifest.version = value
                        .as_u64()
                        .and_then(|version| u32::try_from(version).ok())
                        .ok_or("\"version\" must be a whole number")?;
                    if manifest.version > BRIDGE_VERSION {
                        return Err(format!(
                            "This dashboard needs manifest version {}; this DataDiver reads version {BRIDGE_VERSION}",
                            manifest.version
                        ));
                    }
                }
                "title" => manifest.title = Some(value.as_str().ok_or("\"title\" must be text")?.to_owned()),
                "sources" => manifest.sources = parse_sources(value)?,
                "commands" => manifest.commands = parse_commands(value)?,
                // Room for what later versions add, and for the page's own notes.
                _ if key.starts_with('$') || key.starts_with('_') => {}
                other => return Err(format!("The manifest has an unknown field \"{other}\"")),
            }
        }
        Ok(manifest)
    }
}

fn parse_sources(value: Value) -> Result<Vec<SourceSpec>, String> {
    let Value::Object(sources) = value else {
        return Err("\"sources\" must be an object of alias: source".into());
    };
    let mut specs: Vec<SourceSpec> = Vec::new();
    for (alias, source) in sources {
        if !valid_alias(&alias) {
            return Err(format!(
                "The source alias \"{alias}\" must start with a letter or _ and hold only letters, digits and _"
            ));
        }
        if specs.iter().any(|spec| spec.alias.eq_ignore_ascii_case(&alias)) {
            return Err(format!("The source alias \"{alias}\" appears twice (aliases ignore case)"));
        }
        let kind = match source {
            Value::String(path) => SourceKind::File(path),
            Value::Object(object) => source_kind(&alias, object)?,
            _ => return Err(format!("The source \"{alias}\" must be a path or an object")),
        };
        specs.push(SourceSpec { alias, kind });
    }
    Ok(specs)
}

fn source_kind(alias: &str, object: Map<String, Value>) -> Result<SourceKind, String> {
    let mut kind = None;
    for (key, value) in object {
        let text = || {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("\"{key}\" of the source \"{alias}\" must be text"))
        };
        let next = match key.as_str() {
            "file" => SourceKind::File(text()?),
            "profile" => SourceKind::Profile(text()?),
            _ if key.starts_with('$') || key.starts_with('_') => continue,
            other => return Err(format!("The source \"{alias}\" has an unknown field \"{other}\"")),
        };
        if kind.replace(next).is_some() {
            return Err(format!("The source \"{alias}\" must have one of \"file\" or \"profile\", not both"));
        }
    }
    kind.ok_or_else(|| format!("The source \"{alias}\" needs \"file\" or \"profile\""))
}

fn valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_commands(value: Value) -> Result<Vec<CommandSpec>, String> {
    let Value::Array(commands) = value else {
        return Err("\"commands\" must be a list".into());
    };
    let mut specs: Vec<CommandSpec> = Vec::new();
    for command in commands {
        let Value::Object(object) = command else {
            return Err("Each command must be an object with \"id\" and \"title\"".into());
        };
        let text = |key: &str| -> Result<Option<String>, String> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(format!("A command's \"{key}\" must be text")),
            }
        };
        let id = text("id")?.filter(|id| !id.is_empty()).ok_or("Each command needs an \"id\"")?;
        let title = text("title")?.unwrap_or_else(|| id.clone());
        let key = match text("key")? {
            Some(key) => Some(dashboard_keystroke(&key).map_err(|error| format!("The key of command \"{id}\": {error}"))?),
            None => None,
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !matches!(key.as_str(), "id" | "title" | "key") && !key.starts_with(['$', '_']))
        {
            return Err(format!("The command \"{id}\" has an unknown field \"{unknown}\""));
        }
        if specs.iter().any(|spec| spec.id == id) {
            return Err(format!("The command id \"{id}\" appears twice"));
        }
        if let Some(key) = &key
            && let Some(other) = specs.iter().find(|spec| spec.key.as_ref() == Some(key))
        {
            return Err(format!("The commands \"{}\" and \"{id}\" both use {key}", other.id));
        }
        specs.push(CommandSpec { id, title, key });
    }
    Ok(specs)
}

/// A key as DataDiver writes it, from `ctrl+shift+r` or `Ctrl-Shift-R`.
pub fn dashboard_keystroke(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".into());
    }
    // `+` joins keys in VS Code's notation; a lone `+` or a trailing `++` is the plus key.
    let normalized = if key.contains('+') && key != "+" {
        let (rest, plus) = match key.strip_suffix("++") {
            Some(rest) => (rest, true),
            None => (key, false),
        };
        let mut parts = rest.split('+').map(str::to_owned).collect::<Vec<_>>();
        if plus {
            parts.push("+".into());
        }
        parts.join("-")
    } else {
        key.to_owned()
    };
    crate::application::keymap::canonical_keystroke(&normalized.to_lowercase())
}

/// Where a source reads from, resolved for the dashboard at `dashboard`.
pub fn resolve_file(path: &str, dashboard: &Path) -> PathBuf {
    let home = || std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let resolved = if path == "~" {
        home().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        match home() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        }
    } else {
        let path = PathBuf::from(path);
        if path.is_absolute() || path.has_root() {
            path
        } else {
            dashboard.parent().unwrap_or(Path::new(".")).join(path)
        }
    };
    // `.\data\x.csv` reads better as `data\x.csv`.
    let mut clean = PathBuf::new();
    for part in resolved.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if clean.file_name().is_some() => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What a source of the open dashboard turned out to be.
#[derive(Clone, Debug, Serialize)]
pub struct SourceStatus {
    pub alias: String,
    pub kind: &'static str,
    /// The resolved file path or the profile relation the alias reads.
    pub location: String,
    /// Why the source cannot be read; its alias is then missing from queries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs a dashboard's queries on the profile database, with a view for each
/// source on every connection it uses.
pub struct DashboardEngine {
    database: Arc<SourceDatabase>,
    /// Makes the source views on a new connection.
    setup: String,
    pool: Mutex<Vec<Connection>>,
    pub sources: Vec<SourceStatus>,
}

impl DashboardEngine {
    /// Resolves each source and checks that it can be read. Opening the
    /// profile database runs its startup script, so this may block.
    pub fn open(database: Arc<SourceDatabase>, manifest: &Manifest, dashboard: &Path) -> Result<Self, String> {
        database.initialize()?;
        let connection = database.connect()?;
        let mut setup = String::new();
        let mut sources = Vec::new();
        for spec in &manifest.sources {
            let (kind, location, select) = match &spec.kind {
                SourceKind::File(path) => {
                    let resolved = resolve_file(path, dashboard);
                    let location = resolved.to_string_lossy().into_owned();
                    let select = source_reader(&resolved).map(|reader| reader.replacen('?', &sql_string(&location), 1));
                    ("file", location, select)
                }
                SourceKind::Profile(name) => ("profile", name.clone(), Ok(name.clone())),
            };
            let view = format!(
                "CREATE OR REPLACE TEMP VIEW {} AS SELECT * FROM {};\n",
                quote_identifier(&spec.alias),
                select.as_deref().unwrap_or("")
            );
            let error = match select {
                Err(error) => Some(error),
                Ok(_) => connection.execute_batch(&view).err().map(|error| source_error(&spec.kind, &error.to_string())),
            };
            if error.is_none() {
                setup.push_str(&view);
            }
            sources.push(SourceStatus {
                alias: spec.alias.clone(),
                kind,
                location,
                error,
            });
        }
        Ok(Self {
            database,
            setup,
            pool: Mutex::new(vec![connection]),
            sources,
        })
    }

    pub fn source_errors(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter_map(|source| source.error.as_ref().map(|error| format!("{}: {error}", source.alias)))
            .collect()
    }

    fn connection(&self) -> Result<Connection, String> {
        if let Some(connection) = self.pool.lock().map_err(|_| "dashboard connections poisoned")?.pop() {
            return Ok(connection);
        }
        let connection = self.database.connect()?;
        connection
            .execute_batch(&self.setup)
            .map_err(|error| format!("Cannot make the dashboard's source views: {error}"))?;
        Ok(connection)
    }

    fn release(&self, connection: Connection) {
        if let Ok(mut pool) = self.pool.lock()
            && pool.len() < POOL_SIZE
        {
            pool.push(connection);
        }
    }

    /// Runs one read-only query from the page.
    pub fn query(&self, request: &QueryRequest) -> Result<QueryResult, String> {
        let started = Instant::now();
        let limit = request.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROW_LIMIT);
        let sql = read_only_query(&request.sql)?;
        let (sql, binds) = bind_parameters(sql, request.params.as_ref())?;
        let connection = self.connection()?;
        let result = run_query(&connection, &sql, &binds, limit);
        // A failed query can leave its read-only transaction open.
        if connection.is_autocommit() {
            self.release(connection);
        }
        let mut result = result?;
        result.elapsed_ms = (started.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
        Ok(result)
    }
}

/// Explains a source that cannot be read in the dashboard's terms.
fn source_error(kind: &SourceKind, error: &str) -> String {
    let error = error.lines().next().unwrap_or(error);
    match kind {
        SourceKind::Profile(name) if error.contains("does not exist") => format!(
            "{name} is not in the profile database. Watched files are views in the files schema, such as files.orders; \
             databases the startup script attaches use their own name. {error}"
        ),
        _ => error.to_owned(),
    }
}

/// A query the page sends.
#[derive(Clone, Debug, Default)]
pub struct QueryRequest {
    pub sql: String,
    /// `{name: value}` for `$name` parameters, or `[value, …]` for `?` and `$1`.
    pub params: Option<Value>,
    pub limit: Option<usize>,
}

impl QueryRequest {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ResultColumn {
    pub name: String,
    /// The DuckDB type, such as `BIGINT` or `TIMESTAMP WITH TIME ZONE`.
    #[serde(rename = "type")]
    pub data_type: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub columns: Vec<ResultColumn>,
    /// Each row's values in column order.
    pub rows: Vec<Vec<Value>>,
    /// The query had more rows than the limit; `rows` stops there.
    pub truncated: bool,
    pub elapsed_ms: f64,
}

/// Statements a dashboard may run: the ones that only read.
const READ_KEYWORDS: &[&str] = &["select", "with", "from", "values", "table", "pivot", "unpivot", "summarize", "describe"];

/// The one statement of `sql`, if it only reads.
fn read_only_query(sql: &str) -> Result<&str, String> {
    let statements = diver_sql::statements(sql);
    let [statement] = statements.as_slice() else {
        return Err(if statements.is_empty() {
            "The query is empty".into()
        } else {
            "Send one statement per query".into()
        });
    };
    let text = &sql[std::ops::Range::<usize>::from(*statement)];
    let first = first_word(text).unwrap_or_default().to_ascii_lowercase();
    if !READ_KEYWORDS.contains(&first.as_str()) {
        return Err(format!(
            "Dashboards run read-only queries; {} is not allowed. Start with SELECT, WITH, FROM, VALUES, PIVOT, SUMMARIZE or DESCRIBE.",
            if first.is_empty() { "this statement".to_owned() } else { first.to_uppercase() }
        ));
    }
    Ok(text)
}

/// The first word of `sql` after comments, whitespace and opening parentheses.
fn first_word(sql: &str) -> Option<&str> {
    use diver_sql::SyntaxKind as K;
    let mut at = 0usize;
    for token in diver_sql::tokenize(sql) {
        let len = usize::from(token.len);
        let range = at..at + len;
        at += len;
        match token.kind {
            K::WHITESPACE | K::LINE_COMMENT | K::BLOCK_COMMENT | K::L_PAREN => continue,
            K::IDENT => return Some(&sql[range]),
            _ => return None,
        }
    }
    None
}

/// A value to bind to one parameter.
#[derive(Clone, Debug, PartialEq)]
enum Bind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl duckdb::ToSql for Bind {
    fn to_sql(&self) -> duckdb::Result<duckdb::types::ToSqlOutput<'_>> {
        use duckdb::types::{ToSqlOutput, ValueRef};
        Ok(match self {
            Bind::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            Bind::Bool(value) => ToSqlOutput::Borrowed(ValueRef::Boolean(*value)),
            Bind::Int(value) => ToSqlOutput::Borrowed(ValueRef::BigInt(*value)),
            Bind::Float(value) => ToSqlOutput::Borrowed(ValueRef::Double(*value)),
            Bind::Text(value) => ToSqlOutput::Borrowed(ValueRef::Text(value.as_bytes())),
        })
    }
}

fn scalar_bind(value: &Value, name: &str) -> Result<Bind, String> {
    Ok(match value {
        Value::Null => Bind::Null,
        Value::Bool(value) => Bind::Bool(*value),
        Value::Number(number) => match number.as_i64() {
            Some(value) => Bind::Int(value),
            None => Bind::Float(number.as_f64().ok_or_else(|| format!("The parameter {name} is not a usable number"))?),
        },
        Value::String(text) => Bind::Text(text.clone()),
        Value::Array(_) | Value::Object(_) => {
            return Err(format!("The parameter {name} holds a list inside a list or an object; send text, numbers, true/false, null or a list of those"));
        }
    })
}

/// A list parameter as a DuckDB list literal, such as `['a', 'b']`.
fn list_literal(values: &[Value], name: &str) -> Result<String, String> {
    let items = values
        .iter()
        .map(|value| {
            Ok(match scalar_bind(value, name)? {
                Bind::Null => "NULL".to_owned(),
                Bind::Bool(value) => if value { "TRUE" } else { "FALSE" }.to_owned(),
                Bind::Int(value) => value.to_string(),
                Bind::Float(value) if value.is_finite() => format!("{value:?}"),
                Bind::Float(_) => return Err(format!("The parameter {name} holds a number JSON cannot")),
                Bind::Text(text) => sql_string(&text),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

/// Writes list parameters into the SQL as list literals, since DuckDB binds
/// only single values, and returns the values to bind in placeholder order.
fn bind_parameters(sql: &str, params: Option<&Value>) -> Result<(String, Vec<Bind>), String> {
    use diver_sql::SyntaxKind as K;
    let mut out = String::with_capacity(sql.len());
    let mut binds = Vec::new();
    let mut at = 0usize;
    let mut next_positional = 0usize;
    for token in diver_sql::tokenize(sql) {
        let len = usize::from(token.len);
        let text = &sql[at..at + len];
        at += len;
        if token.kind != K::PARAM {
            out.push_str(text);
            continue;
        }
        let (value, label) = match (text, params) {
            ("?", Some(Value::Array(values))) => {
                next_positional += 1;
                (values.get(next_positional - 1), format!("#{next_positional}"))
            }
            (_, Some(Value::Array(values))) if text[1..].chars().all(|c| c.is_ascii_digit()) => {
                let index = text[1..].parse::<usize>().unwrap_or(0);
                (index.checked_sub(1).and_then(|index| values.get(index)), text.to_owned())
            }
            (_, Some(Value::Object(values))) if text.len() > 1 => (values.get(&text[1..]), text.to_owned()),
            _ => (None, text.to_owned()),
        };
        let value = value.ok_or_else(|| match params {
            None => format!("The query uses {label} but sends no parameters"),
            Some(_) => format!("The query uses {label}, which the parameters do not have"),
        })?;
        match value {
            Value::Array(values) => out.push_str(&list_literal(values, &label)?),
            value => {
                // Every placeholder becomes `?`, bound in order, so a `$name`
                // used twice binds twice.
                out.push('?');
                binds.push(scalar_bind(value, &label)?);
            }
        }
    }
    Ok((out, binds))
}

fn run_query(connection: &Connection, sql: &str, binds: &[Bind], limit: usize) -> Result<QueryResult, String> {
    let error = |error: duckdb::Error| error.to_string();
    connection.execute_batch("BEGIN TRANSACTION READ ONLY").map_err(error)?;
    let result = (|| {
        let params = duckdb::params_from_iter(binds.iter());
        // DESCRIBE names the columns and their DuckDB types without running the query.
        let mut describe = connection.prepare(&format!("DESCRIBE SELECT * FROM ({sql}\n)")).map_err(error)?;
        let columns = describe
            .query_map(params, |row| {
                Ok(ResultColumn {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                })
            })
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        if columns.is_empty() {
            return Ok(QueryResult {
                columns,
                rows: Vec::new(),
                truncated: false,
                elapsed_ms: 0.0,
            });
        }
        let aliases = (0..columns.len()).map(|index| format!("c{index}")).collect::<Vec<_>>();
        let projection = columns
            .iter()
            .zip(&aliases)
            .map(|(column, alias)| match value_kind(&column.data_type) {
                ValueKind::Native => alias.clone(),
                ValueKind::Text => format!("CAST({alias} AS VARCHAR)"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let wrapped = format!(
            "SELECT {projection} FROM ({sql}\n) AS __datadiver_query({}) LIMIT {}",
            aliases.join(", "),
            limit + 1
        );
        let mut statement = connection.prepare(&wrapped).map_err(error)?;
        let mut rows = statement.query(duckdb::params_from_iter(binds.iter())).map_err(error)?;
        let mut out = Vec::new();
        let mut truncated = false;
        while let Some(row) = rows.next().map_err(error)? {
            if out.len() == limit {
                truncated = true;
                break;
            }
            let values = (0..columns.len())
                .map(|index| row.get::<_, DuckValue>(index).map(json_value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            out.push(values);
        }
        Ok(QueryResult {
            columns,
            rows: out,
            truncated,
            elapsed_ms: 0.0,
        })
    })();
    let _ = connection.execute_batch("ROLLBACK");
    result
}

enum ValueKind {
    /// Sent as DuckDB returns it: numbers, true/false and text.
    Native,
    /// Sent as DuckDB writes it as text: dates, times, lists, and the rest.
    Text,
}

fn value_kind(data_type: &str) -> ValueKind {
    let base = data_type.split('(').next().unwrap_or(data_type).trim().to_ascii_uppercase();
    match base.as_str() {
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT"
        | "UINTEGER" | "UBIGINT" | "UHUGEINT" | "FLOAT" | "DOUBLE" | "DECIMAL" | "VARCHAR" => ValueKind::Native,
        _ => ValueKind::Text,
    }
}

/// A DuckDB value as JSON. Numbers JavaScript would round, such as large
/// integers and long decimals, are sent as text so nothing changes silently.
fn json_value(value: DuckValue) -> Value {
    let integer = |value: i128| {
        if (-SAFE_INTEGER..=SAFE_INTEGER).contains(&value) {
            Value::Number((value as i64).into())
        } else {
            Value::String(value.to_string())
        }
    };
    let float = |value: f64| match serde_json::Number::from_f64(value) {
        Some(number) => Value::Number(number),
        None => Value::String(if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }),
    };
    match value {
        DuckValue::Null => Value::Null,
        DuckValue::Boolean(value) => Value::Bool(value),
        DuckValue::TinyInt(value) => integer(value.into()),
        DuckValue::SmallInt(value) => integer(value.into()),
        DuckValue::Int(value) => integer(value.into()),
        DuckValue::BigInt(value) => integer(value.into()),
        DuckValue::HugeInt(value) => integer(value),
        DuckValue::UTinyInt(value) => integer(value.into()),
        DuckValue::USmallInt(value) => integer(value.into()),
        DuckValue::UInt(value) => integer(value.into()),
        DuckValue::UBigInt(value) => integer(value.into()),
        DuckValue::UHugeInt(value) => i128::try_from(value).map(integer).unwrap_or_else(|_| Value::String(value.to_string())),
        DuckValue::Float(value) => float(f64::from(value)),
        DuckValue::Double(value) => float(value),
        DuckValue::Decimal(decimal) => {
            let text = decimal.to_string();
            let digits = text.chars().filter(char::is_ascii_digit).count();
            match text.parse::<serde_json::Number>() {
                // A double holds 15 significant digits exactly.
                Ok(number) if digits <= 15 => Value::Number(number),
                _ => Value::String(text),
            }
        }
        DuckValue::Text(text) => Value::String(text),
        DuckValue::Enum(text) => Value::String(text),
        // The rest were cast to text in SQL; these are left for safety.
        other => Value::String(format!("{other:?}")),
    }
}

/// The watched files and the files in watched folders.
pub fn watched_files(settings: &super::json_store::Settings) -> Vec<PathBuf> {
    let folders = settings.watched_folders.iter().map(PathBuf::from).collect::<Vec<_>>();
    let mut files = super::catalog::scan_sources(&folders);
    files.extend(settings.watched_files.iter().map(PathBuf::from));
    files
}

/// Brings the views of the watched files up to date, so a profile source
/// such as `files.orders` finds the file as it is watched now.
pub fn prepare_profile(database: &SourceDatabase, repository: &super::json_store::ViewRepository) -> Result<(), String> {
    let settings = repository.load_settings().map_err(|error| format!("Cannot read settings.json: {error}"))?;
    if settings.file_views {
        // A file that cannot get a view shows up as that source's error.
        database.sync_file_views(&watched_files(&settings))?;
    }
    Ok(())
}

/// Reads a dashboard file and its manifest.
pub fn read_dashboard(path: &Path) -> Result<(String, Manifest), String> {
    let html = std::fs::read_to_string(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let manifest = Manifest::from_html(&html).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((html, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(home: &Path, manifest: &str) -> DashboardEngine {
        let database = SourceDatabase::for_profile(home);
        let manifest = Manifest::parse(manifest).unwrap();
        DashboardEngine::open(database, &manifest, &home.join("board.ddash")).unwrap()
    }

    #[test]
    fn reads_the_manifest_from_the_page() {
        let html = r#"<!doctype html><html><head>
            <script src="x.js"></script>
            <script type="application/json" id="datadiver">
              {"title": "Sales", "sources": {"orders": "data/orders.csv", "people": {"profile": "files.people"}},
               "commands": [{"id": "reset", "title": "Reset filters", "key": "Ctrl+Shift+R"}]}
            </script></head></html>"#;
        let manifest = Manifest::from_html(html).unwrap();
        assert_eq!(manifest.title.as_deref(), Some("Sales"));
        assert_eq!(
            manifest.sources,
            vec![
                SourceSpec { alias: "orders".into(), kind: SourceKind::File("data/orders.csv".into()) },
                SourceSpec { alias: "people".into(), kind: SourceKind::Profile("files.people".into()) },
            ]
        );
        assert_eq!(manifest.commands[0].key.as_deref(), Some("ctrl-shift-r"));
        assert_eq!(Manifest::from_html("<html></html>").unwrap().sources, Vec::new());
    }

    #[test]
    fn rejects_manifests_that_would_go_wrong_later() {
        for (json, message) in [
            (r#"{"sources": {"1x": "a.csv"}}"#, "must start with a letter"),
            (r#"{"sources": {"a": "a.csv", "A": "b.csv"}}"#, "appears twice"),
            (r#"{"sources": {"a": {"file": "a.csv", "profile": "x"}}}"#, "not both"),
            (r#"{"sources": {"a": {"path": "a.csv"}}}"#, "unknown field"),
            (r#"{"sourcse": {}}"#, "unknown field \"sourcse\""),
            (r#"{"version": 9}"#, "needs manifest version 9"),
            (r#"{"commands": [{"id": "a", "key": "ctrl+x"}, {"id": "b", "key": "Ctrl-X"}]}"#, "both use"),
        ] {
            let error = Manifest::parse(json).unwrap_err();
            assert!(error.contains(message), "{json}: {error}");
        }
        assert!(Manifest::parse(r#"{"_notes": "kept for the author", "sources": {}}"#).is_ok());
    }

    #[test]
    fn relative_paths_start_at_the_dashboard() {
        let dashboard = Path::new(r"C:\boards\sales.ddash");
        assert_eq!(resolve_file("data/orders.csv", dashboard), PathBuf::from(r"C:\boards\data\orders.csv"));
        assert_eq!(resolve_file(r".\orders.csv", dashboard), PathBuf::from(r"C:\boards\orders.csv"));
        assert_eq!(resolve_file("../shared/x.parquet", dashboard), PathBuf::from(r"C:\shared\x.parquet"));
        assert_eq!(resolve_file(r"D:\x.csv", dashboard), PathBuf::from(r"D:\x.csv"));
        assert_eq!(resolve_file(r"\\server\share\x.csv", dashboard), PathBuf::from(r"\\server\share\x.csv"));
    }

    #[test]
    fn queries_read_sources_by_alias_with_parameters() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("sales.csv"), "region,amount,day\nnorth,1.5,2024-01-02\nsouth,2,2024-01-03\nnorth,4,2024-01-04\n").unwrap();
        let engine = engine(home.path(), r#"{"sources": {"sales": "sales.csv", "missing": "nope.csv"}}"#);
        assert!(engine.sources[0].error.is_none());
        assert!(engine.sources[1].error.is_some());

        let mut request = QueryRequest::new("select region, sum(amount) as total, min(day) as first from sales where region = $region group by all");
        request.params = Some(serde_json::json!({"region": "north"}));
        let result = engine.query(&request).unwrap();
        assert_eq!(result.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["region", "total", "first"]);
        assert_eq!(result.columns[2].data_type, "DATE");
        assert_eq!(result.rows, vec![vec![serde_json::json!("north"), serde_json::json!(5.5), serde_json::json!("2024-01-02")]]);

        let mut request = QueryRequest::new("select count(*) as n from sales where region in (select unnest($regions))");
        request.params = Some(serde_json::json!({"regions": ["north", "it's"]}));
        assert_eq!(engine.query(&request).unwrap().rows, vec![vec![serde_json::json!(2)]]);

        let mut request = QueryRequest::new("select * from sales order by amount");
        request.limit = Some(2);
        let result = engine.query(&request).unwrap();
        assert!(result.truncated);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][1], serde_json::json!(1.5));
    }

    #[test]
    fn large_numbers_arrive_unrounded() {
        let home = tempfile::tempdir().unwrap();
        let engine = engine(home.path(), "{}");
        let result = engine
            .query(&QueryRequest::new(
                "select 9007199254740993::bigint as big, 12.34::decimal(10,2) as money, 1234567890123456.78::decimal(18,2) as long, 'nan'::double as nan",
            ))
            .unwrap();
        assert_eq!(
            result.rows[0],
            vec![
                serde_json::json!("9007199254740993"),
                serde_json::json!(12.34),
                serde_json::json!("1234567890123456.78"),
                serde_json::json!("NaN")
            ]
        );
    }

    #[test]
    fn only_read_only_statements_run() {
        let home = tempfile::tempdir().unwrap();
        let engine = engine(home.path(), "{}");
        for sql in [
            "create table t (a int)",
            "select 1; drop table x",
            "copy (select 1) to 'x.csv'",
            "attach 'x.duckdb'",
            "install httpfs",
            "set threads = 1",
            "",
        ] {
            assert!(engine.query(&QueryRequest::new(sql)).is_err(), "{sql}");
        }
        for sql in [
            "-- the total\n(select 42 as answer)",
            "with t as (select 1 as a) select * from t",
            "from (values (1), (2)) v(a)",
            "summarize select 1 as a",
            "describe select 1 as a",
            "pivot (values ('a', 1), ('b', 2)) v(k, n) on k using sum(n)",
            "select 1 as a -- a trailing comment",
        ] {
            assert!(engine.query(&QueryRequest::new(sql)).is_ok(), "{sql}");
        }
    }

    #[test]
    fn failed_queries_leave_the_engine_usable() {
        let home = tempfile::tempdir().unwrap();
        let engine = engine(home.path(), "{}");
        for _ in 0..3 {
            assert!(engine.query(&QueryRequest::new("select * from nowhere")).is_err());
            assert_eq!(engine.query(&QueryRequest::new("select 1 as one")).unwrap().rows, vec![vec![serde_json::json!(1)]]);
        }
    }

    #[test]
    fn profile_sources_read_the_profile_database() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        database.connect().unwrap().execute_batch("CREATE SCHEMA files; CREATE TABLE files.people AS SELECT 'ann' AS name").unwrap();
        let engine = engine(home.path(), r#"{"sources": {"people": {"profile": "files.people"}, "gone": {"profile": "files.gone"}}}"#);
        assert_eq!(engine.query(&QueryRequest::new("from people")).unwrap().rows, vec![vec![serde_json::json!("ann")]]);
        assert!(engine.sources[1].error.as_deref().unwrap().contains("not in the profile database"));
    }
}
