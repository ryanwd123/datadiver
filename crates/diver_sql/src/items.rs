//! Completion items: expectations turned into labels through the catalog,
//! ranked and quoted. Filtering by the typed prefix is the editor's job.

use rowan::TextRange;

use crate::catalog::{Catalog, ColumnInfo, TableKind};
use crate::complete::{CompletionContext, Expected, FunctionKind, Name, StringNameKind, eq_fold};
use crate::document::{DocumentObject, ObjectKind};
use crate::scope::{ColumnSource, ColumnType, Columns, RelationSource, VisibleRelation, resolve_in};
use crate::{Kw, KwCategory, SyntaxKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Keyword,
    Column,
    Alias,
    Cte,
    Table,
    View,
    Schema,
    Database,
    Function,
    TableFunction,
    Type,
    Setting,
    Variable,
    Sequence,
    Extension,
    Pragma,
    File,
    Folder,
    Operator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: ItemKind,
    /// Quoted when needed: `"Order Date"`, `"select"`.
    pub insert_text: String,
    pub replace_range: TextRange,
    /// Column type, table of origin, signature.
    pub detail: Option<String>,
    pub documentation: Option<String>,
    /// Lower first.
    pub sort_key: u32,
    /// A function at a place it is called, which takes its `(…)` next. Not
    /// a name that DROP or COMMENT ON refers to.
    pub call: bool,
}

/// Rank bands: in-scope columns, then aliases, CTEs and tables, then
/// functions, then keywords.
mod rank {
    /// What a string argument names, as nothing else fits it as well.
    pub const STRING_NAME: u32 = 50;
    pub const COLUMN: u32 = 100;
    pub const ALIAS: u32 = 200;
    pub const CTE: u32 = 300;
    pub const TABLE: u32 = 350;
    pub const SCHEMA: u32 = 400;
    pub const DATABASE: u32 = 450;
    pub const FUNCTION: u32 = 500;
    pub const OTHER: u32 = 600;
    pub const KEYWORD: u32 = 900;
}

/// How identifier items are inserted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quoting {
    /// Quoted only when DuckDB needs it: `amount`, `"Order Date"`.
    #[default]
    WhenNeeded,
    /// Database, schema, table and column names are always quoted: `"amount"`.
    Always,
}

/// Words that never stand alone: offered only as part of their phrase.
const PHRASE_ONLY: &[Kw] = &[Kw::Group, Kw::Order, Kw::Partition];

pub fn items(
    ctx: &CompletionContext,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
) -> Vec<CompletionItem> {
    items_with(ctx, catalog, objects, Quoting::default())
}

pub fn items_with(
    ctx: &CompletionContext,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    quoting: Quoting,
) -> Vec<CompletionItem> {
    let mut out = Items {
        ctx,
        quoting,
        out: Vec::new(),
    };
    for expected in &ctx.expected {
        match expected {
            Expected::Keyword(kw) => {
                let has_phrase = ctx
                    .expected
                    .iter()
                    .any(|e| matches!(e, Expected::Phrase(p) if p[0] == *kw));
                if !(PHRASE_ONLY.contains(kw) && has_phrase) {
                    out.keyword(&kw.text().to_ascii_uppercase());
                }
            }
            Expected::Phrase(words) => {
                let text: Vec<String> = words
                    .iter()
                    .map(|w| w.text().to_ascii_uppercase())
                    .collect();
                out.keyword(&text.join(" "));
            }
            Expected::Punct(kind) => {
                if *kind == SyntaxKind::STAR {
                    out.push("*", ItemKind::Operator, "*".into(), None, rank::COLUMN - 1);
                }
            }
            Expected::DatabaseName => {
                for db in catalog.databases() {
                    out.name(&db, ItemKind::Database, None, rank::DATABASE);
                }
            }
            Expected::SchemaName { database } => {
                let db = database.as_ref().map(|d| d.text.as_str());
                if db.is_none_or(|d| catalog.databases().iter().any(|x| eq_fold(x, d))) {
                    for schema in catalog.schemas(db) {
                        out.name(&schema, ItemKind::Schema, None, rank::SCHEMA);
                    }
                }
                for o in objects
                    .iter()
                    .filter(|o| o.kind == ObjectKind::Schema && database.is_none())
                {
                    out.name(&o.name().text, ItemKind::Schema, None, rank::SCHEMA);
                }
            }
            Expected::TableRef { qualifier } => out.tables(qualifier, catalog, objects),
            Expected::CteName => {
                for cte in &ctx.scope.ctes {
                    out.name(&cte.text, ItemKind::Cte, Some("CTE".into()), rank::CTE);
                }
            }
            Expected::ColumnRef { qualifier } => out.columns(qualifier, catalog, objects),
            Expected::FunctionName { kind, qualifier } => {
                out.functions(*kind, qualifier, catalog, objects)
            }
            Expected::TypeName => {
                for t in ctx.dialect.types().iter().map(|t| t.to_string()).chain(catalog.types()) {
                    out.push(&t, ItemKind::Type, t.clone(), None, rank::OTHER);
                }
                for o in objects.iter().filter(|o| o.kind == ObjectKind::Type) {
                    out.name(&o.name().text, ItemKind::Type, None, rank::OTHER);
                }
            }
            Expected::SettingName => {
                for s in catalog.settings() {
                    out.push(
                        &s.name,
                        ItemKind::Setting,
                        s.name.clone(),
                        s.description.clone(),
                        rank::OTHER,
                    );
                }
            }
            Expected::ExtensionName => {
                for e in catalog.extensions() {
                    out.push(&e, ItemKind::Extension, e.clone(), None, rank::OTHER);
                }
            }
            Expected::PragmaName => {
                for name in ctx.dialect.pragmas().iter().map(|p| p.to_string()).chain(catalog.pragmas()) {
                    out.push(&name, ItemKind::Pragma, name.clone(), None, rank::OTHER);
                }
            }
            Expected::StringName(kind) => out.string_names(*kind, catalog, objects),
            Expected::FilePath => out.paths(catalog),
            Expected::NewName(_) | Expected::Expr => {}
        }
    }
    let mut items = out.out;
    // one item per label: the best-ranked one
    items.sort_by_key(|i| i.sort_key);
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.label.clone()));
    items
}

struct Items<'a> {
    ctx: &'a CompletionContext,
    quoting: Quoting,
    out: Vec<CompletionItem>,
}

impl Items<'_> {
    fn push(
        &mut self,
        label: &str,
        kind: ItemKind,
        insert_text: String,
        detail: Option<String>,
        sort_key: u32,
    ) {
        self.out.push(CompletionItem {
            label: label.to_string(),
            kind,
            insert_text,
            replace_range: self.ctx.replace_range,
            detail,
            documentation: None,
            sort_key,
            call: false,
        });
    }

    fn keyword(&mut self, text: &str) {
        self.push(
            text,
            ItemKind::Keyword,
            text.to_string(),
            None,
            rank::KEYWORD,
        );
    }

    /// An identifier item, quoted when it has to be.
    fn name(&mut self, name: &str, kind: ItemKind, detail: Option<String>, sort_key: u32) {
        let always = self.quoting == Quoting::Always
            && matches!(
                kind,
                ItemKind::Database
                    | ItemKind::Schema
                    | ItemKind::Table
                    | ItemKind::View
                    | ItemKind::Cte
                    | ItemKind::Column
            );
        let insert = if self.ctx.quoted || always {
            quote(name)
        } else {
            quote_if_needed(name)
        };
        let label = quote_if_needed(name);
        self.push(&label, kind, insert, detail, sort_key);
    }

    fn tables(&mut self, qualifier: &[Name], catalog: &dyn Catalog, objects: &[DocumentObject]) {
        let add = |this: &mut Self, list: Vec<crate::catalog::TableInfo>| {
            for t in list {
                let kind = if t.kind == TableKind::View {
                    ItemKind::View
                } else {
                    ItemKind::Table
                };
                this.name(&t.name, kind, t.schema.clone(), rank::TABLE);
            }
        };
        match qualifier {
            [] => add(self, catalog.tables(None, None)),
            [x] => {
                add(self, catalog.tables(None, Some(&x.text)));
                if catalog.databases().iter().any(|d| eq_fold(d, &x.text)) {
                    add(self, catalog.tables(Some(&x.text), None));
                }
            }
            [db, schema] => add(self, catalog.tables(Some(&db.text), Some(&schema.text))),
            _ => {}
        }
        for o in objects {
            let kind = match o.kind {
                ObjectKind::Table => ItemKind::Table,
                ObjectKind::View => ItemKind::View,
                _ => continue,
            };
            let (schema, name) = o.path.split_at(o.path.len() - 1);
            let schema_ok = match (qualifier, schema) {
                ([], []) => true,
                ([], [s]) => s.matches("main"),
                ([q], [s]) => s.matches(&q.text),
                ([q], [d, _]) => d.matches(&q.text),
                ([q1, q2], [d, s]) => d.matches(&q1.text) && s.matches(&q2.text),
                _ => false,
            };
            if schema_ok {
                self.name(
                    &name[0].text,
                    kind,
                    Some("in this script".into()),
                    rank::TABLE,
                );
            }
        }
    }

    fn columns(&mut self, qualifier: &[Name], catalog: &dyn Catalog, objects: &[DocumentObject]) {
        let scope = &self.ctx.scope;
        if qualifier.is_empty() {
            let mut relations: Vec<&VisibleRelation> = scope.relations.iter().collect();
            relations.sort_by_key(|r| r.depth);
            for r in relations {
                let detail = r.name().map(|n| n.text);
                for c in relation_columns(r, catalog, objects, 0) {
                    self.name(
                        &c.name,
                        ItemKind::Column,
                        c.data_type.or(detail.clone()),
                        rank::COLUMN + 10 * r.depth.min(9),
                    );
                }
            }
            for c in catalog.implicit_columns() {
                self.name(&c.name, ItemKind::Column, c.data_type, rank::COLUMN);
            }
            for alias in &scope.aliases {
                self.name(
                    &alias.text,
                    ItemKind::Alias,
                    Some("alias".into()),
                    rank::ALIAS,
                );
            }
            for r in &scope.relations {
                if let Some(name) = r.name() {
                    let kind = if matches!(r.source, RelationSource::Cte { .. }) {
                        ItemKind::Cte
                    } else {
                        ItemKind::Alias
                    };
                    self.name(&name.text, kind, None, rank::ALIAS + 10);
                }
            }
            return;
        }
        let relations = scope.resolve(qualifier);
        if relations.is_empty() {
            // `s.st.|` or `st.|`: the fields of a struct column.
            let data_type = column_type(&scope.relations, qualifier, catalog, objects, 0).or_else(|| {
                let (column, fields) = qualifier.split_first()?;
                let implicit = catalog.implicit_columns();
                let c = implicit.iter().find(|c| column.matches(&c.name))?;
                field_type(c.data_type.clone()?, fields)
            });
            for field in data_type.as_deref().and_then(struct_fields).unwrap_or_default() {
                self.name(&field.name, ItemKind::Column, field.data_type, rank::COLUMN);
            }
            return;
        }
        for r in relations {
            let detail = r.name().map(|n| n.text);
            for c in relation_columns(r, catalog, objects, 0) {
                self.name(
                    &c.name,
                    ItemKind::Column,
                    c.data_type.or(detail.clone()),
                    rank::COLUMN,
                );
            }
        }
    }

    fn functions(
        &mut self,
        kind: FunctionKind,
        qualifier: &[Name],
        catalog: &dyn Catalog,
        objects: &[DocumentObject],
    ) {
        if qualifier.len() > 1 {
            return;
        }
        for f in catalog.functions() {
            let kind_ok = match kind {
                FunctionKind::Table => f.kind == FunctionKind::Table,
                FunctionKind::Any => f.kind != FunctionKind::Table,
                FunctionKind::Macro => matches!(f.kind, FunctionKind::Macro | FunctionKind::Table),
                k => f.kind == k,
            };
            let schema_ok = match (qualifier, &f.schema) {
                ([], None) => true,
                ([], Some(s)) => {
                    s.eq_ignore_ascii_case("main") || s.eq_ignore_ascii_case("pg_catalog")
                }
                ([q], Some(s)) => q.matches(s),
                ([_], None) => false,
                _ => false,
            };
            if kind_ok && schema_ok {
                let item_kind = if f.kind == FunctionKind::Table {
                    ItemKind::TableFunction
                } else {
                    ItemKind::Function
                };
                let detail = f.signatures.first().cloned();
                let insert = function_name(&f.name);
                self.out.push(CompletionItem {
                    label: f.name.clone(),
                    kind: item_kind,
                    insert_text: insert,
                    replace_range: self.ctx.replace_range,
                    detail,
                    documentation: f.description.clone(),
                    sort_key: rank::FUNCTION,
                    // `current_date` and its like take no parentheses.
                    call: kind != FunctionKind::Macro && f.signatures.first().is_none_or(|s| s.contains('(')),
                });
            }
        }
        for o in objects {
            let ok = match o.kind {
                ObjectKind::TableMacro => matches!(kind, FunctionKind::Table | FunctionKind::Macro),
                ObjectKind::Macro => kind != FunctionKind::Table,
                _ => false,
            };
            if ok && qualifier.len() + 1 >= o.path.len() {
                let item_kind = if o.kind == ObjectKind::TableMacro {
                    ItemKind::TableFunction
                } else {
                    ItemKind::Function
                };
                self.name(
                    &o.name().text,
                    item_kind,
                    Some("in this script".into()),
                    rank::FUNCTION,
                );
                if let Some(item) = self.out.last_mut() {
                    item.call = kind != FunctionKind::Macro;
                }
            }
        }
    }

    /// Variables, sequences or settings, inserted into the string the cursor
    /// is in, or as a string literal where one may start.
    fn string_names(&mut self, kind: StringNameKind, catalog: &dyn Catalog, objects: &[DocumentObject]) {
        let mut names: Vec<(String, Option<String>)> = match kind {
            StringNameKind::Variable => catalog
                .variables()
                .into_iter()
                .map(|v| (v.name, v.data_type))
                .collect(),
            StringNameKind::Sequence => catalog.sequences().into_iter().map(|s| (s, None)).collect(),
            StringNameKind::Setting => catalog
                .settings()
                .into_iter()
                .map(|s| (s.name, s.description))
                .collect(),
        };
        let own = match kind {
            StringNameKind::Variable => Some(ObjectKind::Variable),
            StringNameKind::Sequence => Some(ObjectKind::Sequence),
            StringNameKind::Setting => None,
        };
        for o in objects.iter().filter(|o| Some(o.kind) == own) {
            let path: Vec<&str> = o.path.iter().map(|n| n.text.as_str()).collect();
            names.push((path.join("."), Some("in this script".into())));
        }
        let item_kind = match kind {
            StringNameKind::Variable => ItemKind::Variable,
            StringNameKind::Sequence => ItemKind::Sequence,
            StringNameKind::Setting => ItemKind::Setting,
        };
        for (name, detail) in names {
            let escaped = name.replace('\'', "''");
            let insert = if self.ctx.in_string.is_some() {
                escaped
            } else {
                format!("'{escaped}'")
            };
            self.push(&name, item_kind, insert, detail, rank::STRING_NAME);
        }
    }

    fn paths(&mut self, catalog: &dyn Catalog) {
        if self.ctx.in_string.is_none() {
            return;
        }
        for p in catalog.paths(&self.ctx.prefix) {
            let kind = if p.is_dir {
                ItemKind::Folder
            } else {
                ItemKind::File
            };
            let sort_key = if p.is_dir {
                rank::OTHER - 1
            } else {
                rank::OTHER
            };
            self.push(&p.path, kind, p.path.clone(), None, sort_key);
        }
    }
}

/// The columns of a relation: inferred ones, the catalog's, or a script's own table's.
pub fn relation_columns(
    relation: &VisibleRelation,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    depth: u32,
) -> Vec<ColumnInfo> {
    columns_of(&relation.source, &relation.columns, catalog, objects, depth)
}

fn columns_of(
    source: &RelationSource,
    columns: &Columns,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    depth: u32,
) -> Vec<ColumnInfo> {
    if depth > 8 {
        return Vec::new();
    }
    match columns {
        Columns::Unknown => Vec::new(),
        Columns::FromCatalog => {
            if let RelationSource::Table { path } = source
                && let Some(o) = objects.iter().find(|o| o.matches_path(path))
            {
                let source = RelationSource::Table {
                    path: o.path.clone(),
                };
                if o.columns != Columns::FromCatalog {
                    return columns_of(&source, &o.columns, catalog, objects, depth + 1);
                }
            }
            if let RelationSource::TableFunction { name, .. } = source
                && let Some(o) = objects
                    .iter()
                    .find(|o| o.kind == ObjectKind::TableMacro && o.matches_path(name))
            {
                return columns_of(source, &o.columns, catalog, objects, depth + 1);
            }
            catalog.columns(source).unwrap_or_default()
        }
        Columns::Known(list) => {
            let mut out = Vec::new();
            for c in list {
                match c {
                    ColumnSource::Named(n) => out.push(ColumnInfo {
                        name: n.text.clone(),
                        data_type: None,
                    }),
                    ColumnSource::Typed { name, data_type } => out.push(ColumnInfo {
                        name: name.text.clone(),
                        data_type: resolve_type(data_type, catalog, objects, depth + 1),
                    }),
                    ColumnSource::Unnamed(_) => {}
                    ColumnSource::Star {
                        relations,
                        exclude,
                        rename,
                        skip,
                    } => {
                        let mut cols: Vec<ColumnInfo> = relations
                            .iter()
                            .flat_map(|r| {
                                columns_of(&r.source, &r.columns, catalog, objects, depth + 1)
                            })
                            .filter(|c| !exclude.iter().any(|e| e.matches(&c.name)))
                            .collect();
                        for c in &mut cols {
                            if let Some((_, new)) =
                                rename.iter().find(|(old, _)| old.matches(&c.name))
                            {
                                c.name = new.text.clone();
                            }
                        }
                        out.extend(cols.into_iter().skip(*skip));
                    }
                }
            }
            out
        }
    }
}

fn resolve_type(
    data_type: &ColumnType,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    depth: u32,
) -> Option<String> {
    match data_type {
        ColumnType::Text(text) => Some(text.clone()),
        ColumnType::Of { relations, path } => column_type(relations, path, catalog, objects, depth),
    }
}

/// The type of what `path` names among `relations`: `t.col`, `col`, or a
/// struct field below either, as `t.col.field`. A relation qualifier is
/// tried before a column of the same name, as DuckDB binds them.
pub fn column_type(
    relations: &[VisibleRelation],
    path: &[Name],
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    depth: u32,
) -> Option<String> {
    if depth > 8 {
        return None;
    }
    for split in (0..path.len().min(4)).rev() {
        let (qualifier, rest) = path.split_at(split);
        let Some((column, fields)) = rest.split_first() else {
            continue;
        };
        let candidates: Vec<&VisibleRelation> = if qualifier.is_empty() {
            let mut all: Vec<&VisibleRelation> = relations.iter().collect();
            all.sort_by_key(|r| r.depth);
            all
        } else {
            resolve_in(relations, qualifier)
        };
        for r in candidates {
            if let Some(found) = find_column(&r.source, &r.columns, &column.text, catalog, objects, depth) {
                return field_type(found?, fields);
            }
        }
    }
    None
}

/// The type of column `name` of a relation: `None` when it has no such
/// column, `Some(None)` when it has one of an unknown type. Only the column
/// asked for is looked into, so a chain of CTEs costs one lookup per level.
fn find_column(
    source: &RelationSource,
    columns: &Columns,
    name: &str,
    catalog: &dyn Catalog,
    objects: &[DocumentObject],
    depth: u32,
) -> Option<Option<String>> {
    if depth > 8 {
        return None;
    }
    let Columns::Known(list) = columns else {
        let all = columns_of(source, columns, catalog, objects, depth + 1);
        return all.into_iter().find(|c| eq_fold(&c.name, name)).map(|c| c.data_type);
    };
    for c in list {
        match c {
            ColumnSource::Named(n) if n.matches(name) => return Some(None),
            ColumnSource::Typed { name: n, data_type } if n.matches(name) => {
                return Some(resolve_type(data_type, catalog, objects, depth + 1));
            }
            ColumnSource::Star { skip, .. } if *skip > 0 => {
                let one = Columns::Known(vec![c.clone()]);
                if let Some(found) = columns_of(source, &one, catalog, objects, depth + 1)
                    .into_iter()
                    .find(|c| eq_fold(&c.name, name))
                {
                    return Some(found.data_type);
                }
            }
            ColumnSource::Star {
                relations,
                exclude,
                rename,
                ..
            } => {
                let original = match rename.iter().find(|(_, new)| new.matches(name)) {
                    Some((old, _)) => old.text.as_str(),
                    // renamed to something else, so not here under this name
                    None if rename.iter().any(|(old, _)| old.matches(name)) => continue,
                    None => name,
                };
                if exclude.iter().any(|e| e.matches(original)) {
                    continue;
                }
                for r in relations {
                    if let Some(found) = find_column(&r.source, &r.columns, original, catalog, objects, depth + 1) {
                        return Some(found);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// The type of the struct field `fields` names below a column of type `data_type`.
fn field_type(data_type: String, fields: &[Name]) -> Option<String> {
    let mut data_type = data_type;
    for field in fields {
        data_type = struct_fields(&data_type)?
            .into_iter()
            .find(|f| field.matches(&f.name))?
            .data_type?;
    }
    Some(data_type)
}

/// The fields of a struct type, as DuckDB writes it:
/// `STRUCT(a INTEGER, "b c" STRUCT(d VARCHAR))`. Fields may leave out their
/// type. Not a struct, or a list of structs: `None`.
pub fn struct_fields(data_type: &str) -> Option<Vec<ColumnInfo>> {
    let text = data_type.trim();
    let keyword = text.get(..6)?;
    if !keyword.eq_ignore_ascii_case("struct") {
        return None;
    }
    let inner = text[6..].trim_start().strip_prefix('(')?.strip_suffix(')')?;
    let mut parts = Vec::new();
    let (mut depth, mut quoted, mut start) = (0i32, false, 0);
    for (i, ch) in inner.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            ',' if !quoted && depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 || quoted {
        return None;
    }
    parts.push(&inner[start..]);
    let mut fields = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, rest) = if let Some(after) = part.strip_prefix('"') {
            let mut name = String::new();
            let mut chars = after.char_indices().peekable();
            let mut end = after.len();
            while let Some((i, ch)) = chars.next() {
                if ch == '"' {
                    if chars.peek().is_some_and(|(_, next)| *next == '"') {
                        chars.next();
                        name.push('"');
                        continue;
                    }
                    end = i + 1;
                    break;
                }
                name.push(ch);
            }
            (name, &after[end..])
        } else {
            let end = part.find(char::is_whitespace).unwrap_or(part.len());
            (part[..end].to_string(), &part[end..])
        };
        let rest = rest.trim();
        fields.push(ColumnInfo {
            name,
            data_type: (!rest.is_empty()).then(|| rest.to_string()),
        });
    }
    Some(fields)
}

/// Whether DuckDB needs `name` quoted: anything but plain words, and reserved
/// words. Case does not count, as DuckDB matches unquoted names ignoring case
/// and keeps the case they are written in.
pub fn needs_quotes(name: &str) -> bool {
    let mut chars = name.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    !plain
        || Kw::lookup(name).is_some_and(|kw| {
            kw.category() != KwCategory::Unreserved && kw.category() != KwCategory::Contextual
        })
}

/// A function's name as a call writes it. Keywords that name functions,
/// like `nullif`, `coalesce` and `left`, are called bare, and quoting those
/// the grammar reads itself would not find them.
fn function_name(name: &str) -> String {
    let reserved = Kw::lookup(name).is_some_and(|kw| kw.category() == KwCategory::Reserved);
    if reserved { quote(name) } else { quote_if_needed_ignoring_keywords(name) }
}

fn quote_if_needed_ignoring_keywords(name: &str) -> String {
    let mut chars = name.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain { name.to_string() } else { quote(name) }
}

pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn quote_if_needed(name: &str) -> String {
    if needs_quotes(name) {
        quote(name)
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_fields_read_duckdb_struct_types() {
        let fields = struct_fields("STRUCT(a INTEGER, \"b, \"\"c\"\"\" STRUCT(d VARCHAR, e INTEGER[]), f)").unwrap();
        let fields: Vec<(&str, Option<&str>)> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.data_type.as_deref()))
            .collect();
        assert_eq!(
            fields,
            [
                ("a", Some("INTEGER")),
                ("b, \"c\"", Some("STRUCT(d VARCHAR, e INTEGER[])")),
                ("f", None)
            ]
        );
        assert_eq!(struct_fields("struct (x DOUBLE)").unwrap()[0].name, "x");
        assert_eq!(struct_fields("STRUCT(a INTEGER)[]"), None);
        assert_eq!(struct_fields("STRUCT(a INTEGER)[3]"), None);
        assert_eq!(struct_fields("INTEGER"), None);
        assert_eq!(struct_fields("MAP(VARCHAR, INTEGER)"), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_if_needed("amount"), "amount");
        assert_eq!(quote_if_needed("order date"), "\"order date\"");
        assert_eq!(quote_if_needed("select"), "\"select\"");
        assert_eq!(quote_if_needed("year"), "year");
        assert_eq!(quote_if_needed("Mixed"), "Mixed");
        assert_eq!(quote_if_needed("ID"), "ID");
        assert_eq!(quote_if_needed("SELECT"), "\"SELECT\"");
        assert_eq!(quote_if_needed("x\"y"), "\"x\"\"y\"");
        assert_eq!(quote_if_needed("1st"), "\"1st\"");
    }

    #[test]
    fn always_quoting_quotes_names_but_not_keywords_or_functions() {
        let catalog = crate::EmptyCatalog;
        let text = "SELECT  FROM t";
        let ctx = crate::complete(text, 7.into());
        let objects = crate::document_objects(&crate::parse("CREATE TABLE t(amount INT)"));
        let items = items_with(&ctx, &catalog, &objects, Quoting::Always);
        let insert = |label: &str| items.iter().find(|i| i.label == label).map(|i| i.insert_text.clone());
        assert_eq!(insert("amount").as_deref(), Some("\"amount\""));
        assert_eq!(insert("DISTINCT").as_deref(), Some("DISTINCT"));
        let items = super::items(&ctx, &catalog, &objects);
        assert_eq!(items.iter().find(|i| i.label == "amount").unwrap().insert_text, "amount");
    }
}
