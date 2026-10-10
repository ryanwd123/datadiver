use std::collections::HashSet;

use crate::domain::documents::ViewAction;
use crate::domain::frame::ResultSnapshot;

pub fn validate_action(
    action: &ViewAction,
    existing: &[ViewAction],
    replacing_key: Option<&str>,
) -> Result<(), String> {
    let mut chars = action.key.chars();
    let key = chars.next().ok_or("action key is required")?;
    if chars.next().is_some() || key.is_whitespace() || key.is_control() {
        return Err("action key must be one printable non-whitespace character".into());
    }
    if action.description.trim().is_empty() {
        return Err("action description is required".into());
    }
    if action.command.trim().is_empty() {
        return Err("action command is required".into());
    }
    if existing
        .iter()
        .any(|saved| saved.key == action.key && replacing_key != Some(saved.key.as_str()))
    {
        return Err(format!("action key {} is already used", action.key));
    }
    validate_template(&action.command)?;
    Ok(())
}

pub fn validate_actions(actions: &[ViewAction]) -> Result<(), String> {
    let mut keys = HashSet::new();
    for action in actions {
        validate_action(action, &[], None)?;
        if !keys.insert(&action.key) {
            return Err(format!("action key {} is already used", action.key));
        }
    }
    Ok(())
}

/// What running an action does once its placeholders are filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// `http://` and `https://` templates open in the browser, with values
    /// percent-encoded.
    Url(String),
    /// Anything else starts a program directly, without a shell, so cell values
    /// cannot inject shell syntax. Each value stays inside its argument.
    Program { program: String, args: Vec<String> },
}

impl Launch {
    /// The filled-in command as text, quoting arguments that need it.
    pub fn preview(&self) -> String {
        match self {
            Self::Url(url) => url.clone(),
            Self::Program { program, args } => std::iter::once(program)
                .chain(args)
                .map(|word| {
                    if word.is_empty() || word.contains(char::is_whitespace) {
                        format!("\"{word}\"")
                    } else {
                        word.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// Column names and display values of result row `row`, in column order.
pub fn row_values(result: &ResultSnapshot, row: usize) -> Result<Vec<(String, String)>, String> {
    if row >= result.row_count() {
        return Err("an active row is required".into());
    }
    let rows = match result.cached_rows(row, 1) {
        Some(rows) => rows,
        None => result.fetch_rows(row, 1)?,
    };
    let cached_row = rows.first().ok_or("active row is unavailable")?;
    Ok(result
        .columns
        .iter()
        .zip(result.column_indices.iter())
        .map(|(column, &index)| {
            (
                column.0.clone(),
                cached_row.values[index].clone().unwrap_or_default(),
            )
        })
        .collect())
}

/// Fill `template` with the values of result row `row`.
pub fn launch_for_row(
    template: &str,
    result: &ResultSnapshot,
    row: usize,
) -> Result<Launch, String> {
    launch_with_values(template, &row_values(result, row)?)
}

/// Fill `template` from `(column, value)` pairs.
pub fn launch_with_values(template: &str, values: &[(String, String)]) -> Result<Launch, String> {
    prepare(template, |column| {
        values
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("column {column} is missing"))
    })
}

fn prepare(
    template: &str,
    mut lookup: impl FnMut(&str) -> Result<String, String>,
) -> Result<Launch, String> {
    let template = template.trim();
    let lowered = template.to_ascii_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        let url = expand(template, |column| {
            lookup(column).map(|value| percent_encode(&value))
        })?;
        // Values are already encoded, so any space left came from the template.
        return Ok(Launch::Url(url.replace(' ', "%20")));
    }
    let mut words = split_arguments(template)
        .iter()
        .map(|word| expand(word, &mut lookup))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let program = words
        .next()
        .filter(|program| !program.is_empty())
        .ok_or("action command has no program")?;
    Ok(Launch::Program {
        program,
        args: words.collect(),
    })
}

/// Split on whitespace outside double quotes, removing the quotes.
fn split_arguments(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in command.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            ch if ch.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            ch => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn validate_template(template: &str) -> Result<(), String> {
    prepare(template, |_| Ok("value".into())).map(|_| ())
}

fn expand(
    template: &str,
    mut substitute: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut column = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    if next == '{' {
                        return Err("nested action placeholder".into());
                    }
                    column.push(next);
                }
                if !closed || column.is_empty() {
                    return Err("malformed action placeholder".into());
                }
                output.push_str(&substitute(&column)?);
            }
            '}' => return Err("unmatched closing brace in action".into()),
            ch => output.push(ch),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::frame::FrameSnapshot;
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use duckdb::types::Value;

    #[test]
    fn validates_case_sensitive_unique_keys() {
        let saved = ViewAction {
            key: "a".into(),
            description: "Old".into(),
            command: "echo old".into(),
        };
        let candidate = ViewAction {
            key: "A".into(),
            description: "New".into(),
            command: "echo new".into(),
        };
        assert!(validate_action(&candidate, std::slice::from_ref(&saved), None).is_ok());
        assert!(validate_action(&saved, std::slice::from_ref(&saved), None).is_err());
        assert!(validate_action(&saved, std::slice::from_ref(&saved), Some("a")).is_ok());
    }

    #[test]
    fn substitutes_active_row_and_escaped_braces() {
        let store = DuckDbStore::from_rows(
            &[("Name".into(), "VARCHAR".into())],
            [vec![Value::Text("A & B".into())], vec![Value::Null]],
        )
        .unwrap();
        let source = FrameSnapshot::new(store, 1);
        let result = ResultSnapshot::from_source(&source, 1, 1);
        let program = |program: &str, args: &[&str]| Launch::Program {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        };
        assert_eq!(
            launch_for_row("echo {{value}} {Name}", &result, 0).unwrap(),
            program("echo", &["{value}", "A & B"])
        );
        assert_eq!(
            launch_for_row("echo {Name}", &result, 1).unwrap(),
            program("echo", &[""])
        );
        assert!(launch_for_row("echo {Missing}", &result, 0).is_err());
        assert!(launch_for_row("echo {", &result, 0).is_err());
        assert!(launch_for_row("echo {Name}", &result, 2).is_err());
    }

    #[test]
    fn urls_encode_values_and_commands_keep_values_in_one_argument() {
        let lookup = |column: &str| match column {
            "Name" => Ok("O'Brien & \"Sons\" ü".to_owned()),
            "Empty" => Ok(String::new()),
            _ => Err(format!("column {column} is missing")),
        };
        assert_eq!(
            prepare(" https://example.com/search?q={Name} ", lookup).unwrap(),
            Launch::Url(
                "https://example.com/search?q=O%27Brien%20%26%20%22Sons%22%20%C3%BC".into()
            )
        );
        assert_eq!(
            prepare("code \"C:/My Files/{Name}.txt\" --flag \"\"", lookup).unwrap(),
            Launch::Program {
                program: "code".into(),
                args: vec![
                    "C:/My Files/O'Brien & \"Sons\" ü.txt".into(),
                    "--flag".into(),
                    String::new()
                ],
            }
        );
        assert!(prepare("{Empty} arg", lookup).is_err());
        assert!(validate_template("{Program} arg").is_ok());
        let values = [("Id".to_owned(), "a b".to_owned())];
        assert_eq!(
            launch_with_values("notepad {Id} \"\" x", &values)
                .unwrap()
                .preview(),
            "notepad \"a b\" \"\" x"
        );
        assert_eq!(
            launch_with_values("https://x.test/{Id}", &values)
                .unwrap()
                .preview(),
            "https://x.test/a%20b"
        );
        assert!(validate_template("   ").is_err());
    }
}
