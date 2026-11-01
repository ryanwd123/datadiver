//! What the app knows about the database: the `Catalog` trait completion
//! asks, and `MockCatalog` for tests.
//!
//! Calls must be fast; the app answers from caches and returns `None` for
//! columns it hasn't described yet.

use crate::complete::FunctionKind;
use crate::scope::RelationSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionInfo {
    pub name: String,
    pub schema: Option<String>,
    pub kind: FunctionKind,
    /// `sum(arg)`-style signatures.
    pub signatures: Vec<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingInfo {
    pub name: String,
    pub description: Option<String>,
}

/// A variable `SET VARIABLE` made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableInfo {
    pub name: String,
    pub data_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathInfo {
    /// What to insert in place of the typed path.
    pub path: String,
    pub is_dir: bool,
}

/// Everything has a default, so an implementation answers only what it knows.
pub trait Catalog {
    fn databases(&self) -> Vec<String> {
        Vec::new()
    }
    fn schemas(&self, _database: Option<&str>) -> Vec<String> {
        Vec::new()
    }
    /// Tables of a schema; `None` means the default (search path) schema.
    fn tables(&self, _database: Option<&str>, _schema: Option<&str>) -> Vec<TableInfo> {
        Vec::new()
    }
    /// Columns of a table, file or table function; `None` when unknown.
    fn columns(&self, _relation: &RelationSource) -> Option<Vec<ColumnInfo>> {
        None
    }
    /// Columns in scope when the text names no relation, such as the frame
    /// an expression editor (`Entry::Expression`) works on.
    fn implicit_columns(&self) -> Vec<ColumnInfo> {
        Vec::new()
    }
    fn functions(&self) -> Vec<FunctionInfo> {
        Vec::new()
    }
    /// Types beyond the dialect's built-in ones, such as user-defined types.
    fn types(&self) -> Vec<String> {
        Vec::new()
    }
    fn settings(&self) -> Vec<SettingInfo> {
        Vec::new()
    }
    fn extensions(&self) -> Vec<String> {
        Vec::new()
    }
    fn variables(&self) -> Vec<VariableInfo> {
        Vec::new()
    }
    /// Sequences by the name `nextval` takes: bare in the default schema,
    /// qualified elsewhere (`other.ser`).
    fn sequences(&self) -> Vec<String> {
        Vec::new()
    }
    /// Pragmas beyond the dialect's built-in ones.
    fn pragmas(&self) -> Vec<String> {
        Vec::new()
    }
    /// Paths completing `typed`, the string's text up to the cursor.
    fn paths(&self, _typed: &str) -> Vec<PathInfo> {
        Vec::new()
    }
}

/// A catalog that knows nothing.
pub struct EmptyCatalog;

impl Catalog for EmptyCatalog {}

/// DuckDB's built-in type names (`duckdb_types()`, primary names).
pub fn builtin_types() -> &'static [&'static str] {
    &[
        "BIGINT",
        "BIT",
        "BLOB",
        "BOOLEAN",
        "DATE",
        "DECIMAL",
        "DOUBLE",
        "FLOAT",
        "HUGEINT",
        "INTEGER",
        "INTERVAL",
        "JSON",
        "LIST",
        "MAP",
        "SMALLINT",
        "STRUCT",
        "TIME",
        "TIMESTAMP",
        "TIMESTAMP WITH TIME ZONE",
        "TIMESTAMP_MS",
        "TIMESTAMP_NS",
        "TIMESTAMP_S",
        "TINYINT",
        "UBIGINT",
        "UHUGEINT",
        "UINTEGER",
        "UNION",
        "USMALLINT",
        "UTINYINT",
        "UUID",
        "VARCHAR",
        "VARINT",
        "ENUM",
        "TIMETZ",
        "TIMESTAMPTZ",
        "BIGNUM",
    ]
}

/// The type names Polars SQL maps to Polars types (polars-sql 0.55, `types.rs`).
pub fn polars_types() -> &'static [&'static str] {
    &[
        "BIGINT",
        "BINARY",
        "BLOB",
        "BOOLEAN",
        "BYTEA",
        "CHAR",
        "DATE",
        "DATETIME",
        "DECIMAL",
        "DOUBLE",
        "FLOAT",
        "HUGEINT",
        "INT",
        "INTEGER",
        "INTERVAL",
        "NUMERIC",
        "REAL",
        "SMALLINT",
        "STRING",
        "TEXT",
        "TIME",
        "TIMESTAMP",
        "TINYINT",
        "UBIGINT",
        "UHUGEINT",
        "UINTEGER",
        "USMALLINT",
        "UTINYINT",
        "UUID",
        "VARBINARY",
        "VARCHAR",
    ]
}

/// Pragmas DuckDB has without extensions.
pub fn builtin_pragmas() -> &'static [&'static str] {
    &[
        "database_list",
        "database_size",
        "show_tables",
        "show_tables_expanded",
        "table_info",
        "show",
        "storage_info",
        "version",
        "platform",
        "user_agent",
        "metadata_info",
        "functions",
        "collations",
        "enable_profiling",
        "disable_profiling",
        "profiling_output",
        "enable_progress_bar",
        "disable_progress_bar",
        "threads",
        "memory_limit",
        "enable_checkpoint_on_shutdown",
        "disable_checkpoint_on_shutdown",
        "force_checkpoint",
        "verify_external",
        "disable_verification",
        "explain_output",
        "enable_optimizer",
        "disable_optimizer",
    ]
}

#[cfg(feature = "mock")]
pub use mock::MockCatalog;

#[cfg(feature = "mock")]
mod mock {
    use super::*;
    use crate::complete::eq_fold;
    use crate::scope::file_stem;

    /// An in-memory catalog: tables with columns, functions, settings, ...
    #[derive(Clone, Debug, Default)]
    pub struct MockCatalog {
        pub databases: Vec<String>,
        /// (database, schema)
        pub schemas: Vec<(String, String)>,
        /// (database, schema, info, columns)
        pub tables: Vec<(String, String, TableInfo, Vec<ColumnInfo>)>,
        pub functions: Vec<FunctionInfo>,
        /// Columns of table functions and table macros by name.
        pub table_function_columns: Vec<(String, Vec<String>)>,
        /// Columns of files by stem.
        pub file_columns: Vec<(String, Vec<String>)>,
        pub settings: Vec<SettingInfo>,
        pub extensions: Vec<String>,
        pub variables: Vec<VariableInfo>,
        pub sequences: Vec<String>,
        pub paths: Vec<PathInfo>,
        pub implicit: Vec<ColumnInfo>,
    }

    impl MockCatalog {
        /// A catalog with the `memory` database and its `main` schema.
        pub fn new() -> MockCatalog {
            MockCatalog {
                databases: vec!["memory".into()],
                schemas: vec![("memory".into(), "main".into())],
                ..MockCatalog::default()
            }
        }

        pub fn database(mut self, name: &str) -> Self {
            self.databases.push(name.into());
            self
        }

        pub fn schema(mut self, database: &str, schema: &str) -> Self {
            self.schemas.push((database.into(), schema.into()));
            self
        }

        /// A table in `memory.main`; columns are `"name type"` or `"name"`.
        pub fn table(self, name: &str, columns: &[&str]) -> Self {
            self.table_in("memory", "main", name, columns)
        }

        pub fn table_in(
            mut self,
            database: &str,
            schema: &str,
            name: &str,
            columns: &[&str],
        ) -> Self {
            self.add(database, schema, name, TableKind::Table, columns);
            self
        }

        pub fn view_in(
            mut self,
            database: &str,
            schema: &str,
            name: &str,
            columns: &[&str],
        ) -> Self {
            self.add(database, schema, name, TableKind::View, columns);
            self
        }

        fn add(
            &mut self,
            database: &str,
            schema: &str,
            name: &str,
            kind: TableKind,
            columns: &[&str],
        ) {
            let info = TableInfo {
                database: Some(database.into()),
                schema: Some(schema.into()),
                name: name.into(),
                kind,
            };
            let columns = columns
                .iter()
                .map(|c| {
                    // a quoted name may contain spaces
                    let (name, ty) = if let Some(rest) = c.strip_prefix('"') {
                        let end = rest.find('"').unwrap_or(rest.len());
                        (
                            rest[..end].to_string(),
                            rest.get(end + 1..).map(str::trim).filter(|t| !t.is_empty()),
                        )
                    } else {
                        match c.split_once(' ') {
                            Some((n, t)) => (n.to_string(), Some(t.trim())),
                            None => (c.to_string(), None),
                        }
                    };
                    ColumnInfo {
                        name,
                        data_type: ty.map(str::to_string),
                    }
                })
                .collect();
            self.tables
                .push((database.into(), schema.into(), info, columns));
        }

        pub fn function(mut self, name: &str, kind: FunctionKind) -> Self {
            self.functions.push(FunctionInfo {
                name: name.into(),
                schema: None,
                kind,
                signatures: vec![format!("{name}(...)")],
                description: None,
            });
            self
        }

        pub fn function_in(mut self, schema: &str, name: &str, kind: FunctionKind) -> Self {
            self.functions.push(FunctionInfo {
                name: name.into(),
                schema: Some(schema.into()),
                kind,
                signatures: Vec::new(),
                description: None,
            });
            self
        }

        pub fn table_function(mut self, name: &str, columns: &[&str]) -> Self {
            self = self.function(name, FunctionKind::Table);
            self.table_function_columns
                .push((name.into(), columns.iter().map(|c| c.to_string()).collect()));
            self
        }

        pub fn file(mut self, stem: &str, columns: &[&str]) -> Self {
            self.file_columns
                .push((stem.into(), columns.iter().map(|c| c.to_string()).collect()));
            self
        }

        pub fn setting(mut self, name: &str) -> Self {
            self.settings.push(SettingInfo {
                name: name.into(),
                description: None,
            });
            self
        }

        pub fn extension(mut self, name: &str) -> Self {
            self.extensions.push(name.into());
            self
        }

        pub fn variable(mut self, name: &str, data_type: &str) -> Self {
            self.variables.push(VariableInfo {
                name: name.into(),
                data_type: Some(data_type.into()),
            });
            self
        }

        pub fn sequence(mut self, name: &str) -> Self {
            self.sequences.push(name.into());
            self
        }

        /// Columns of the frame an expression is written against.
        pub fn implicit_columns(mut self, columns: &[&str]) -> Self {
            self.implicit.extend(columns.iter().map(|c| ColumnInfo {
                name: c.to_string(),
                data_type: None,
            }));
            self
        }

        pub fn path(mut self, path: &str, is_dir: bool) -> Self {
            self.paths.push(PathInfo {
                path: path.into(),
                is_dir,
            });
            self
        }

        fn find_table(&self, path: &[crate::complete::Name]) -> Option<&Vec<ColumnInfo>> {
            let (db, schema, name) = match path {
                [t] => (None, None, t),
                [s, t] => (None, Some(s), t),
                [d, s, t] => (Some(d), Some(s), t),
                _ => return None,
            };
            let hit = |default_only: bool| {
                self.tables.iter().find(|(d, s, info, _)| {
                    info.name.eq_ignore_ascii_case(&name.text)
                        && db.is_none_or(|x| eq_fold(d, &x.text))
                        && match schema {
                            Some(x) => {
                                eq_fold(s, &x.text)
                                    || (db.is_none() && eq_fold(d, &x.text) && s == "main")
                            }
                            None => !default_only || s == "main",
                        }
                })
            };
            hit(true).or_else(|| hit(false)).map(|(_, _, _, cols)| cols)
        }
    }

    fn named(cols: &[String]) -> Vec<ColumnInfo> {
        cols.iter()
            .map(|c| ColumnInfo {
                name: c.clone(),
                data_type: None,
            })
            .collect()
    }

    impl Catalog for MockCatalog {
        fn databases(&self) -> Vec<String> {
            self.databases.clone()
        }

        fn schemas(&self, database: Option<&str>) -> Vec<String> {
            let db = database.unwrap_or("memory");
            self.schemas
                .iter()
                .filter(|(d, _)| eq_fold(d, db))
                .map(|(_, s)| s.clone())
                .collect()
        }

        fn tables(&self, database: Option<&str>, schema: Option<&str>) -> Vec<TableInfo> {
            let db = database.unwrap_or("memory");
            let schema = schema.unwrap_or("main");
            self.tables
                .iter()
                .filter(|(d, s, _, _)| eq_fold(d, db) && eq_fold(s, schema))
                .map(|(_, _, info, _)| info.clone())
                .collect()
        }

        fn columns(&self, relation: &RelationSource) -> Option<Vec<ColumnInfo>> {
            match relation {
                RelationSource::Table { path } => self.find_table(path).cloned(),
                RelationSource::TableFunction { name, .. } => {
                    let last = name.last()?;
                    self.table_function_columns
                        .iter()
                        .find(|(n, _)| last.matches(n))
                        .map(|(_, c)| named(c))
                }
                RelationSource::File { path } => {
                    let stem = file_stem(path);
                    self.file_columns
                        .iter()
                        .find(|(n, _)| eq_fold(n, &stem))
                        .map(|(_, c)| named(c))
                }
                _ => None,
            }
        }

        fn implicit_columns(&self) -> Vec<ColumnInfo> {
            self.implicit.clone()
        }

        fn functions(&self) -> Vec<FunctionInfo> {
            self.functions.clone()
        }

        fn settings(&self) -> Vec<SettingInfo> {
            self.settings.clone()
        }

        fn extensions(&self) -> Vec<String> {
            self.extensions.clone()
        }

        fn variables(&self) -> Vec<VariableInfo> {
            self.variables.clone()
        }

        fn sequences(&self) -> Vec<String> {
            self.sequences.clone()
        }

        fn paths(&self, typed: &str) -> Vec<PathInfo> {
            self.paths
                .iter()
                .filter(|p| p.path.starts_with(typed))
                .cloned()
                .collect()
        }
    }
}
