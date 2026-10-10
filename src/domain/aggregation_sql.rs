//! Small lexical checks around the expression slot; DuckDB remains the SQL parser.

pub const PIVOT_LIMITATIONS_URL: &str =
    "https://duckdb.org/docs/current/sql/statements/pivot.html#limitations";
pub const PIVOT_FILTER_ERROR: &str =
    "DuckDB ignores FILTER in PIVOT aggregations. Remove FILTER or remove the pivot dimensions.";

pub fn validate(sql: &str, pivot: bool) -> Result<(), String> {
    if sql.trim().is_empty() {
        return Err("Enter an SQL expression".into());
    }
    let tokens = tokens(sql)?;
    let mut depth = 0;
    for token in &tokens {
        match token.as_str() {
            "(" | "[" => depth += 1,
            ")" | "]" => {
                depth -= 1;
                if depth < 0 {
                    return Err("Unmatched closing bracket in SQL expression".into());
                }
            }
            ";" | "?" => return Err("Enter one expression without statements or parameters".into()),
            "," if depth == 0 => return Err("Enter one result expression per aggregation".into()),
            "select" | "from" | "where" | "group" | "having" | "order" | "limit" | "union"
            | "intersect" | "except" | "pivot" | "with" | "qualify"
                if depth == 0 =>
            {
                return Err("Enter an expression; the app supplies the query".into());
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("Unclosed bracket in SQL expression".into());
    }
    if pivot
        && tokens
            .windows(3)
            .any(|t| t[0] == ")" && t[1] == "filter" && t[2] == "(")
    {
        return Err(PIVOT_FILTER_ERROR.into());
    }
    Ok(())
}

pub fn validate_dimension(sql: &str) -> Result<(), String> {
    validate(sql, false).map_err(|error| error.replace("per aggregation", "per dimension"))?;
    dimension_body(sql)?;
    Ok(())
}

pub fn dimension_body(sql: &str) -> Result<&str, String> {
    let mut depth = 0;
    for (token, start, _) in tokens_spanned(sql)? {
        match token.as_str() {
            "(" | "[" => depth += 1,
            ")" | "]" => depth -= 1,
            "as" if depth == 0 => return Ok(sql[..start].trim()),
            _ => {}
        }
    }
    Ok(sql.trim())
}

/// The name written after `AS` in a dimension, without identifier quotes.
pub fn dimension_alias(sql: &str) -> Option<String> {
    let body = dimension_body(sql).ok()?;
    let rest = sql.trim().get(body.len()..)?.trim_start();
    let alias = rest.get(2..)?.trim();
    if alias.is_empty() {
        return None;
    }
    Some(
        alias
            .strip_prefix('"')
            .and_then(|alias| alias.strip_suffix('"'))
            .map_or_else(|| alias.to_owned(), |alias| alias.replace("\"\"", "\"")),
    )
}

fn tokens(sql: &str) -> Result<Vec<String>, String> {
    Ok(tokens_spanned(sql)?
        .into_iter()
        .map(|(token, _, _)| token)
        .collect())
}

fn tokens_spanned(sql: &str) -> Result<Vec<(String, usize, usize)>, String> {
    let bytes = sql.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let token_start = i;
        match bytes[i] {
            b'\'' | b'"' => {
                let quote = bytes[i];
                let escaped = quote == b'\''
                    && i > 0
                    && matches!(bytes[i - 1], b'e' | b'E')
                    && (i < 2 || !(bytes[i - 2].is_ascii_alphanumeric() || bytes[i - 2] == b'_'));
                i += 1;
                loop {
                    if i == bytes.len() {
                        return Err("Unclosed quote in SQL expression".into());
                    }
                    if escaped && bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        if bytes.get(i) != Some(&quote) {
                            break;
                        }
                    }
                    i += 1;
                }
                result.push(("quoted".into(), token_start, i));
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                i += 2;
                while depth != 0 {
                    if i >= bytes.len() {
                        return Err("Unclosed SQL comment".into());
                    }
                    if bytes[i..].starts_with(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes[i..].starts_with(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'$') {
                    i += 1;
                    let delimiter = &sql[start..i];
                    let end = sql[i..]
                        .find(delimiter)
                        .ok_or("Unclosed dollar-quoted SQL string")?;
                    i += end + delimiter.len();
                    result.push(("quoted".into(), token_start, i));
                } else {
                    return Err("SQL parameters are not supported in expressions".into());
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 128 => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128)
                {
                    i += 1;
                }
                result.push((sql[start..i].to_ascii_lowercase(), start, i));
            }
            byte if byte.is_ascii_whitespace() => i += 1,
            byte => {
                result.push(((byte as char).to_string(), i, i + 1));
                i += 1;
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_alias_reads_the_top_level_name() {
        assert_eq!(
            dimension_alias("date_trunc('month', \"date\") AS month").as_deref(),
            Some("month")
        );
        assert_eq!(
            dimension_alias("upper(region) as \"Region \"\"Code\"\"\"").as_deref(),
            Some("Region \"Code\"")
        );
        assert_eq!(dimension_alias("cast(x AS VARCHAR)"), None);
        assert_eq!(dimension_alias("upper(region)"), None);
    }

    #[test]
    fn filter_is_rejected_only_as_pivot_syntax() {
        for sql in [
            "sum(x) FILTER (WHERE x > 0)",
            "count(*) filter/*nested /* comment */ ok */(where true)",
            "first(x order by y)\n-- comment\nFiLtEr (where true)",
        ] {
            assert_eq!(validate(sql, true), Err(PIVOT_FILTER_ERROR.into()));
            assert!(validate(sql, false).is_ok());
        }
        for sql in [
            "first('FILTER (WHERE x)')",
            "sum(\"filter\")",
            "count(*) /* FILTER (WHERE x) */",
            "first($tag$FILTER (WHERE x)$tag$)",
            "first('it''s FILTER (WHERE x)')",
            "first(E'it\\'s FILTER (WHERE x)')",
        ] {
            assert!(validate(sql, true).is_ok(), "{sql}");
        }
    }

    #[test]
    fn expressions_cannot_escape_the_projection_slot() {
        for sql in [
            "",
            "sum(x), count(*)",
            "sum(x); drop table qp_data",
            "sum(x) from qp_data",
            "count(*) -- comment\n;",
            "sum(x))",
            "sum(x",
            "sum(?)",
            "first('unfinished",
            "count(*) /* unfinished",
        ] {
            assert!(validate(sql, false).is_err(), "{sql}");
        }
        for sql in [
            "sum(x) / nullif(count(*), 0)",
            "first(x ORDER BY y DESC)",
            "CASE WHEN count(*) > 0 THEN sum(x) ELSE 0 END",
            "count(*) -- trailing comment",
            "sum(x::DECIMAL(12,2))",
            "sum(x) AS result",
            "count(*) AS \"Row count\"",
            "sum(x) total",
        ] {
            assert!(validate(sql, false).is_ok(), "{sql}");
        }
    }
}
