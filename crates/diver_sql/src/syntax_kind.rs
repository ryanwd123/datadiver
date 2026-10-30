//! Token and node kinds, and the rowan language they make up.
//!
//! Keyword kinds are generated from `keywords.csv` (see `tests/sourcegen.rs`);
//! everything else is listed here.

#[rustfmt::skip]
mod generated;

use std::fmt;

pub use generated::Kw;

/// The kind of a token or a node.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxKind(pub u16);

/// How freely DuckDB lets a keyword be used as a name.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum KwCategory {
    /// Never a bare name (`SELECT`, `FROM`).
    Reserved,
    /// Any name (`year`, `name`).
    Unreserved,
    /// A function or type name but not a column name (`LEFT`, `ILIKE`).
    TypeFunction,
    /// A column name but not a function or type name (`BETWEEN`, `VALUES`).
    ColumnName,
    /// A plain identifier to DuckDB that the grammar reads as a keyword in places.
    Contextual,
}

macro_rules! kinds {
    ($($name:ident),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u16)]
        enum Plain { $($name,)* __Last }

        impl SyntaxKind {
            $(pub const $name: SyntaxKind = SyntaxKind(Plain::$name as u16);)*
        }

        static PLAIN_NAMES: &[&str] = &[$(stringify!($name)),*];
        const PLAIN_COUNT: u16 = Plain::__Last as u16;
    };
}

kinds! {
    // trivia
    WHITESPACE,
    LINE_COMMENT,
    BLOCK_COMMENT,
    UNTERMINATED_BLOCK_COMMENT,

    // words and literals
    IDENT,
    QUOTED_IDENT,
    UNTERMINATED_QUOTED_IDENT,
    STRING,
    ESCAPE_STRING,
    DOLLAR_STRING,
    BLOB_STRING,
    UNTERMINATED_STRING,
    INT_NUMBER,
    FLOAT_NUMBER,
    PARAM,
    // the completion hole: the partial word under the cursor, or empty
    CURSOR,
    ERROR_TOKEN,
    EOF,

    // punctuation (keep L_PAREN first and NOT_TILDE_STAR last)
    L_PAREN,
    R_PAREN,
    L_BRACK,
    R_BRACK,
    L_CURLY,
    R_CURLY,
    COMMA,
    SEMICOLON,
    DOT,
    COLON,
    DOUBLE_COLON,
    COLON_EQ,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    SLASH2,
    PERCENT,
    CARET,
    STAR2,
    EQ,
    EQ2,
    NEQ,
    LT,
    GT,
    LTE,
    GTE,
    ARROW,
    LONG_ARROW,
    FAT_ARROW,
    PIPE,
    PIPE2,
    AMP,
    AMP2,
    TILDE,
    BANG,
    AT,
    HASH,
    AT_GT,
    LT_AT,
    SHL,
    SHR,
    CARET_AT,
    TILDE2,
    TILDE2_STAR,
    NOT_TILDE2,
    NOT_TILDE2_STAR,
    TILDE_STAR,
    NOT_TILDE,
    NOT_TILDE_STAR,

    // nodes
    SOURCE_FILE,
    ERROR,
    UNKNOWN_STMT,
    SELECT_STMT,
    EXPRESSION_ROOT,

    // queries
    QUERY,
    WITH_CLAUSE,
    CTE,
    MATERIALIZED_CLAUSE,
    SELECT_CORE,
    SELECT_CLAUSE,
    DISTINCT_CLAUSE,
    SELECT_ITEM,
    STAR_EXPR,
    EXCLUDE_CLAUSE,
    REPLACE_CLAUSE,
    REPLACE_ITEM,
    RENAME_CLAUSE,
    RENAME_ITEM,
    FROM_CLAUSE,
    WHERE_CLAUSE,
    GROUP_BY_CLAUSE,
    GROUPING_SET,
    HAVING_CLAUSE,
    WINDOW_CLAUSE,
    WINDOW_DEF,
    QUALIFY_CLAUSE,
    ORDER_BY_CLAUSE,
    ORDER_ITEM,
    LIMIT_CLAUSE,
    OFFSET_CLAUSE,
    SET_OP,
    PAREN_QUERY,
    VALUES_CLAUSE,
    VALUES_ROW,
    SAMPLE_CLAUSE,
    INTO_CLAUSE,

    // FROM items
    TABLE_REF,
    FILE_REF,
    TABLE_FUNCTION,
    SUBQUERY_REF,
    PAREN_JOIN,
    JOIN_EXPR,
    JOIN_ON,
    JOIN_USING,
    PIVOT_REF,
    UNPIVOT_REF,
    PIVOT_ON,
    PIVOT_USING,
    PIVOT_IN,
    ALIAS,
    COLUMN_ALIASES,

    // names
    NAME,
    PATH,
    NAME_LIST,

    // expressions
    LITERAL,
    COLUMN_REF,
    PARAM_EXPR,
    POSITIONAL_REF,
    BIN_EXPR,
    PREFIX_EXPR,
    POSTFIX_EXPR,
    PAREN_EXPR,
    ROW_EXPR,
    CALL_EXPR,
    ARG_LIST,
    NAMED_ARG,
    FILTER_CLAUSE,
    WITHIN_GROUP_CLAUSE,
    OVER_CLAUSE,
    WINDOW_SPEC,
    PARTITION_BY,
    FRAME_CLAUSE,
    FRAME_BOUND,
    CAST_EXPR,
    CASE_EXPR,
    WHEN_CLAUSE,
    ELSE_CLAUSE,
    IN_EXPR,
    BETWEEN_EXPR,
    LIKE_EXPR,
    IS_EXPR,
    EXISTS_EXPR,
    SUBQUERY_EXPR,
    QUANTIFIED_EXPR,
    LIST_EXPR,
    LIST_COMPREHENSION,
    STRUCT_EXPR,
    STRUCT_FIELD,
    MAP_EXPR,
    INDEX_EXPR,
    FIELD_EXPR,
    LAMBDA_EXPR,
    LAMBDA_PARAMS,
    INTERVAL_EXPR,
    COLLATE_EXPR,
    AT_TIME_ZONE_EXPR,
    SPECIAL_CALL,
    DEFAULT_EXPR,

    // types
    TYPE_NAME,
    TYPE_ARGS,
    STRUCT_TYPE_FIELD,

    // statements
    CREATE_STMT,
    DROP_STMT,
    ALTER_STMT,
    ALTER_ACTION,
    COLUMN_DEF,
    TABLE_CONSTRAINT,
    COLUMN_CONSTRAINT,
    TABLE_ELEMENTS,
    MACRO_PARAMS,
    MACRO_PARAM,
    INSERT_STMT,
    INSERT_COLUMNS,
    ON_CONFLICT_CLAUSE,
    RETURNING_CLAUSE,
    UPDATE_STMT,
    SET_CLAUSE,
    SET_ITEM,
    DELETE_STMT,
    USING_CLAUSE,
    MERGE_STMT,
    MERGE_WHEN,
    COPY_STMT,
    OPTION_LIST,
    OPTION,
    ATTACH_STMT,
    DETACH_STMT,
    USE_STMT,
    EXPORT_STMT,
    IMPORT_STMT,
    DESCRIBE_STMT,
    SHOW_STMT,
    SUMMARIZE_STMT,
    EXPLAIN_STMT,
    PRAGMA_STMT,
    SET_STMT,
    RESET_STMT,
    INSTALL_STMT,
    LOAD_STMT,
    TRANSACTION_STMT,
    CHECKPOINT_STMT,
    PREPARE_STMT,
    EXECUTE_STMT,
    DEALLOCATE_STMT,
    CALL_STMT,
    VACUUM_STMT,
    ANALYZE_STMT,
    COMMENT_STMT,
    TRUNCATE_STMT,
    PIVOT_STMT,
}

/// Keyword kinds start here, one per `Kw`, in `Kw` order.
const KW_BASE: u16 = 1024;
const _: () = assert!(PLAIN_COUNT < KW_BASE);

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE
                | SyntaxKind::LINE_COMMENT
                | SyntaxKind::BLOCK_COMMENT
                | SyntaxKind::UNTERMINATED_BLOCK_COMMENT
        )
    }

    pub fn is_comment(self) -> bool {
        self.is_trivia() && self != SyntaxKind::WHITESPACE
    }

    pub fn is_keyword(self) -> bool {
        self.0 >= KW_BASE && usize::from(self.0 - KW_BASE) < generated::KW_COUNT
    }

    pub fn as_kw(self) -> Option<Kw> {
        self.is_keyword()
            .then(|| generated::KW_ALL[usize::from(self.0 - KW_BASE)])
    }

    pub fn is_punct(self) -> bool {
        (SyntaxKind::L_PAREN.0..=SyntaxKind::NOT_TILDE_STAR.0).contains(&self.0)
    }

    /// String literals of every flavor, terminated or not.
    pub fn is_string(self) -> bool {
        matches!(
            self,
            SyntaxKind::STRING
                | SyntaxKind::ESCAPE_STRING
                | SyntaxKind::DOLLAR_STRING
                | SyntaxKind::BLOB_STRING
                | SyntaxKind::UNTERMINATED_STRING
        )
    }

    /// Identifiers, keywords and the completion hole: what a name can be made of.
    pub fn is_word(self) -> bool {
        matches!(
            self,
            SyntaxKind::IDENT | SyntaxKind::QUOTED_IDENT | SyntaxKind::UNTERMINATED_QUOTED_IDENT
        ) || self.is_keyword()
    }

    pub fn name(self) -> &'static str {
        if let Some(kw) = self.as_kw() {
            generated::KW_KIND_NAMES[kw as usize]
        } else {
            PLAIN_NAMES
                .get(usize::from(self.0))
                .copied()
                .unwrap_or("UNKNOWN_KIND")
        }
    }
}

impl From<Kw> for SyntaxKind {
    fn from(kw: Kw) -> SyntaxKind {
        SyntaxKind(KW_BASE + kw as u16)
    }
}

impl fmt::Debug for SyntaxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The longest keyword DuckDB has is well under this.
const KW_MAX_LEN: usize = 32;

impl Kw {
    /// Every keyword, in `Kw` order.
    pub fn all() -> &'static [Kw] {
        &generated::KW_ALL
    }

    /// Lowercase text.
    pub fn text(self) -> &'static str {
        generated::KW_TEXT[self as usize]
    }

    pub fn category(self) -> KwCategory {
        generated::KW_CATEGORY[self as usize]
    }

    pub fn is_reserved(self) -> bool {
        self.category() == KwCategory::Reserved
    }

    /// The keyword `word` spells, ignoring case.
    pub fn lookup(word: &str) -> Option<Kw> {
        let bytes = word.as_bytes();
        if bytes.is_empty() || bytes.len() > KW_MAX_LEN {
            return None;
        }
        let mut buf = [0u8; KW_MAX_LEN];
        for (dst, &b) in buf.iter_mut().zip(bytes) {
            if !b.is_ascii() {
                return None;
            }
            *dst = b.to_ascii_lowercase();
        }
        let key = &buf[..bytes.len()];
        generated::KW_BY_LEN
            .binary_search_by(|(text, _)| (text.len(), text.as_bytes()).cmp(&(key.len(), key)))
            .ok()
            .map(|i| generated::KW_BY_LEN[i].1)
    }
}

/// The rowan language of DuckDB SQL trees.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DuckSql {}

impl rowan::Language for DuckSql {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind.0)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<DuckSql>;
pub type SyntaxToken = rowan::SyntaxToken<DuckSql>;
pub type SyntaxElement = rowan::SyntaxElement<DuckSql>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_folds_case() {
        assert_eq!(Kw::lookup("SeLeCt"), Some(Kw::Select));
        assert_eq!(Kw::lookup("pivot_longer"), Some(Kw::PivotLonger));
        assert_eq!(Kw::lookup("orders"), None);
        assert_eq!(Kw::lookup("séléct"), None);
        for &kw in Kw::all() {
            assert_eq!(Kw::lookup(kw.text()), Some(kw));
            assert!(kw.text().len() <= KW_MAX_LEN);
        }
    }

    #[test]
    fn kinds_round_trip() {
        assert_eq!(SyntaxKind::from(Kw::Select), SyntaxKind::SELECT_KW);
        assert_eq!(SyntaxKind::SELECT_KW.as_kw(), Some(Kw::Select));
        assert_eq!(format!("{:?}", SyntaxKind::SELECT_KW), "SELECT_KW");
        assert_eq!(format!("{:?}", SyntaxKind::COLUMN_REF), "COLUMN_REF");
        assert!(SyntaxKind::COMMA.is_punct());
        assert!(!SyntaxKind::IDENT.is_punct());
    }
}
