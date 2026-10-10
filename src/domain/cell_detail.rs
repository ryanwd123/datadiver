//! The text the cell detail pane shows for a value.

/// A value ready to show: the text, and whether it is pretty-printed JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailText {
    pub text: String,
    pub json: bool,
}

/// Pretty-prints a JSON object or array; any other value shows as it is.
/// Keys keep their order and numbers their exact text, since the JSON is
/// re-indented rather than parsed and written again.
pub fn detail_text(value: &str) -> DetailText {
    let trimmed = value.trim();
    let container = trimmed.starts_with('{') || trimmed.starts_with('[');
    if container && serde_json::from_str::<serde::de::IgnoredAny>(trimmed).is_ok() {
        DetailText {
            text: indent_json(trimmed),
            json: true,
        }
    } else {
        DetailText {
            text: value.to_owned(),
            json: false,
        }
    }
}

/// Re-indents valid JSON with two spaces per level.
fn indent_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = json.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n("  ", depth));
    };
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '{' | '[' => {
                out.push(ch);
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
                if chars.peek().is_some_and(|next| matches!(next, '}' | ']')) {
                    out.push(chars.next().expect("peeked"));
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(ch);
            }
            ',' => {
                out.push(ch);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            ch if ch.is_whitespace() => {}
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_objects_and_arrays_indent_keeping_key_order_and_exact_numbers() {
        let detail = detail_text(r#" {"b":1.50,"a":[1, {"x":"y, z: {w}"}],"e":{},"f":[ ]} "#);
        assert!(detail.json);
        assert_eq!(
            detail.text,
            "{\n  \"b\": 1.50,\n  \"a\": [\n    1,\n    {\n      \"x\": \"y, z: {w}\"\n    }\n  ],\n  \"e\": {},\n  \"f\": []\n}"
        );
    }

    #[test]
    fn escaped_quotes_stay_inside_strings() {
        let detail = detail_text(r#"["a\"b,c", "\\"]"#);
        assert_eq!(detail.text, "[\n  \"a\\\"b,c\",\n  \"\\\\\"\n]");
    }

    #[test]
    fn plain_text_scalars_and_broken_json_show_unchanged() {
        for value in ["hello\nworld", "42", "\"quoted\"", "{not json", "[1, 2"] {
            assert_eq!(
                detail_text(value),
                DetailText {
                    text: value.into(),
                    json: false
                }
            );
        }
    }
}
