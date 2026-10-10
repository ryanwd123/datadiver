#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    Nanoseconds,
    Microseconds,
    Milliseconds,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Boolean,
    String,
    Int8,
    Int16,
    Int32,
    Int64,
    Int128,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    UInt128,
    Float32,
    Float64,
    Date,
    Datetime(TimeUnit, Option<String>),
    Time,
    Duration(TimeUnit),
    List,
    Struct,
    Other,
}

impl ColumnType {
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Self::Int8
                | Self::Int16
                | Self::Int32
                | Self::Int64
                | Self::Int128
                | Self::UInt8
                | Self::UInt16
                | Self::UInt32
                | Self::UInt64
                | Self::UInt128
                | Self::Float32
                | Self::Float64
        )
    }
}

pub fn from_duckdb(data_type: &str) -> ColumnType {
    let kind = data_type.to_ascii_uppercase();
    if kind.starts_with("BOOLEAN") {
        ColumnType::Boolean
    } else if kind.starts_with("UTINYINT") {
        ColumnType::UInt8
    } else if kind.starts_with("USMALLINT") {
        ColumnType::UInt16
    } else if kind.starts_with("UINTEGER") {
        ColumnType::UInt32
    } else if kind.starts_with("UBIGINT") {
        ColumnType::UInt64
    } else if kind.starts_with("UHUGEINT") {
        ColumnType::UInt128
    } else if kind.starts_with("TINYINT") {
        ColumnType::Int8
    } else if kind.starts_with("SMALLINT") {
        ColumnType::Int16
    } else if kind.starts_with("INTEGER") {
        ColumnType::Int32
    } else if kind.starts_with("BIGINT") {
        ColumnType::Int64
    } else if kind.starts_with("HUGEINT") {
        ColumnType::Int128
    } else if kind.starts_with("FLOAT") {
        ColumnType::Float32
    } else if kind.starts_with("DOUBLE") || kind.starts_with("DECIMAL") {
        ColumnType::Float64
    } else if kind.starts_with("DATE") {
        ColumnType::Date
    } else if kind.starts_with("TIMESTAMP") {
        ColumnType::Datetime(TimeUnit::Microseconds, None)
    } else if kind.starts_with("TIME") {
        ColumnType::Time
    } else if kind.starts_with("INTERVAL") {
        ColumnType::Duration(TimeUnit::Microseconds)
    } else if kind.starts_with("LIST") || kind.starts_with("ARRAY") {
        ColumnType::List
    } else if kind.starts_with("STRUCT") || kind.starts_with("MAP") {
        ColumnType::Struct
    } else if kind.starts_with("VARCHAR") || kind.starts_with("CHAR") || kind.starts_with("ENUM") {
        ColumnType::String
    } else {
        ColumnType::Other
    }
}
