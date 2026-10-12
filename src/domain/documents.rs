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
    pub column: String,
    pub descending: bool,
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
    /// Shade measure cells by where they fall in their column's range.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub heatmap: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqlConfig {
    pub text: String,
    pub auto_save: bool,
    pub result_sort: Vec<SortKey>,
    /// Runs the whole script each time the view opens.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_run: bool,
    /// The leading result columns kept in view, by name. SQL sets the column
    /// order, so only the part still leading the result is frozen.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub frozen_columns: Vec<String>,
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

/// Keeps rows whose target renders as `value`, or with `exclude` removes them.
/// Values are compared as DuckDB renders them with `CAST(... AS VARCHAR)`, which is
/// also how the grid displays cells, so any displayed value can be matched exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueFilter {
    pub target: Dimension,
    /// `None` matches SQL NULL.
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude: bool,
}

impl ValueFilter {
    pub fn label(&self) -> String {
        let target = match &self.target {
            Dimension::Column(name) => name.clone(),
            Dimension::Expression { sql } => crate::domain::aggregation_sql::dimension_alias(sql)
                .unwrap_or_else(|| sql.trim().to_owned()),
        };
        let operator = if self.exclude { "≠" } else { "=" };
        let value = match &self.value {
            Some(value) => crate::domain::formatting::truncate(value, 40),
            None => "NULL".to_owned(),
        };
        format!("{target} {operator} {value}")
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

    pub fn label(&self) -> String {
        match (&self.kind, &self.catalog) {
            (RelationKind::Function, _) => format!("{}()", self.name),
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_view: Option<DatabaseView>,
    #[serde(default)]
    pub slicers: Vec<SlicerDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<ValueFilter>,
    #[serde(default)]
    pub actions: Vec<ViewAction>,
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
            slicers: Vec::new(),
            filters: Vec::new(),
            actions: Vec::new(),
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

    pub fn validate(&self) -> Result<(), String> {
        if let Some(view) = &self.database_view {
            if view.name.trim().is_empty()
                || (view.kind != RelationKind::Function && view.schema.trim().is_empty())
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
