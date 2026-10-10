//! AI provider settings and the prompts DataDiver sends to an
//! OpenAI-compatible Chat Completions endpoint.

use serde::{Deserialize, Serialize};

use crate::domain::sql_editor::{may_mutate, statements};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Name of the provider used by default; the first one when unset or missing.
    pub default_provider: Option<String>,
    pub providers: Vec<AiProvider>,
}

impl AiSettings {
    pub fn active(&self) -> Option<&AiProvider> {
        self.default_provider
            .as_ref()
            .and_then(|name| self.providers.iter().find(|p| &p.name == name))
            .or_else(|| self.providers.first())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiProvider {
    pub name: String,
    /// The API root, such as `https://openrouter.ai/api/v1`. A URL that already
    /// ends in `/chat/completions` is used as is.
    pub base_url: String,
    /// The key itself, or `{env:NAME}` to read it from an environment variable.
    /// Empty sends no `Authorization` header (local servers such as Ollama).
    pub api_key: String,
    pub model: String,
    /// Extra request headers, such as `OpenAI-Organization`, sent in order.
    pub headers: Vec<AiHeader>,
    /// Opt-in: tables named with `@` also send min, max, and frequent values.
    /// Off, only names, types, comments, counts, and the editor's SQL go.
    pub send_data: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiHeader {
    pub name: String,
    pub value: String,
}

impl AiProvider {
    pub fn chat_url(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_owned()
        } else {
            format!("{base}/chat/completions")
        }
    }

    /// The key to send, or `None` for no `Authorization` header.
    pub fn resolve_api_key(&self) -> Result<Option<String>, String> {
        let key = self.api_key.trim();
        if key.is_empty() {
            return Ok(None);
        }
        match env_reference(key) {
            Some(name) => std::env::var(name)
                .map(Some)
                .map_err(|_| format!("environment variable {name} is not set")),
            None => Ok(Some(key.to_owned())),
        }
    }

    pub fn validate(&self, others: &[AiProvider], editing: Option<&str>) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("provider name is required".into());
        }
        if others
            .iter()
            .any(|other| other.name == name && Some(other.name.as_str()) != editing)
        {
            return Err(format!("a provider named {name} already exists"));
        }
        let url = self.base_url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("base URL must start with http:// or https://".into());
        }
        if self.model.trim().is_empty() {
            return Err("model is required".into());
        }
        for header in &self.headers {
            let name = header.name.trim();
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic() && b != b':') {
                return Err(format!("header name {name:?} is not valid"));
            }
        }
        Ok(())
    }
}

/// `{env:NAME}`, as OpenCode writes it, or `$NAME`.
fn env_reference(key: &str) -> Option<&str> {
    key.strip_prefix("{env:")
        .and_then(|rest| rest.strip_suffix('}'))
        .or_else(|| key.strip_prefix('$'))
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// A relation the model may query, with typed columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaRelation {
    /// The name as SQL should write it, which `@` mentions also use.
    pub name: String,
    pub is_view: bool,
    /// `COMMENT ON` text from the database.
    pub comment: Option<String>,
    pub columns: Vec<SchemaColumn>,
    /// Statistics, only for relations the user names with `@`.
    pub profile: Option<RelationProfile>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaColumn {
    pub name: String,
    pub kind: String,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelationProfile {
    pub rows: u64,
    /// One per column, in column order.
    pub columns: Vec<ColumnProfile>,
}

/// Counts are always sent; values only when the provider allows data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnProfile {
    pub nulls: u64,
    /// Distinct non-null values; exact when `all_values`, else an estimate.
    pub unique: u64,
    pub min: Option<String>,
    pub max: Option<String>,
    /// Values with their row counts, most frequent first: every value when
    /// `all_values`, otherwise the most frequent few.
    pub values: Vec<(Option<String>, u64)>,
    pub all_values: bool,
}

/// A column with at most this many distinct values has all of them listed.
pub const ALL_VALUES_LIMIT: u64 = 25;
/// Most frequent values listed for other text columns.
pub const TOP_VALUES: usize = 5;

/// The `@name` being typed at `cursor`, for completion: the range covers the
/// `@` through the end of the name, and `typed` is the name without quotes.
pub fn mention_query(text: &str, cursor: usize) -> Option<crate::domain::query::ColumnNameQuery> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let at = text[..cursor].rfind('@')?;
    if text[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
    {
        return None;
    }
    let segment = &text[at + 1..cursor];
    // Spaces end a name unless a quoted part is still open.
    let open_quote = segment.matches('"').count() % 2 == 1;
    if segment.contains(char::is_whitespace) && !open_quote {
        return None;
    }
    let rest = &text[cursor..];
    let end = cursor
        + rest
            .find(|c: char| c.is_whitespace() && !open_quote || c == '@')
            .unwrap_or(rest.len());
    Some(crate::domain::query::ColumnNameQuery {
        typed: segment.replace('"', ""),
        range: at..end,
    })
}

/// The relations `prompt` names as `@name`, by index, in catalog order.
pub fn mentioned_relations(prompt: &str, relations: &[SchemaRelation]) -> Vec<usize> {
    let prompt = prompt.to_lowercase();
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '"');
    // A `.` continues the name only before another part, not ending a sentence.
    let continues = |rest: &str| {
        let mut chars = rest.chars();
        match chars.next() {
            Some('.') => chars.next().is_some_and(name_char),
            next => next.is_some_and(name_char),
        }
    };
    relations
        .iter()
        .enumerate()
        .filter(|(_, relation)| {
            let needle = format!("@{}", relation.name.to_lowercase());
            prompt.match_indices(&needle).any(|(at, _)| {
                let before = prompt[..at].chars().next_back();
                !before.is_some_and(char::is_alphanumeric)
                    && !continues(&prompt[at + needle.len()..])
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// Everything the model is told when asked for SQL.
#[derive(Clone, Debug, Default)]
pub struct SqlRequest {
    pub prompt: String,
    pub relations: Vec<SchemaRelation>,
    /// Columns of the result the view shows now.
    pub result_columns: Vec<(String, String)>,
    pub editor_text: String,
    /// SQL the user selected to be rewritten.
    pub selection: Option<String>,
}

/// Bounds the schema part of a prompt; the rest of the relations are listed by name.
const SCHEMA_BUDGET: usize = 60_000;
const EDITOR_BUDGET: usize = 8_000;
const VALUE_CHARS: usize = 60;

const SQL_SYSTEM_PROMPT: &str = "You write SQL for DuckDB inside DataDiver, a data exploration app.
Rules:
- Use DuckDB's SQL dialect and functions.
- Use only the relations and columns listed below. Qualify a relation the way it is listed when its name is ambiguous.
- Quote identifiers with double quotes when they contain spaces, punctuation, or capital letters.
- Text in [brackets] is the database's comment on a relation or column.
- Relations the user names as @name are described with row counts and column statistics (~ marks an estimate). Where values are listed, filter on them exactly as written.
- Write one statement unless the request clearly needs several.
- Reply with the SQL only, in a single ```sql code block, with no explanation. Comments inside the SQL are fine.";

pub fn sql_messages(request: &SqlRequest) -> Vec<ChatMessage> {
    let mut context = String::new();
    context.push_str(&describe_relations(&request.relations));
    if !request.result_columns.is_empty() {
        context.push_str("\nThe current result (shown in the grid) has columns: ");
        let columns = request
            .result_columns
            .iter()
            .map(|(name, kind)| format!("{} {kind}", quote_if_needed(name)))
            .collect::<Vec<_>>();
        context.push_str(&columns.join(", "));
        context.push('\n');
    }
    let editor = request.editor_text.trim();
    if !editor.is_empty() && request.selection.is_none() {
        context.push_str("\nThe SQL editor currently contains:\n```sql\n");
        context.push_str(truncate(editor, EDITOR_BUDGET));
        context.push_str("\n```\n");
    }
    let mut user = request.prompt.trim().to_owned();
    if let Some(selection) = &request.selection {
        user = format!(
            "Rewrite this SQL. {user}\n```sql\n{}\n```",
            truncate(selection.trim(), EDITOR_BUDGET)
        );
    }
    vec![
        ChatMessage::new("system", format!("{SQL_SYSTEM_PROMPT}\n\n{context}")),
        ChatMessage::new("user", user),
    ]
}

/// The follow-up after the model's SQL failed to prepare.
pub fn fix_messages(mut messages: Vec<ChatMessage>, reply: &str, error: &str) -> Vec<ChatMessage> {
    messages.push(ChatMessage::new("assistant", reply));
    messages.push(ChatMessage::new(
        "user",
        format!(
            "DuckDB rejected that SQL:\n{error}\nFix it. Reply with the corrected SQL only, in a single ```sql code block."
        ),
    ));
    messages
}

fn comment_text(comment: &Option<String>) -> String {
    match comment.as_deref().map(str::trim) {
        Some(comment) if !comment.is_empty() => format!(" [{}]", comment.replace('\n', " ")),
        _ => String::new(),
    }
}

fn column_text(column: &SchemaColumn) -> String {
    format!(
        "{} {}{}",
        quote_if_needed(&column.name),
        column.kind,
        comment_text(&column.comment)
    )
}

fn relation_heading(relation: &SchemaRelation) -> String {
    format!(
        "{}{}{}",
        relation.name,
        if relation.is_view { " (view)" } else { "" },
        comment_text(&relation.comment)
    )
}

fn value_text(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("'{}'", truncate(value, VALUE_CHARS).replace('\'', "''")),
        None => "NULL".into(),
    }
}

/// One line per column: counts, then values when the provider allows data.
fn profile_text(relation: &SchemaRelation, profile: &RelationProfile) -> String {
    let mut text = format!("{} · {} rows\n", relation_heading(relation), profile.rows);
    for (column, stats) in relation.columns.iter().zip(&profile.columns) {
        let null_share = if profile.rows == 0 {
            0.0
        } else {
            stats.nulls as f64 * 100.0 / profile.rows as f64
        };
        let estimate = if stats.all_values { "" } else { "~" };
        text.push_str(&format!(
            "  {}: {estimate}{} distinct, {null_share:.1}% null",
            column_text(column),
            stats.unique
        ));
        if let (Some(min), Some(max)) = (&stats.min, &stats.max) {
            text.push_str(&format!(
                "; min {}, max {}",
                value_text(Some(min)),
                value_text(Some(max))
            ));
        }
        if !stats.values.is_empty() {
            let values = stats
                .values
                .iter()
                .map(|(value, count)| format!("{} ({count})", value_text(value.as_deref())))
                .collect::<Vec<_>>()
                .join(", ");
            let label = if stats.all_values {
                "values"
            } else {
                "most frequent"
            };
            text.push_str(&format!("; {label}: {values}"));
        }
        text.push('\n');
    }
    text
}

fn describe_relations(relations: &[SchemaRelation]) -> String {
    if relations.is_empty() {
        return "There are no tables or views yet; files can be read with read_csv, read_parquet, and read_json.\n".into();
    }
    // With @ mentions only those are described; the rest are named for joins.
    if relations.iter().any(|relation| relation.profile.is_some()) {
        let mut text = String::from("Relations the user named:\n");
        let mut others = Vec::new();
        for relation in relations {
            match &relation.profile {
                Some(profile) => text.push_str(&profile_text(relation, profile)),
                None => others.push(relation.name.as_str()),
            }
        }
        if !others.is_empty() {
            text.push_str(&format!(
                "Other relations (not described): {}\n",
                truncate(&others.join(", "), SCHEMA_BUDGET / 4)
            ));
        }
        return text;
    }
    let mut text = String::from("Relations (name: columns):\n");
    let mut listed = 0;
    for relation in relations {
        let columns = relation.columns.iter().map(column_text).collect::<Vec<_>>();
        let line = format!("{}: {}\n", relation_heading(relation), columns.join(", "));
        if text.len() + line.len() > SCHEMA_BUDGET {
            break;
        }
        text.push_str(&line);
        listed += 1;
    }
    let rest = &relations[listed..];
    if !rest.is_empty() {
        let names = rest.iter().map(|r| r.name.as_str()).collect::<Vec<_>>();
        text.push_str(&format!(
            "Other relations (columns not listed): {}\n",
            truncate(&names.join(", "), SCHEMA_BUDGET / 4)
        ));
    }
    text
}

fn quote_if_needed(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The SQL in a model reply: the first fenced block, else the whole reply.
pub fn extract_sql(reply: &str) -> String {
    let reply = strip_thinking(reply);
    if let Some(start) = reply.find("```") {
        let body = &reply[start + 3..];
        // Skip the language tag on the fence line.
        let body = body.split_once('\n').map_or("", |(_, rest)| rest);
        let end = body.find("```").unwrap_or(body.len());
        return body[..end].trim().to_owned();
    }
    reply.trim().to_owned()
}

/// Some local reasoning models put their reasoning inline in `<think>` tags.
fn strip_thinking(reply: &str) -> &str {
    match reply.find("</think>") {
        Some(end) if reply.trim_start().starts_with("<think>") => &reply[end + "</think>".len()..],
        _ => reply,
    }
}

/// How many times a reply DuckDB rejects is sent back to be fixed.
const MAX_FIXES: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlCheck {
    /// Every statement prepared.
    Passed,
    /// Statements after one that writes may depend on it, so they were not prepared.
    Partial { checked: usize, total: usize },
    /// DuckDB still rejected it after the fixes.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlProposal {
    pub sql: String,
    pub check: SqlCheck,
}

/// One step of a request, for the log the ask strip can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiEvent {
    /// Messages added to the conversation and sent; earlier ones are resent too.
    Sent(Vec<ChatMessage>),
    Received {
        text: String,
        elapsed: std::time::Duration,
    },
    Checked(SqlCheck),
    Failed(String),
    /// Preparation before the request, such as profiling an `@` table.
    Note(String),
}

/// Asks for SQL, prepares it with `prepare`, and sends errors back to be
/// fixed, reporting each step to `log`.
pub fn generate_sql(
    request: &SqlRequest,
    mut chat: impl FnMut(&[ChatMessage]) -> Result<String, String>,
    mut prepare: impl FnMut(&str) -> Result<(), String>,
    mut log: impl FnMut(AiEvent),
) -> Result<SqlProposal, String> {
    let mut messages = sql_messages(request);
    let mut sent = 0;
    let mut fixes = 0;
    loop {
        log(AiEvent::Sent(messages[sent..].to_vec()));
        sent = messages.len();
        let started = std::time::Instant::now();
        let reply = chat(&messages).inspect_err(|error| log(AiEvent::Failed(error.clone())))?;
        log(AiEvent::Received {
            text: reply.clone(),
            elapsed: started.elapsed(),
        });
        let sql = extract_sql(&reply);
        if sql.is_empty() {
            let error = "the model replied without SQL".to_owned();
            log(AiEvent::Failed(error.clone()));
            return Err(error);
        }
        let check = check_statements(&sql, &mut prepare);
        log(AiEvent::Checked(check.clone()));
        match check {
            SqlCheck::Failed(error) if fixes < MAX_FIXES => {
                fixes += 1;
                messages = fix_messages(messages, &reply, &error);
                // The reply going back was already logged as received.
                sent += 1;
            }
            check => return Ok(SqlProposal { sql, check }),
        }
    }
}

fn check_statements(sql: &str, prepare: &mut impl FnMut(&str) -> Result<(), String>) -> SqlCheck {
    let ranges = statements(sql);
    let total = ranges.len();
    for (index, range) in ranges.into_iter().enumerate() {
        let statement = &sql[range];
        if let Err(error) = prepare(statement) {
            return SqlCheck::Failed(error);
        }
        if index + 1 < total && may_mutate(statement) {
            return SqlCheck::Partial {
                checked: index + 1,
                total,
            };
        }
    }
    SqlCheck::Passed
}

/// The text that puts `sql` at `range` of `text`: a selection is replaced as
/// is; at a cursor it goes on its own lines, ending in `;` when other SQL
/// follows, so running the current statement runs just it. Returns the text
/// to insert and the offsets of the SQL within it.
pub fn insertion(
    text: &str,
    range: std::ops::Range<usize>,
    sql: &str,
) -> (String, std::ops::Range<usize>) {
    if !range.is_empty() {
        return (sql.to_owned(), 0..sql.len());
    }
    let before = &text[..range.start];
    let after = &text[range.start..];
    let mut sql = sql.trim().to_owned();
    if !after.trim().is_empty() && !sql.ends_with(';') {
        sql.push(';');
    }
    let line_before = before.rsplit('\n').next().unwrap_or("");
    let prefix = if line_before.trim().is_empty() {
        ""
    } else if before.trim_end().ends_with(';') {
        "\n"
    } else {
        ";\n"
    };
    let line_after = after.split('\n').next().unwrap_or("");
    let suffix = if line_after.trim().is_empty() {
        ""
    } else {
        "\n"
    };
    let start = prefix.len();
    (format!("{prefix}{sql}{suffix}"), start..start + sql.len())
}

/// `name`, `catalog.name`, or `catalog.schema.name`, leaving out the parts
/// DuckDB resolves by default.
pub fn relation_name(catalog: &str, schema: &str, name: &str, default_catalog: &str) -> String {
    let mut parts = Vec::new();
    if catalog != default_catalog || schema != "main" {
        parts.push(catalog);
    }
    if schema != "main" {
        parts.push(schema);
    }
    parts.push(name);
    parts
        .into_iter()
        .map(quote_if_needed)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AiProvider {
        AiProvider {
            name: "AI Factory".into(),
            base_url: "https://ai.example.com/v1/".into(),
            api_key: "secret".into(),
            model: "gpt-6-sole".into(),
            headers: vec![AiHeader {
                name: "OpenAI-Organization".into(),
                value: "org".into(),
            }],
            send_data: false,
        }
    }

    #[test]
    fn chat_url_appends_the_endpoint_once() {
        assert_eq!(
            provider().chat_url(),
            "https://ai.example.com/v1/chat/completions"
        );
        let full = AiProvider {
            base_url: "http://localhost:11434/v1/chat/completions".into(),
            ..provider()
        };
        assert_eq!(
            full.chat_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn api_key_reads_environment_references() {
        assert_eq!(provider().resolve_api_key(), Ok(Some("secret".into())));
        let empty = AiProvider {
            api_key: " ".into(),
            ..provider()
        };
        assert_eq!(empty.resolve_api_key(), Ok(None));
        let path = std::env::var("PATH").unwrap();
        for key in ["{env:PATH}", "$PATH", "{env: PATH }"] {
            let env = AiProvider {
                api_key: key.into(),
                ..provider()
            };
            assert_eq!(env.resolve_api_key(), Ok(Some(path.clone())), "{key}");
        }
        let missing = AiProvider {
            api_key: "{env:DATADIVER_SURELY_MISSING_KEY}".into(),
            ..provider()
        };
        assert!(missing.resolve_api_key().unwrap_err().contains("not set"));
    }

    #[test]
    fn validation_checks_names_urls_models_and_headers() {
        let others = vec![provider()];
        assert!(provider().validate(&others, Some("AI Factory")).is_ok());
        assert!(
            provider()
                .validate(&others, None)
                .unwrap_err()
                .contains("already exists")
        );
        let bad_url = AiProvider {
            name: "x".into(),
            base_url: "ai.example.com".into(),
            ..provider()
        };
        assert!(bad_url.validate(&others, None).is_err());
        let bad_header = AiProvider {
            name: "x".into(),
            headers: vec![AiHeader {
                name: "Bad Header".into(),
                value: String::new(),
            }],
            ..provider()
        };
        assert!(bad_header.validate(&others, None).is_err());
    }

    #[test]
    fn active_provider_falls_back_to_the_first() {
        let mut settings = AiSettings {
            default_provider: Some("missing".into()),
            providers: vec![provider()],
        };
        assert_eq!(settings.active().unwrap().name, "AI Factory");
        settings.providers.clear();
        assert!(settings.active().is_none());
    }

    #[test]
    fn extracts_fenced_sql() {
        assert_eq!(
            extract_sql("Here:\n```sql\nSELECT 1;\n```\nDone"),
            "SELECT 1;"
        );
        assert_eq!(extract_sql("```\nSELECT 2\n```"), "SELECT 2");
        assert_eq!(extract_sql("  SELECT 3  "), "SELECT 3");
        assert_eq!(
            extract_sql("<think>```sql\nnope\n```</think>\n```sql\nSELECT 4\n```"),
            "SELECT 4"
        );
    }

    fn column(name: &str, kind: &str, comment: Option<&str>) -> SchemaColumn {
        SchemaColumn {
            name: name.into(),
            kind: kind.into(),
            comment: comment.map(Into::into),
        }
    }

    #[test]
    fn prompt_lists_schema_comments_and_selection() {
        let request = SqlRequest {
            prompt: "add a total".into(),
            relations: vec![SchemaRelation {
                name: "memory.main.sales".into(),
                comment: Some("Monthly\nsales".into()),
                columns: vec![
                    column("region", "VARCHAR", Some("Sales region")),
                    column("Unit Price", "DOUBLE", None),
                ],
                ..Default::default()
            }],
            result_columns: vec![("total".into(), "BIGINT".into())],
            editor_text: "SELECT * FROM sales".into(),
            selection: Some("SELECT region FROM sales".into()),
        };
        let messages = sql_messages(&request);
        let system = &messages[0].content;
        assert!(
            system.contains(
                "memory.main.sales [Monthly sales]: region VARCHAR [Sales region], \"Unit Price\" DOUBLE"
            ),
            "{system}"
        );
        // Without @ mentions no statistics are gathered.
        assert!(!system.contains("distinct"));
        assert!(system.contains("current result (shown in the grid) has columns: total BIGINT"));
        // The selection replaces the editor text as context.
        assert!(!system.contains("SQL editor currently contains"));
        assert!(
            messages[1]
                .content
                .starts_with("Rewrite this SQL. add a total")
        );
        assert!(messages[1].content.contains("SELECT region FROM sales"));

        let fixed = fix_messages(messages, "SELECT nope", "Binder Error");
        assert_eq!(fixed.len(), 4);
        assert_eq!(fixed[2].role, "assistant");
        assert!(fixed[3].content.contains("Binder Error"));
    }

    #[test]
    fn generate_sql_sends_errors_back_until_it_prepares() {
        let request = SqlRequest {
            prompt: "count rows".into(),
            ..Default::default()
        };
        let mut replies = vec![
            "```sql\nSELECT count(*) FROM t\n```",
            "```sql\nSELECT nope\n```",
        ];
        let mut calls = Vec::new();
        let mut events = Vec::new();
        let proposal = generate_sql(
            &request,
            |messages| {
                calls.push(messages.len());
                Ok(replies.pop().unwrap().into())
            },
            |sql| {
                if sql.contains("nope") {
                    Err("Binder Error: nope".into())
                } else {
                    Ok(())
                }
            },
            |event| events.push(event),
        )
        .unwrap();
        assert_eq!(proposal.sql, "SELECT count(*) FROM t");
        assert_eq!(proposal.check, SqlCheck::Passed);
        assert_eq!(calls, [2, 4]);
        // Each send logs only what is new: the system and user messages, then
        // the fix request (the reply it answers was logged as received).
        let roles = |event: &AiEvent| -> Vec<String> {
            match event {
                AiEvent::Sent(messages) => messages.iter().map(|m| m.role.clone()).collect(),
                _ => Vec::new(),
            }
        };
        assert_eq!(events.len(), 6);
        assert_eq!(roles(&events[0]), ["system", "user"]);
        assert!(matches!(&events[1], AiEvent::Received { text, .. } if text.contains("nope")));
        assert_eq!(
            events[2],
            AiEvent::Checked(SqlCheck::Failed("Binder Error: nope".into()))
        );
        assert_eq!(roles(&events[3]), ["user"]);
        assert_eq!(events[5], AiEvent::Checked(SqlCheck::Passed));

        // After two fixes the last reply is offered with its error.
        let mut asked = 0;
        let proposal = generate_sql(
            &request,
            |_| {
                asked += 1;
                Ok("SELECT nope".into())
            },
            |_| Err("still wrong".into()),
            |_| {},
        )
        .unwrap();
        assert_eq!(asked, 3);
        assert_eq!(proposal.check, SqlCheck::Failed("still wrong".into()));
        let mut events = Vec::new();
        let result = generate_sql(
            &request,
            |_| Err("offline".into()),
            |_| Ok(()),
            |event| events.push(event),
        );
        assert!(result.is_err());
        assert_eq!(events.last(), Some(&AiEvent::Failed("offline".into())));
    }

    #[test]
    fn statements_after_a_write_are_not_prepared() {
        let mut prepared = Vec::new();
        let check = check_statements(
            "CREATE TABLE x AS SELECT 1 AS a; SELECT a FROM x",
            &mut |sql: &str| {
                prepared.push(sql.trim().to_owned());
                Ok(())
            },
        );
        assert_eq!(
            check,
            SqlCheck::Partial {
                checked: 1,
                total: 2
            }
        );
        assert_eq!(prepared, ["CREATE TABLE x AS SELECT 1 AS a;"]);
        assert_eq!(
            check_statements("SELECT 1; SELECT 2", &mut |_: &str| Ok(())),
            SqlCheck::Passed
        );
    }

    #[test]
    fn insertion_keeps_statements_apart() {
        // A selection is replaced exactly.
        assert_eq!(
            insertion("SELECT 1", 0..8, "SELECT 2"),
            ("SELECT 2".into(), 0..8)
        );
        // An empty editor or blank line takes the SQL as is.
        assert_eq!(insertion("", 0..0, "SELECT 1\n"), ("SELECT 1".into(), 0..8));
        // After a statement on the same line, it starts a new line.
        let text = "SELECT 1;";
        assert_eq!(
            insertion(text, 9..9, "SELECT 2"),
            ("\nSELECT 2".into(), 1..9)
        );
        // After an unterminated statement, that one is ended first.
        let text = "SELECT 1";
        assert_eq!(
            insertion(text, 8..8, "SELECT 2"),
            (";\nSELECT 2".into(), 2..10)
        );
        // With SQL after the cursor, it ends in `;` and on its own line.
        let text = "SELECT 9";
        assert_eq!(
            insertion(text, 0..0, "SELECT 2"),
            ("SELECT 2;\n".into(), 0..9)
        );
    }

    #[test]
    fn relation_names_leave_out_default_parts() {
        assert_eq!(
            relation_name("profile", "main", "sales", "profile"),
            "sales"
        );
        assert_eq!(relation_name("temp", "main", "t", "profile"), "temp.t");
        assert_eq!(
            relation_name("profile", "files", "My Data", "profile"),
            "profile.files.\"My Data\""
        );
    }

    #[test]
    fn named_relations_get_profiles_and_others_only_names() {
        let sales = SchemaRelation {
            name: "sales".into(),
            columns: vec![
                column("region", "VARCHAR", None),
                column("amount", "DOUBLE", None),
            ],
            profile: Some(RelationProfile {
                rows: 200,
                columns: vec![
                    ColumnProfile {
                        nulls: 0,
                        unique: 2,
                        min: Some("East".into()),
                        max: Some("O'Neil".into()),
                        values: vec![(Some("East".into()), 120), (Some("O'Neil".into()), 80)],
                        all_values: true,
                    },
                    ColumnProfile {
                        nulls: 50,
                        unique: 140,
                        min: Some("0.5".into()),
                        max: Some("99".into()),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };
        let other = SchemaRelation {
            name: "temp.fruit".into(),
            columns: vec![column("color", "VARCHAR", None)],
            ..Default::default()
        };
        let text = describe_relations(&[sales, other]);
        assert!(text.contains("sales · 200 rows"), "{text}");
        assert!(text.contains(
            "  region VARCHAR: 2 distinct, 0.0% null; min 'East', max 'O''Neil'; values: 'East' (120), 'O''Neil' (80)"
        ), "{text}");
        assert!(text.contains("  amount DOUBLE: ~140 distinct, 25.0% null; min '0.5', max '99'\n"));
        assert!(text.contains("Other relations (not described): temp.fruit"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn mentions_match_whole_names_only() {
        let relation = |name: &str| SchemaRelation {
            name: name.into(),
            ..Default::default()
        };
        let relations = [
            relation("sales"),
            relation("sales_2024"),
            relation("temp.fruit"),
            relation("\"My Data\""),
        ];
        assert_eq!(
            mentioned_relations(
                "join @sales_2024 to @\"My Data\", by me@sales.com",
                &relations
            ),
            [1, 3]
        );
        assert_eq!(
            mentioned_relations("from @Sales and @temp.fruit.", &relations),
            [0, 2]
        );
        assert!(mentioned_relations("no tables", &relations).is_empty());
    }

    #[test]
    fn mention_queries_cover_the_name_being_typed() {
        let query =
            |text: &str, cursor: usize| mention_query(text, cursor).map(|q| (q.typed, q.range));
        assert_eq!(query("sum @sal", 8), Some(("sal".into(), 4..8)));
        assert_eq!(query("@", 1), Some((String::new(), 0..1)));
        // Mid-name, the whole name is replaced.
        assert_eq!(query("@sales by", 3), Some(("sa".into(), 0..6)));
        // An open quote allows spaces.
        assert_eq!(query("@\"My Da", 7), Some(("My Da".into(), 0..7)));
        assert_eq!(query("@sales by", 9), None);
        assert_eq!(query("me@sales", 8), None);
    }

    #[test]
    fn schema_budget_lists_overflow_by_name() {
        let wide = SchemaRelation {
            name: "wide".into(),
            columns: (0..2000)
                .map(|i| column(&format!("column_{i}"), "INTEGER", None))
                .collect(),
            ..Default::default()
        };
        let relations = vec![
            wide.clone(),
            SchemaRelation {
                name: "later".into(),
                ..wide
            },
        ];
        let text = describe_relations(&relations);
        assert!(text.len() < SCHEMA_BUDGET + 1000);
        assert!(text.contains("Other relations (columns not listed): later"));
    }
}
