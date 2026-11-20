use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_QUERY_LIMIT: u64 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ScalarKey {
    Null,
    Boolean(bool),
    String(String),
    Signed(String),
    Unsigned(String),
    FloatBits(String),
    Decimal(String),
    Date(String),
    Datetime {
        value: String,
        unit: String,
        timezone: Option<String>,
    },
    Time(String),
    Duration {
        value: String,
        unit: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormatOverride {
    pub grouping: bool,
    pub precision: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnPresentation {
    #[serde(deserialize_with = "deserialize_json_f32")]
    pub width: f32,
    pub alignment: Alignment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_override: Option<FormatOverride>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    /// The column sorted by, or with `sql`, the SQL expression.
    pub column: String,
    pub descending: bool,
    /// `column` is a SQL expression over the row, such as
    /// `CASE status WHEN 'open' THEN 0 ELSE 1 END`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sql: bool,
}

impl SortKey {
    pub fn column(column: impl Into<String>, descending: bool) -> Self {
        Self {
            column: column.into(),
            descending,
            sql: false,
        }
    }

    pub fn expression(sql: impl Into<String>, descending: bool) -> Self {
        Self {
            column: sql.into(),
            descending,
            sql: true,
        }
    }

    /// Whether this key sorts by the column `name`.
    pub fn is_column(&self, name: &str) -> bool {
        !self.sql && self.column == name
    }

    /// Whether both keys sort by the same column or expression.
    pub fn same_target(&self, other: &SortKey) -> bool {
        self.sql == other.sql && self.column == other.column
    }

    /// The column or expression on one line, for lists.
    pub fn target_label(&self) -> String {
        self.column.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableConfig {
    pub column_order: Vec<String>,
    pub frozen_count: usize,
    pub sort: Vec<SortKey>,
    pub presentation: BTreeMap<String, ColumnPresentation>,
    /// Columns left out of the grid and of plain search. They keep their place
    /// in `column_order`, so showing one puts it back where it was.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregationOp {
    Sum,
    Mean,
    Median,
    Minimum,
    Maximum,
    RowCount,
    NonNullCount,
    DistinctNonNullCount,
    First,
    Last,
    StandardDeviation,
    Variance,
    Percentile,
    ValuesList,
    UniqueValuesList,
    SqlExpression,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Linear,
    Nearest,
    Lower,
    Higher,
    Midpoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ConditionValue {
    Literal(ScalarKey),
    TodayOffsetDays(i32),
    NowOffsetSeconds(i64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Equal,
    NotEqual,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    Contains,
    StartsWith,
    EndsWith,
    In,
    IsNull,
    IsNotNull,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub column: String,
    pub operation: ConditionOp,
    #[serde(default)]
    pub values: Vec<ConditionValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aggregation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_column: String,
    pub operation: AggregationOp,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default, deserialize_with = "deserialize_optional_json_f64")]
    pub percentile: Option<f64>,
    #[serde(default)]
    pub interpolation: Option<Interpolation>,
    #[serde(default)]
    pub ddof: Option<u8>,
}

impl Aggregation {
    pub fn sql_expression(sql: String, name: Option<String>) -> Self {
        Self {
            sql: Some(sql),
            source_column: String::new(),
            operation: AggregationOp::SqlExpression,
            name,
            conditions: Vec::new(),
            percentile: None,
            interpolation: None,
            ddof: None,
        }
    }

    pub fn validate_expression(&self, pivot: bool) -> Result<(), String> {
        if self.operation == AggregationOp::SqlExpression {
            let sql = self.sql.as_deref().ok_or("SQL expression is missing")?;
            crate::domain::aggregation_sql::validate(sql, pivot)?;
            if !self.source_column.is_empty()
                || !self.conditions.is_empty()
                || self.percentile.is_some()
                || self.interpolation.is_some()
                || self.ddof.is_some()
            {
                return Err("SQL expressions cannot have structured aggregation options".into());
            }
        } else if self.sql.is_some() {
            return Err("SQL expression requires the SQL operation".into());
        }
        Ok(())
    }
}

fn deserialize_json_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let number = serde_json::Number::deserialize(deserializer)?;
    let value = number
        .as_f64()
        .ok_or_else(|| D::Error::custom("invalid width"))?;
    if !value.is_finite() || value.abs() > f32::MAX as f64 {
        return Err(D::Error::custom("width is out of range"));
    }
    Ok(value as f32)
}

fn deserialize_optional_json_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let number = Option::<serde_json::Number>::deserialize(deserializer)?;
    number
        .map(|number| {
            number
                .as_f64()
                .ok_or_else(|| D::Error::custom("invalid percentile"))
        })
        .transpose()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dimension {
    Column(String),
    Expression { sql: String },
}

impl Dimension {
    pub fn expression(sql: impl Into<String>) -> Self {
        Self::Expression { sql: sql.into() }
    }

    pub fn column(&self) -> Option<&str> {
        match self {
            Self::Column(name) => Some(name),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Column(name) if name.is_empty() => Err("column name is empty".into()),
            Self::Column(_) => Ok(()),
            Self::Expression { sql } => crate::domain::aggregation_sql::validate_dimension(sql),
        }
    }
}

impl From<String> for Dimension {
    fn from(name: String) -> Self {
        Self::Column(name)
    }
}
impl From<&str> for Dimension {
    fn from(name: &str) -> Self {
        Self::Column(name.into())
    }
}
impl std::fmt::Display for Dimension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Column(name) => f.write_str(name),
            Self::Expression { sql } => f.write_str(sql),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryConfig {
    pub group_by: Vec<Dimension>,
    pub pivot_by: Vec<Dimension>,
    pub aggregations: Vec<Aggregation>,
    pub result_sort: Vec<SortKey>,
    /// Show grand totals: a total row under the grid and, for a pivot, a
    /// `(Total)` column per row. Totals aggregate source rows, so they stay
    /// correct for averages, medians, and distinct counts.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub totals: bool,
    /// Conditions on the result's columns, applied after grouping like
    /// SQL's HAVING. One per column.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub result_filters: Vec<ResultFilter>,
    /// SQL conditions on the result's columns, applied after grouping like
    /// SQL's HAVING: `"Count" > 1`. A group stays when every one holds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub having: Vec<String>,
    /// Groups by only the first `depth` group-by fields; the fields after
    /// them are collapsed. `None` groups by every field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    /// Shows the groups as an Excel-style outline, with items that collapse
    /// into a subtotal row. `None` shows the flat result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<Outline>,
}

impl QueryConfig {
    /// The group-by fields the result has: all of them, or the first
    /// `depth` while the rest are collapsed.
    pub fn shown_group_by(&self) -> &[Dimension] {
        &self.group_by[..self.depth.unwrap_or(usize::MAX).min(self.group_by.len())]
    }

    /// The flat query the engines run: the shown fields only, no outline.
    pub fn flattened(&self) -> QueryConfig {
        QueryConfig {
            group_by: self.shown_group_by().to_vec(),
            depth: None,
            outline: None,
            ..self.clone()
        }
    }

    /// The outline when it has items to collapse: two shown fields or more.
    pub fn active_outline(&self) -> Option<&Outline> {
        self.outline.as_ref().filter(|_| self.shown_group_by().len() >= 2)
    }

    /// How many leading result columns show group keys: one per shown field,
    /// or the single column of row labels in the compact layout.
    pub fn key_columns(&self) -> usize {
        match self.active_outline() {
            Some(outline) if outline.layout == OutlineLayout::Compact => 1,
            _ => self.shown_group_by().len(),
        }
    }
}

/// The key values of an outline item, one per group-by field from the
/// first, as the grid shows them; `None` is SQL NULL.
pub type OutlinePath = Vec<Option<String>>;

/// How a query shows its groups as an outline. An item is the group of one
/// path of key values; a collapsed item shows as one subtotal row, which
/// aggregates source rows like the totals do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Outline {
    pub layout: OutlineLayout,
    /// Items at this level (1 for the first field) and deeper start
    /// collapsed: "expand to level N". `None` starts every item expanded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<usize>,
    /// Items in the other state from the one `level` starts them in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub toggled: Vec<OutlinePath>,
    /// Levels whose expanded items show their subtotals. In the tabular
    /// layout such an item gets a subtotal row; the other layouts always
    /// show a row above an expanded item, with its subtotals only here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtotals: Vec<usize>,
}

impl Outline {
    /// Whether the item at `path` (its level is the path's length) is collapsed.
    pub fn is_collapsed(&self, path: &[Option<String>]) -> bool {
        let starts_collapsed = self.level.is_some_and(|level| path.len() >= level);
        starts_collapsed != self.toggled.iter().any(|toggled| toggled.as_slice() == path)
    }

    /// Collapses or expands the item at `path`. Returns whether it changed.
    pub fn set_collapsed(&mut self, path: &[Option<String>], collapsed: bool) -> bool {
        if self.is_collapsed(path) == collapsed {
            return false;
        }
        match self.toggled.iter().position(|toggled| toggled.as_slice() == path) {
            Some(index) => {
                self.toggled.remove(index);
            }
            None => self.toggled.push(path.to_vec()),
        }
        true
    }

    /// Whether expanded items at `level` show a row of their own.
    pub fn shows_header(&self, level: usize) -> bool {
        self.layout != OutlineLayout::Tabular || self.subtotals.contains(&level)
    }

    /// Whether expanded items at `level` show their subtotals.
    pub fn shows_subtotals(&self, level: usize) -> bool {
        self.subtotals.contains(&level)
    }
}

/// Where an outline shows its keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlineLayout {
    /// Each field in its own column, with the keys repeated on every row.
    #[default]
    Tabular,
    /// Each field in its own column; an expanded item's row sits above its
    /// children, which leave its keys blank.
    Outline,
    /// One indented column of row labels.
    Compact,
}

impl OutlineLayout {
    pub const ALL: [OutlineLayout; 3] = [Self::Tabular, Self::Outline, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tabular => "Tabular",
            Self::Outline => "Outline",
            Self::Compact => "Compact",
        }
    }
}

/// A condition on one column of a query's result, in the language of
/// condition cards: `>100`, `10..20`, `!=0`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultFilter {
    pub column: String,
    pub condition: String,
}

impl ResultFilter {
    pub fn label(&self) -> String {
        format!("{} {}", self.column, self.condition.trim())
    }
}

/// A SQL source, whose last statement's rows other views can be built on,
/// or a SQL script, a scratchpad of statements run now and then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqlConfig {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub script: bool,
    /// Which engine runs the SQL: all of DuckDB's, or Polars SQL.
    pub engine: Engine,
    pub result_sort: Vec<SortKey>,
    /// Runs the whole script each time the view opens.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_run: bool,
    /// A SQL source's rows are read from its last statement each time they
    /// are queried, like a database view, rather than kept in memory. Only
    /// DuckDB streams.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub streaming: bool,
    /// The leading result columns kept in view, by name. SQL sets the column
    /// order, so only the part still leading the result is frozen.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub frozen_columns: Vec<String>,
}

impl Default for SqlConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            script: false,
            engine: Engine::DuckDb,
            result_sort: Vec::new(),
            auto_run: false,
            streaming: false,
            frozen_columns: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlicerDefinition {
    pub column: String,
    #[serde(default)]
    pub selected: Vec<ScalarKey>,
    /// A condition card's text, such as `>=10 <100`, or an SQL card's
    /// predicate. `None` makes a value card, which filters by `selected` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// An SQL card: `condition` is a DuckDB predicate over the whole source row,
    /// and `column` is empty. A view has at most one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sql: bool,
}

impl SlicerDefinition {
    /// An empty SQL card.
    pub fn sql_filter() -> Self {
        Self {
            column: String::new(),
            selected: Vec::new(),
            condition: Some(String::new()),
            sql: true,
        }
    }

    /// Whether the card currently narrows the rows.
    pub fn is_active(&self) -> bool {
        match &self.condition {
            Some(text) => !text.trim().is_empty(),
            None => !self.selected.is_empty(),
        }
    }

    /// Removes the card's selection or condition text, keeping the card.
    pub fn clear(&mut self) {
        self.selected.clear();
        if let Some(text) = &mut self.condition {
            text.clear();
        }
    }
}

/// Keeps rows whose target renders as one of `values`, or with `exclude`
/// removes them; an excluded value keeps rows where the target is NULL.
/// Values are compared as DuckDB renders them with `CAST(... AS VARCHAR)`, which is
/// also how the grid displays cells, so any displayed value can be matched exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueFilter {
    pub target: Dimension,
    /// `None` matches SQL NULL.
    pub values: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude: bool,
}

impl ValueFilter {
    /// How many values a label lists before counting the rest.
    const LABEL_VALUES: usize = 3;

    pub fn label(&self) -> String {
        let target = match &self.target {
            Dimension::Column(name) => name.clone(),
            Dimension::Expression { sql } => crate::domain::aggregation_sql::dimension_alias(sql)
                .unwrap_or_else(|| sql.trim().to_owned()),
        };
        let shown = |value: &Option<String>| match value {
            Some(value) => crate::domain::formatting::truncate(value, 40),
            None => "NULL".to_owned(),
        };
        match self.values.as_slice() {
            [None] if self.exclude => format!("{target} is not null"),
            [None] => format!("{target} is null"),
            [value] => format!("{target} {} {}", if self.exclude { "≠" } else { "=" }, shown(value)),
            values => {
                let mut listed: Vec<String> = values.iter().take(Self::LABEL_VALUES).map(shown).collect();
                if values.len() > Self::LABEL_VALUES {
                    listed.push(format!("+{} more", values.len() - Self::LABEL_VALUES));
                }
                let operator = if self.exclude { "not in" } else { "in" };
                format!("{target} {operator} ({})", listed.join(", "))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewAction {
    pub key: String,
    pub description: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "configuration", rename_all = "snake_case")]
pub enum ViewKind {
    Table(TableConfig),
    #[serde(alias = "pivot")]
    Query(QueryConfig),
    Sql(SqlConfig),
}

/// What a [`DatabaseView`] names in DuckDB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    #[default]
    View,
    /// A DuckDB table, read through DuckDB's own rowid like an imported file.
    Table,
    /// A table read only through queries: one in another kind of attached
    /// database, such as SQLite, or a DuckDB table whose own `rowid` column
    /// hides DuckDB's rowid.
    TableWithoutRowid,
    /// A table function called without arguments, such as `duckdb_tables()`.
    Function,
    /// A source file read in place through DuckDB's reader, never imported.
    /// The relation's `name` is the file's path.
    File,
}

/// A relation in the profile database, or in a database attached to it,
/// opened as a read-only source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatabaseView {
    /// An attached database; `None` is the profile database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    pub schema: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_default_kind")]
    pub kind: RelationKind,
}

fn is_default_kind(kind: &RelationKind) -> bool {
    *kind == RelationKind::View
}

impl DatabaseView {
    pub fn view(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn function(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::Function,
            ..Self::default()
        }
    }

    /// A source file opened in query mode, read in place on every query.
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            name: path.into(),
            kind: RelationKind::File,
            ..Self::default()
        }
    }

    /// The source path of a file opened in query mode.
    pub fn file_path(&self) -> Option<&str> {
        (self.kind == RelationKind::File).then_some(self.name.as_str())
    }

    pub fn label(&self) -> String {
        match (&self.kind, &self.catalog) {
            (RelationKind::Function, _) => format!("{}()", self.name),
            (RelationKind::File, _) => std::path::Path::new(&self.name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.name.clone()),
            (_, Some(catalog)) => format!("{catalog}.{}.{}", self.schema, self.name),
            (_, None) => format!("{}.{}", self.schema, self.name),
        }
    }

    /// The SQL that reads every row of the relation.
    pub fn relation_sql(&self) -> String {
        use crate::infrastructure::duckdb_store::quote_identifier;
        if self.kind == RelationKind::Function {
            return format!("{}()", quote_identifier(&self.name));
        }
        if self.kind == RelationKind::File {
            // An unsupported path fails when the store opens, with the reader's message.
            let path = format!("'{}'", self.name.replace('\'', "''"));
            return crate::infrastructure::source_database::source_reader(std::path::Path::new(
                &self.name,
            ))
            .map(|reader| reader.replacen('?', &path, 1))
            .unwrap_or_else(|_| format!("read_csv({path})"));
        }
        let relation = format!(
            "{}.{}",
            quote_identifier(&self.schema),
            quote_identifier(&self.name)
        );
        match &self.catalog {
            Some(catalog) => format!("{}.{relation}", quote_identifier(catalog)),
            None => relation,
        }
    }
}

/// Conditional formatting: rules in the formatting language (see the
/// `diver_format` crate) and the columns they format: those listed, and
/// those `selector` picks (see [`crate::domain::column_selector`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalFormat {
    pub rules: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selector: String,
}

impl ConditionalFormat {
    /// The indices of the `names` (in the grid's order) the format applies
    /// to; an invalid selector picks nothing more.
    pub fn targets<S: AsRef<str>>(&self, names: &[S]) -> Vec<usize> {
        let mut targets: Vec<usize> = self
            .columns
            .iter()
            .filter_map(|column| names.iter().position(|name| name.as_ref() == column))
            .collect();
        if !self.selector.trim().is_empty()
            && let Ok(selector) = crate::domain::column_selector::ColumnSelector::parse(&self.selector)
        {
            targets.extend(selector.select(names));
        }
        targets.sort_unstable();
        targets.dedup();
        targets
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_view: Option<DatabaseView>,
    /// The id of the SQL source whose rows the view reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_source: Option<String>,
    #[serde(default)]
    pub slicers: Vec<SlicerDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<ValueFilter>,
    #[serde(default)]
    pub actions: Vec<ViewAction>,
    /// Applied in order; a later format overrides what an earlier one set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<ConditionalFormat>,
    /// Saves each change a moment after it is made, once the view has a name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_save: bool,
    #[serde(flatten)]
    pub view: ViewKind,
}

impl DocumentDefinition {
    pub fn draft_table(name: String, source_path: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: String::new(),
            name,
            source_path,
            database_view: None,
            sql_source: None,
            slicers: Vec::new(),
            filters: Vec::new(),
            actions: Vec::new(),
            formats: Vec::new(),
            auto_save: false,
            view: ViewKind::Table(TableConfig::default()),
        }
    }

    /// Tables with DuckDB's rowid open in table mode; other relations open as
    /// a query.
    pub fn draft_database_view(view: DatabaseView) -> Self {
        let mut definition = Self::draft_table(view.label(), String::new());
        if view.kind != RelationKind::Table {
            definition.view = ViewKind::Query(QueryConfig::default());
        }
        definition.database_view = Some(view);
        definition
    }

    /// A table of the rows a saved SQL source returns.
    pub fn draft_sql_source(source: &DocumentDefinition) -> Self {
        let mut definition = Self::draft_table(source.name.clone(), String::new());
        definition.sql_source = Some(source.id.clone());
        definition
    }

    /// Whether this is a SQL script rather than a SQL source or another view.
    pub fn is_sql_script(&self) -> bool {
        matches!(&self.view, ViewKind::Sql(config) if config.script)
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.sql_source
            && (id.is_empty()
                || !self.source_path.is_empty()
                || self.database_view.is_some()
                || matches!(self.view, ViewKind::Sql(_)))
        {
            return Err("views on a SQL source read no file or database relation".into());
        }
        if let Some(view) = &self.database_view {
            if view.name.trim().is_empty()
                || (!matches!(view.kind, RelationKind::Function | RelationKind::File)
                    && view.schema.trim().is_empty())
            {
                return Err("database view schema and name are required".into());
            }
            let mode = match self.view {
                ViewKind::Query(_) => true,
                ViewKind::Table(_) => view.kind == RelationKind::Table,
                ViewKind::Sql(_) => false,
            };
            if !self.source_path.is_empty() || !mode {
                return Err(
                    "database views require query or table mode without a source file".into(),
                );
            }
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("unsupported view version {}", self.schema_version));
        }
        if !self
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("view ID contains invalid characters".into());
        }
        if self.name.trim().is_empty() {
            return Err("view name is empty".into());
        }
        crate::domain::actions::validate_actions(&self.actions)?;
        for filter in &self.filters {
            filter.target.validate()?;
        }
        if let ViewKind::Query(pivot) = &self.view {
            if pivot
                .group_by
                .iter()
                .enumerate()
                .any(|(index, value)| pivot.group_by[..index].contains(value))
            {
                return Err("duplicate group column".into());
            }
            if pivot
                .pivot_by
                .iter()
                .enumerate()
                .any(|(index, value)| pivot.pivot_by[..index].contains(value))
            {
                return Err("duplicate pivot dimension".into());
            }
            if pivot
                .aggregations
                .iter()
                .enumerate()
                .any(|(index, value)| pivot.aggregations[..index].contains(value))
            {
                return Err("duplicate aggregation".into());
            }
            for dimension in pivot.group_by.iter().chain(&pivot.pivot_by) {
                dimension.validate()?;
            }
            for group in &pivot.group_by {
                if pivot.pivot_by.contains(group) {
                    return Err(format!("column {group} is both grouped and pivoted"));
                }
            }
            for aggregation in &pivot.aggregations {
                aggregation.validate_expression(!pivot.pivot_by.is_empty())?;
                if let Some(percentile) = aggregation.percentile
                    && (!percentile.is_finite() || !(0.0..=100.0).contains(&percentile))
                {
                    return Err("percentile must be between 0 and 100".into());
                }
            }
        }
        Ok(())
    }
}

/// Which engine reads a source and runs its views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Polars,
    #[serde(rename = "duckdb")]
    DuckDb,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Polars, Engine::DuckDb];

    pub fn label(self) -> &'static str {
        match self {
            Self::Polars => "Polars",
            Self::DuckDb => "DuckDB",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_duckdb_tables_open_database_relations_in_table_mode() {
        for kind in [
            RelationKind::View,
            RelationKind::Table,
            RelationKind::TableWithoutRowid,
            RelationKind::Function,
        ] {
            let relation = DatabaseView {
                kind,
                ..DatabaseView::view("main", "sales")
            };
            let mut draft = DocumentDefinition::draft_database_view(relation);
            assert_eq!(
                matches!(draft.view, ViewKind::Table(_)),
                kind == RelationKind::Table
            );
            draft.validate().unwrap();
            draft.view = ViewKind::Query(QueryConfig::default());
            draft.validate().unwrap();
            draft.view = ViewKind::Table(TableConfig::default());
            assert_eq!(draft.validate().is_ok(), kind == RelationKind::Table);
            draft.view = ViewKind::Sql(SqlConfig::default());
            assert!(draft.validate().is_err());
        }
    }

    #[test]
    fn query_fields_round_trip_as_group_by_and_pivot_by() {
        let mut view = DocumentDefinition::draft_table("Sales".into(), "sales.csv".into());
        view.view = ViewKind::Query(QueryConfig {
            group_by: vec!["region".into()],
            pivot_by: vec!["year".into()],
            ..Default::default()
        });
        let json = serde_json::to_value(&view).unwrap();
        let config = &json["configuration"];
        assert_eq!(config["group_by"], serde_json::json!(["region"]));
        assert_eq!(config["pivot_by"], serde_json::json!(["year"]));
        assert!(config.get("groups").is_none());
        assert!(config.get("dimensions").is_none());
        assert_eq!(
            serde_json::from_value::<DocumentDefinition>(json).unwrap(),
            view
        );
    }

    #[test]
    fn legacy_pivot_kind_reads_as_query_and_writes_query() {
        let mut definition = DocumentDefinition::draft_table("Old".into(), "source.csv".into());
        definition.view = ViewKind::Query(QueryConfig::default());
        let json = serde_json::to_string(&definition).unwrap();
        assert!(json.contains("\"kind\":\"query\""));
        assert!(!json.contains("query_limit"));
        let legacy = json.replace("\"kind\":\"query\"", "\"kind\":\"pivot\"");
        assert_eq!(
            serde_json::from_str::<DocumentDefinition>(&legacy).unwrap(),
            definition
        );
    }

    #[test]
    fn typed_values_round_trip_without_precision_loss() {
        let value = ScalarKey::Unsigned(u64::MAX.to_string());
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<ScalarKey>(&json).unwrap(), value);
    }

    #[test]
    fn draft_round_trips() {
        let view =
            DocumentDefinition::draft_table("Customers".into(), "C:\\data\\customers.csv".into());
        let json = serde_json::to_string_pretty(&view).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentDefinition>(&json).unwrap(),
            view
        );
    }

    #[test]
    fn percentile_view_round_trips_as_json() {
        let mut view = DocumentDefinition::draft_table("Percentile".into(), "data.csv".into());
        view.view = ViewKind::Query(QueryConfig {
            aggregations: vec![Aggregation {
                sql: None,
                source_column: "amount".into(),
                operation: AggregationOp::Percentile,
                name: None,
                conditions: vec![],
                percentile: Some(75.0),
                interpolation: Some(Interpolation::Nearest),
                ddof: None,
            }],
            ..Default::default()
        });
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentDefinition>(&json).unwrap(),
            view
        );
    }

    #[test]
    fn table_width_round_trips_as_json() {
        let mut view = DocumentDefinition::draft_table("Table".into(), "data.csv".into());
        if let ViewKind::Table(config) = &mut view.view {
            config.presentation.insert(
                "amount".into(),
                ColumnPresentation {
                    width: 120.5,
                    alignment: Alignment::Right,
                    format_override: None,
                },
            );
        }
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentDefinition>(&json).unwrap(),
            view
        );
    }

    #[test]
    fn view_id_cannot_escape_storage_directory() {
        let mut view = DocumentDefinition::draft_table("Customers".into(), "C:\\data.csv".into());
        view.id = "../outside".into();
        assert!(view.validate().is_err());
        view.id = "id-1_2".into();
        assert!(view.validate().is_ok());
    }

    #[test]
    fn sql_and_legacy_aggregations_round_trip_without_a_source_requirement() {
        let legacy = r#"{"source_column":"amount","operation":"sum"}"#;
        let old: Aggregation = serde_json::from_str(legacy).unwrap();
        assert!(old.sql.is_none());
        assert!(!serde_json::to_string(&old).unwrap().contains("\"sql\""));
        let custom = Aggregation::sql_expression("count(*)".into(), Some("Rows".into()));
        assert!(custom.validate_expression(true).is_ok());
        let mut view = DocumentDefinition::draft_table("SQL".into(), "source.csv".into());
        view.view = ViewKind::Query(QueryConfig {
            aggregations: vec![old, custom],
            ..Default::default()
        });
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentDefinition>(&json).unwrap(),
            view
        );
        assert!(view.validate().is_ok());
    }
}

#[cfg(test)]
mod dimension_tests {
    use super::*;

    #[test]
    fn dimensions_read_legacy_and_round_trip_mixed_entries() {
        let legacy: QueryConfig =
            serde_json::from_value(serde_json::json!({"group_by":["region"],"pivot_by":["year"]}))
                .unwrap();
        assert_eq!(legacy.group_by, [Dimension::Column("region".into())]);
        let mut config = legacy;
        config
            .group_by
            .push(Dimension::expression("upper(region) AS RegionCode"));
        config.pivot_by.push(Dimension::expression("year + 1"));
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value["group_by"],
            serde_json::json!(["region", {"sql":"upper(region) AS RegionCode"}])
        );
        assert_eq!(
            serde_json::from_value::<QueryConfig>(value).unwrap(),
            config
        );
    }
}

#[cfg(test)]
mod sql_source_tests {
    use super::*;

    #[test]
    fn sql_sources_default_to_duckdb_and_views_on_them_read_nothing_else() {
        let config: SqlConfig = serde_json::from_value(serde_json::json!({"text": "SELECT 1"})).unwrap();
        assert_eq!(config.engine, Engine::DuckDb);
        assert!(!config.script);
        let mut source = DocumentDefinition::draft_table("Orders".into(), String::new());
        source.id = "orders".into();
        source.view = ViewKind::Sql(config);
        assert!(!source.is_sql_script());
        let mut view = DocumentDefinition::draft_sql_source(&source);
        assert_eq!(view.sql_source.as_deref(), Some("orders"));
        view.validate().unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["sql_source"], "orders");
        assert_eq!(serde_json::from_value::<DocumentDefinition>(json).unwrap(), view);
        view.source_path = "orders.csv".into();
        assert!(view.validate().is_err());
        view.source_path.clear();
        view.view = ViewKind::Sql(SqlConfig::default());
        assert!(view.validate().is_err());
    }
}
