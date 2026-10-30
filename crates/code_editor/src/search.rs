//! Find and replace over a snapshot of the text, with match case, whole word
//! and regex options (regex `^` and `$` match at line breaks, as in Monaco).

use regex::{Regex, RegexBuilder};

use crate::movement::{CharClass, class};
use crate::text::Edit;

/// Matches past this many are not collected (Monaco stops near 20k too).
pub const MAX_MATCHES: usize = 20_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Query {
    /// The compiled pattern, or the regex error to show.
    pub fn compile(&self) -> Result<Regex, String> {
        let pattern = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.match_case)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| class(c) == CharClass::Word)
}

/// Every non-empty match in `hay`, as byte ranges.
pub fn find_all(hay: &str, re: &Regex, whole_word: bool) -> Vec<(usize, usize)> {
    re.find_iter(hay)
        .filter(|m| !m.is_empty())
        .map(|m| (m.start(), m.end()))
        .filter(|&(a, b)| !whole_word || (!is_word(hay[..a].chars().next_back()) && !is_word(hay[b..].chars().next())))
        .take(MAX_MATCHES)
        .collect()
}

/// The replacement for the match at `range`: `$1`-style groups expand in
/// regex mode; otherwise the text is literal.
pub fn replacement(hay: &str, re: &Regex, range: (usize, usize), with: &str, regex: bool) -> String {
    if !regex {
        return with.to_string();
    }
    match re.captures_at(hay, range.0) {
        Some(c) if c.get(0).is_some_and(|m| m.start() == range.0) => {
            let mut out = String::new();
            c.expand(with, &mut out);
            out
        }
        _ => with.to_string(),
    }
}

/// Edits replacing every match.
pub fn replace_all(hay: &str, re: &Regex, matches: &[(usize, usize)], with: &str, regex: bool) -> Vec<Edit> {
    matches
        .iter()
        .map(|&m| Edit::new(m.0, m.1, replacement(hay, re, m, with, regex)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str, case: bool, word: bool, regex: bool) -> Query {
        Query {
            text: text.into(),
            match_case: case,
            whole_word: word,
            regex,
        }
    }

    #[test]
    fn literal_case_and_word() {
        let hay = "Foo foo food a.b";
        let re = q("foo", false, false, false).compile().unwrap();
        assert_eq!(find_all(hay, &re, false), vec![(0, 3), (4, 7), (8, 11)]);
        let re = q("foo", true, false, false).compile().unwrap();
        assert_eq!(find_all(hay, &re, false), vec![(4, 7), (8, 11)]);
        assert_eq!(find_all(hay, &re, true), vec![(4, 7)]);
        // Literal mode escapes regex syntax.
        let re = q("a.b", false, false, false).compile().unwrap();
        assert_eq!(find_all("axb a.b", &re, false), vec![(4, 7)]);
    }

    #[test]
    fn regex_lines_and_groups() {
        let hay = "x = 1\ny = 22\n";
        let re = q(r"^(\w) = (\d+)$", true, false, true).compile().unwrap();
        let m = find_all(hay, &re, false);
        assert_eq!(m, vec![(0, 5), (6, 12)]);
        assert_eq!(replacement(hay, &re, m[1], "$2 -> $1", true), "22 -> y");
        assert_eq!(replacement(hay, &re, m[1], "$2", false), "$2");
        let edits = replace_all(hay, &re, &m, "${1}!", true);
        assert_eq!(edits.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["x!", "y!"]);
        assert!(q("(", false, false, true).compile().is_err());
        // Empty matches are skipped.
        let re = q("x*", true, false, true).compile().unwrap();
        assert_eq!(find_all("axxb", &re, false), vec![(1, 3)]);
    }
}
