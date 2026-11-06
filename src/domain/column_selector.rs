//! Picks the columns a format set applies to by a pattern, so a set keeps up
//! with columns that come and go, such as a pivot's.
//!
//! The pattern works like the search box, over column names in the order
//! the grid shows them:
//!
//! - `sales` takes names containing "sales", ignoring case; several words
//!   must all match, and `-total` leaves out names containing "total".
//! - `"Order Date"` takes the column with exactly that name.
//! - `/^q[1-4]$/` takes names the regular expression matches, ignoring case.
//! - `a..b` takes a run of columns: from the first matching `a` through the
//!   last matching `b`. Each end is a word, a quoted name or a regex, and an
//!   end left out reaches the first or last column.
//! - `|` separates alternatives; a column matching any of them is taken.
//!
//! A pattern is parsed once into a [`ColumnSelector`], and matching it is a
//! pass over the names.

use regex::Regex;

#[derive(Clone, Debug)]
pub struct ColumnSelector {
    /// Alternatives separated by `|`.
    groups: Vec<Group>,
}

#[derive(Clone, Debug, Default)]
struct Group {
    include: Vec<Term>,
    exclude: Vec<Term>,
}

#[derive(Clone, Debug)]
enum Term {
    Match(Matcher),
    /// The first column matching the start through the last matching the end.
    Range(Option<Matcher>, Option<Matcher>),
}

#[derive(Clone, Debug)]
enum Matcher {
    /// Lowercase.
    Contains(String),
    /// Lowercase.
    Exact(String),
    Regex(Regex),
}

impl Matcher {
    fn matches(&self, name: &str, lower: &str) -> bool {
        match self {
            Matcher::Contains(word) => lower.contains(word.as_str()),
            Matcher::Exact(exact) => lower == exact,
            Matcher::Regex(regex) => regex.is_match(name),
        }
    }
}

impl ColumnSelector {
    /// Parses `pattern`; the error says what is wrong with it.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        Parser { text: pattern, at: 0 }.selector()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.include.is_empty() && group.exclude.is_empty())
    }

    /// The indices of the `names` the pattern takes, in order.
    pub fn select<S: AsRef<str>>(&self, names: &[S]) -> Vec<usize> {
        if self.is_empty() {
            return Vec::new();
        }
        let lower: Vec<String> = names.iter().map(|name| name.as_ref().to_lowercase()).collect();
        let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
        let mut taken = vec![false; names.len()];
        for group in &self.groups {
            if group.include.is_empty() && group.exclude.is_empty() {
                continue;
            }
            let mut chosen = vec![true; names.len()];
            for term in &group.include {
                let marked = term.marks(&names, &lower);
                chosen.iter_mut().zip(marked).for_each(|(chosen, marked)| *chosen &= marked);
            }
            for term in &group.exclude {
                let marked = term.marks(&names, &lower);
                chosen.iter_mut().zip(marked).for_each(|(chosen, marked)| *chosen &= !marked);
            }
            taken.iter_mut().zip(chosen).for_each(|(taken, chosen)| *taken |= chosen);
        }
        taken.iter().enumerate().filter(|(_, taken)| **taken).map(|(index, _)| index).collect()
    }
}

impl Term {
    /// Which of `names` the term takes.
    fn marks(&self, names: &[&str], lower: &[String]) -> Vec<bool> {
        fn hits<'a>(matcher: &'a Matcher, names: &'a [&str], lower: &'a [String]) -> impl Iterator<Item = usize> + 'a {
            (0..names.len()).filter(move |&i| matcher.matches(names[i], &lower[i]))
        }
        let hits = |matcher| hits(matcher, names, lower);
        match self {
            Term::Match(matcher) => {
                let mut marks = vec![false; names.len()];
                hits(matcher).for_each(|i| marks[i] = true);
                marks
            }
            Term::Range(start, end) => {
                let mut marks = vec![false; names.len()];
                let from = match start {
                    Some(matcher) => hits(matcher).next(),
                    None => (!names.is_empty()).then_some(0),
                };
                let to = match end {
                    Some(matcher) => hits(matcher).last(),
                    None => names.len().checked_sub(1),
                };
                if let (Some(from), Some(to)) = (from, to) {
                    marks[from.min(to)..=from.max(to)].iter_mut().for_each(|mark| *mark = true);
                }
                marks
            }
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_space(&mut self) {
        let trimmed = self.rest().trim_start();
        self.at = self.text.len() - trimmed.len();
    }

    fn selector(mut self) -> Result<ColumnSelector, String> {
        let mut groups = vec![Group::default()];
        loop {
            self.skip_space();
            match self.peek() {
                None => break,
                Some('|') => {
                    self.at += 1;
                    groups.push(Group::default());
                }
                Some(_) => {
                    let exclude = self.rest().starts_with('-') && self.rest()[1..].starts_with(|c: char| !c.is_whitespace() && c != '|');
                    if exclude {
                        self.at += 1;
                    }
                    let term = self.term()?;
                    let group = groups.last_mut().expect("a group");
                    if exclude {
                        group.exclude.push(term);
                    } else {
                        group.include.push(term);
                    }
                }
            }
        }
        Ok(ColumnSelector { groups })
    }

    fn term(&mut self) -> Result<Term, String> {
        let start = self.matcher()?;
        if !self.rest().starts_with("..") {
            return start.map(Term::Match).ok_or_else(|| "expected a column name or pattern".to_string());
        }
        self.at += 2;
        let end = self.matcher()?;
        if start.is_none() && end.is_none() {
            return Err("a range needs a start or an end, like 2021..2025".into());
        }
        Ok(Term::Range(start, end))
    }

    /// A quoted name, a `/regex/` or a word, up to a space, `|` or `..`;
    /// `None` when there is none here.
    fn matcher(&mut self) -> Result<Option<Matcher>, String> {
        match self.peek() {
            Some('"') => {
                self.at += 1;
                let mut name = String::new();
                loop {
                    let Some(c) = self.peek() else {
                        return Err("a quoted name isn't closed".into());
                    };
                    self.at += c.len_utf8();
                    if c == '"' {
                        // `""` is a quote inside the name.
                        if self.peek() == Some('"') {
                            self.at += 1;
                        } else {
                            break;
                        }
                    }
                    name.push(c);
                }
                Ok(Some(Matcher::Exact(name.to_lowercase())))
            }
            Some('/') => {
                self.at += 1;
                let mut pattern = String::new();
                loop {
                    let Some(c) = self.peek() else {
                        return Err("a /regex/ isn't closed".into());
                    };
                    self.at += c.len_utf8();
                    match c {
                        '/' => break,
                        // `\/` is a slash inside the pattern.
                        '\\' if self.peek() == Some('/') => {
                            self.at += 1;
                            pattern.push('/');
                        }
                        c => pattern.push(c),
                    }
                }
                let regex = regex::RegexBuilder::new(&pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| format!("the regex /{pattern}/ is invalid: {}", first_line(&error.to_string())))?;
                Ok(Some(Matcher::Regex(regex)))
            }
            _ => {
                let rest = self.rest();
                let end = rest
                    .char_indices()
                    .find(|(i, c)| c.is_whitespace() || *c == '|' || rest[*i..].starts_with(".."))
                    .map_or(rest.len(), |(i, _)| i);
                if end == 0 {
                    return Ok(None);
                }
                let word = rest[..end].to_lowercase();
                self.at += end;
                Ok(Some(Matcher::Contains(word)))
            }
        }
    }
}

/// The regex crate's errors span lines; the last one says what is wrong.
fn first_line(message: &str) -> &str {
    let line = message.lines().rev().find(|line| !line.trim().is_empty()).map_or(message, str::trim);
    line.strip_prefix("error: ").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::ColumnSelector;

    const NAMES: [&str; 8] = ["region", "abc", "2021-01", "2021-02", "2024-06", "2025-01", "2025-12", "def"];

    fn names(pattern: &str) -> Vec<&'static str> {
        ColumnSelector::parse(pattern)
            .unwrap()
            .select(&NAMES)
            .into_iter()
            .map(|index| NAMES[index])
            .collect()
    }

    #[test]
    fn words_quotes_and_exclusions_work_like_the_search_box() {
        assert_eq!(names("2025"), ["2025-01", "2025-12"]);
        assert_eq!(names("REG"), ["region"]);
        assert_eq!(names("2021 -01"), ["2021-02"]);
        assert_eq!(names("2025 12"), ["2025-12"]);
        assert_eq!(names("20 -2021"), ["2024-06", "2025-01", "2025-12"]);
        assert_eq!(names("\"ABC\""), ["abc"]);
        assert!(names("\"ab\"").is_empty());
        assert_eq!(names("abc | def"), ["abc", "def"]);
        assert!(names("").is_empty() && names("  ").is_empty());
        assert_eq!(names("-20").len(), 3);
    }

    #[test]
    fn ranges_run_in_grid_order_between_their_ends() {
        // Exact names, in either order.
        assert_eq!(names("\"abc\"..\"2021-02\""), ["abc", "2021-01", "2021-02"]);
        assert_eq!(names("\"2021-02\"..\"abc\""), ["abc", "2021-01", "2021-02"]);
        // The first column containing 2021 through the last containing 2025.
        assert_eq!(names("2021..2025"), ["2021-01", "2021-02", "2024-06", "2025-01", "2025-12"]);
        assert_eq!(names("..abc"), ["region", "abc"]);
        assert_eq!(names("2025-12.."), ["2025-12", "def"]);
        assert_eq!(names("2021..2025 -2024"), ["2021-01", "2021-02", "2025-01", "2025-12"]);
        // An end that matches nothing takes nothing.
        assert!(names("2021..zzz").is_empty());
    }

    #[test]
    fn regexes_ignore_case_and_problems_are_explained() {
        assert_eq!(names("/^2025-\\d+$/"), ["2025-01", "2025-12"]);
        assert_eq!(names("/^A/"), ["abc"]);
        assert_eq!(names("/^abc$/..2021"), ["abc", "2021-01", "2021-02"]);
        assert!(ColumnSelector::parse("\"abc").unwrap_err().contains("isn't closed"));
        assert!(ColumnSelector::parse("/abc").unwrap_err().contains("isn't closed"));
        assert!(ColumnSelector::parse("/a(/").unwrap_err().contains("invalid"));
        assert!(ColumnSelector::parse("..").unwrap_err().contains("start or an end"));
    }
}
