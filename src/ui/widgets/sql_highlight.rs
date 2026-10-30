//! Colors for SQL text in the smaller editors, from `diver_sql`'s highlighting.
//! The text may be a script or a lone expression (`sum(x) > 3`); whichever
//! reading parses better colors it.

use diver_sql::{Entry, HighlightClass, ParseOptions};

use crate::ui::platform::Color;

pub const KEYWORD: Color = Color::rgb(0x0b57d0);
pub const STRING: Color = Color::rgb(0xa31515);
pub const NUMBER: Color = Color::rgb(0x098658);
pub const COMMENT: Color = Color::rgb(0x2f7d32);
pub const IDENTIFIER: Color = Color::rgb(0x1f2328);
pub const FUNCTION: Color = Color::rgb(0x795e26);

/// Colored byte ranges of `text`.
pub fn runs(text: &str) -> Vec<(usize, usize, Color)> {
    // Problems count unknown statements too, which an expression reads as.
    let script = diver_sql::parse(text);
    let script_problems = diver_sql::diagnostics(&script).len();
    let parse = if script_problems == 0 {
        script
    } else {
        let options = ParseOptions {
            entry: Entry::Expression,
            ..Default::default()
        };
        let expression = diver_sql::parse_with(text, options);
        if diver_sql::diagnostics(&expression).len() < script_problems {
            expression
        } else {
            script
        }
    };
    diver_sql::highlight(&parse)
        .into_iter()
        .filter_map(|(range, class)| {
            let color = match class {
                HighlightClass::Keyword => KEYWORD,
                HighlightClass::String | HighlightClass::FilePath => STRING,
                HighlightClass::Number => NUMBER,
                HighlightClass::Comment => COMMENT,
                HighlightClass::Function | HighlightClass::Aggregate => FUNCTION,
                _ => return None,
            };
            Some((usize::from(range.start()), usize::from(range.end()), color))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_keywords_strings_numbers_and_comments() {
        let text = "SELECT sum(x), 'a b', 42 FROM t -- note";
        let colored = runs(text)
            .into_iter()
            .map(|(start, end, color)| (&text[start..end], color))
            .collect::<Vec<_>>();
        assert!(colored.contains(&("SELECT", KEYWORD)));
        assert!(colored.contains(&("sum", FUNCTION)));
        assert!(colored.contains(&("'a b'", STRING)));
        assert!(colored.contains(&("42", NUMBER)));
        assert!(colored.contains(&("FROM", KEYWORD)));
        assert!(colored.contains(&("-- note", COMMENT)));
        assert!(!colored.iter().any(|(text, _)| *text == "t"));
    }

    #[test]
    fn expressions_color_as_expressions() {
        let text = "sum(x) FILTER (WHERE y > 1)";
        let colored = runs(text)
            .into_iter()
            .map(|(start, end, color)| (&text[start..end], color))
            .collect::<Vec<_>>();
        assert!(colored.contains(&("sum", FUNCTION)), "{colored:?}");
        assert!(colored.contains(&("WHERE", KEYWORD)), "{colored:?}");
    }
}
