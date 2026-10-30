//! Languages: the `LANGS` table and detection from a file name.
//!
//! Each tree-sitter grammar is an optional dependency behind a `lang-*` feature,
//! and this table is the only place that names one.

/// Auto-indent rules for Enter.
pub struct IndentRules {
    /// A line ending with one of these (ignoring trailing comments and spaces)
    /// indents the next line.
    pub indent_after: &'static [&'static str],
    /// A line starting with one of these words dedents the next line.
    pub dedent_after: &'static [&'static str],
}

pub struct Lang {
    /// "python", "sql".
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Lowercase extensions without the dot.
    pub extensions: &'static [&'static str],
    pub language: fn() -> tree_sitter::Language,
    pub highlights: &'static str,
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
    pub brackets: &'static [(char, char)],
    /// Syntax node kinds that are string literals and comments (auto-closing
    /// is off inside them).
    pub string_nodes: &'static [&'static str],
    pub comment_nodes: &'static [&'static str],
    /// Syntax node kinds that fold when they span lines, and whether the fold
    /// starts on the parent's first line (a Python `block` belongs to its
    /// `def` or `if` line) instead of the node's own.
    pub fold_nodes: &'static [(&'static str, bool)],
    pub auto_close: &'static [(&'static str, &'static str)],
    pub indent: IndentRules,
    /// Seeds word completion.
    pub keywords: &'static [&'static str],
}

pub static LANGS: &[Lang] = &[
    #[cfg(feature = "lang-python")]
    Lang {
        id: "python",
        name: "Python",
        extensions: &["py", "pyw", "pyi"],
        language: || tree_sitter_python::LANGUAGE.into(),
        highlights: include_str!("../queries/python/highlights.scm"),
        line_comment: Some("#"),
        block_comment: None,
        brackets: &[('(', ')'), ('[', ']'), ('{', '}')],
        string_nodes: &["string"],
        comment_nodes: &["comment"],
        fold_nodes: &[
            ("block", true),
            ("dictionary", false),
            ("list", false),
            ("set", false),
            ("tuple", false),
            ("argument_list", false),
            ("parameters", false),
            ("parenthesized_expression", false),
            ("string", false),
        ],
        auto_close: &[
            ("(", ")"),
            ("[", "]"),
            ("{", "}"),
            ("\"", "\""),
            ("'", "'"),
        ],
        indent: IndentRules {
            indent_after: &[":"],
            dedent_after: &["return", "pass", "break", "continue", "raise"],
        },
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
            "del", "elif", "else", "except", "False", "finally", "for", "from", "global",
            "if", "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass",
            "raise", "return", "True", "try", "while", "with", "yield", "self", "print",
            "len", "range", "enumerate", "isinstance", "str", "int", "float", "list",
            "dict", "set", "tuple",
        ],
    },
    #[cfg(feature = "lang-sql")]
    Lang {
        id: "sql",
        name: "SQL",
        extensions: &["sql"],
        language: || tree_sitter_sequel::LANGUAGE.into(),
        highlights: include_str!("../queries/sql/highlights.scm"),
        line_comment: Some("--"),
        block_comment: Some(("/*", "*/")),
        brackets: &[('(', ')'), ('[', ']')],
        string_nodes: &["literal"],
        comment_nodes: &["comment", "marginalia"],
        fold_nodes: &[
            ("statement", false),
            ("subquery", false),
            ("cte", false),
            ("column_definitions", false),
            ("case", false),
            ("list", false),
            ("marginalia", false),
        ],
        auto_close: &[("(", ")"), ("[", "]"), ("'", "'"), ("\"", "\"")],
        indent: IndentRules {
            indent_after: &[],
            dedent_after: &[],
        },
        keywords: &[
            "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "INSERT", "INTO", "VALUES",
            "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "VIEW", "INDEX", "DROP",
            "ALTER", "ADD", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS",
            "ON", "AS", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION",
            "ALL", "DISTINCT", "CASE", "WHEN", "THEN", "ELSE", "END", "NULL", "IS", "IN",
            "BETWEEN", "LIKE", "EXISTS", "WITH", "PRIMARY", "KEY", "FOREIGN",
            "REFERENCES", "DEFAULT", "COUNT", "SUM", "AVG", "MIN", "MAX", "ASC", "DESC",
        ],
    },
];

/// The language registered for `id`, if compiled in.
pub fn by_id(id: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.id == id)
}

/// The language for a file name, from its extension.
pub fn detect(file_name: &str) -> Option<&'static Lang> {
    let ext = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
    LANGS.iter().find(|l| l.extensions.contains(&ext.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_by_extension() {
        assert!(detect("README").is_none());
        assert!(detect("notes.txt").is_none());
        #[cfg(feature = "lang-python")]
        assert_eq!(detect("C:\\x\\Main.PY").map(|l| l.id), Some("python"));
        #[cfg(not(feature = "lang-python"))]
        assert!(detect("main.py").is_none());
        #[cfg(feature = "lang-sql")]
        assert_eq!(detect("q.sql").map(|l| l.id), Some("sql"));
    }
}
