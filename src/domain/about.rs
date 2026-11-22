//! The About page: a table that shows DataDiver's version and shows off
//! the grid. The mascot is pixel art in 24 narrow columns, colored by one
//! formatting rule, beside a list of the main keys and a dive log that
//! uses data bars, a color scale, icons and a sparkline.

use std::collections::BTreeMap;

use crate::domain::documents::{
    Alignment, ColumnPresentation, ConditionalFormat, DocumentDefinition,
    TableConfig, ViewKind,
};

/// The page's file, under the profile's data folder. It is written again
/// each time the page opens, so it always shows the running version.
pub const FILE_NAME: &str = "about-datadiver.csv";

pub const VIEW_NAME: &str = "About DataDiver";

/// A hooded merganser, DataDiver's mascot, one letter per cell; the
/// colors are in [`PIXEL_RULES`].
const MASCOT: [&str; 24] = [
    "........................",
    "........................",
    "........................",
    "........................",
    "......KKKKKK............",
    "....KKKWWWWWKK..........",
    "...KKWWWWWWWWKK.........",
    "..KKWWWWWWWWWKKK........",
    "..KWWWWWWWWWKKKKK.......",
    "..KKWWWWWWWKKKYKK.......",
    "...KKWWWWWKKKKKKKBBBBBB.",
    ".....KKKKKKKKKKKKBBB....",
    ".KKK..KKKKKKKKKKWW......",
    "..KKKKKKKKKKKKKKWKW.....",
    "..KKKKKKKKKKKKKKWKWW....",
    "...KKRRRRRRRRRRKWKWW....",
    "....KRRRRRRRRRRRWWW.....",
    "~~-~~~RRRRRRRRRRRW~~~-~~",
    "~~~~~~~====~====~~~o~~~~",
    "~~~~-~~~~~~~~~~~~~~~~~~~",
    "~~~~~~~~~~~~~~~~~~o~~-~~",
    "~~~~FFF~F~~~~~~~~~~~~~~~",
    "~-~FKFFFF~~-~~~~~~~~~~~~",
    "~~~~FFF~F~~~~~~~~~~~~~~~",
];

const PIXEL_WIDTH: f32 = 26.0;

const PIXEL_RULES: &str = "\
-- The mascot, a hooded merganser. Each cell holds a letter, and this
-- rule turns the letter into a color.
if c = 'K' then bg: #16161b
elif c = 'd' then bg: #3a3a44
elif c = 'g' then bg: #a3a3ad
elif c = 'W' then bg: #fdfdfd
elif c = 'Y' then bg: #ffc107
elif c = 'B' then bg: #4d4d57
elif c = '~' then bg: #2f86c8
elif c = '-' then bg: #7cc0ec
elif c = '=' then bg: #24689c
elif c = 'o' then bg: #cfe9fb
elif c = 'R' then bg: #9c4a1f
elif c = 'F' then bg: #ff8c42
else bg: #d9eefa;
hide_text;
tooltip('A hooded merganser, the diving duck behind DataDiver.\\n576 cells and one formatting rule: press M to see it.', delay=600)";

const TEXT_RULES: &str = "\
if \"kind\" = 'title' then bold, bg: #1d4e74, text: white
elif \"kind\" = 'version' then bold, text: #2f86c8
elif \"kind\" = 'heading' then bold, underline
elif \"kind\" = 'tagline' then italic
elif \"kind\" = 'note' then italic, text: gray;
if \"tip\" is not null then tooltip(\"tip\", max_width=420px)";

const KEY_RULES: &str = "\
if \"kind\" = 'title' then bg: #1d4e74;
-- Keys look like keycaps.
if c is not null then bg: #eef2f7, text: #1d2733, border: 1px #aab6c4, align: center";

const DEPTH_RULES: &str = "\
bar(#2f86c8, fill=gradient, min=0), display(c, '0.0');
tooltip('A dive to {c:.1f} m.\\nThe bar: bar(#2f86c8, fill=gradient, min=0)')";

const TIME_RULES: &str = "\
scale(#e8f6f3, #2fa38f);
tooltip('{c} seconds under water.\\nThe color: scale(#e8f6f3, #2fa38f)')";

const CATCH_RULES: &str = "\
icons(stars);
tooltip('{c} fish caught.\\nThe stars: icons(stars)')";

const PROFILE_RULES: &str = "\
sparkline(\"s1\", \"s2\", \"s3\", \"s4\", \"s5\", \"s6\", \"s7\", color=#1d4e74), hide_text;
tooltip('Down {c:.1f} m and back up.\\nThe line: sparkline(\"s1\", …, \"s7\")')";

/// Each line beside the mascot: its kind (for the rules), text, key and tooltip.
const LINES: [(&str, &str, &str, &str); 24] = [
    ("title", "DataDiver", "", ""),
    ("version", "", "", ""),
    ("tagline", "Explore data fast, mostly from the keyboard.", "", ""),
    ("", "", "", ""),
    ("heading", "Get around", "", ""),
    ("", "Open a file", "Ctrl+O", "CSV, TSV, Excel, Parquet and more. You can also drop files on the window or paste a table with Ctrl+V."),
    ("", "Recent files and saved views", "Ctrl+P", ""),
    ("", "Every command", "Ctrl+Shift+P", "The command palette. Type part of a name; each command shows its key."),
    ("", "Help on what you are doing", "F1", ""),
    ("", "", "", ""),
    ("heading", "Explore", "", ""),
    ("", "Search the rows", "/", "Words, column terms such as region:north, and comparisons such as price>100."),
    ("", "Add a slicer card", "Z", ""),
    ("", "Keep or drop rows like this cell", "=   -", ""),
    ("", "Group, aggregate and pivot", "G   A   P", ""),
    ("", "Drill into a result", "Enter", "Shows the rows behind a grouped or pivoted cell. U goes back."),
    ("", "Format cells with rules", "M", "Everything on this page is formatting. Press M on any column to see its rules."),
    ("", "SQL editor", "Ctrl+J", ""),
    ("", "Copy as CSV, JSON or Markdown", "Ctrl+Shift+C", ""),
    ("", "Save everything as a view", "Ctrl+S", ""),
    ("", "", "", ""),
    ("note", "The duck: 576 cells and one rule.", "", "Click a column of the duck and press M."),
    ("note", "The dive log: bars, scales, icons, lines.", "", "Point at its cells to see the rules behind them."),
    ("note", "Find this again: About DataDiver.", "", "It is in the command palette, Ctrl+Shift+P."),
];

const DEPTHS: [f64; 24] = [
    1.8, 2.4, 3.1, 1.2, 4.6, 2.9, 5.2, 3.3, 2.1, 6.0, 4.1, 1.5, 3.8, 2.7, 5.5, 4.4, 2.2, 3.6,
    1.9, 4.9, 3.0, 2.5, 5.8, 3.4,
];

const CATCHES: [u8; 24] = [1, 0, 2, 0, 3, 1, 2, 1, 0, 3, 2, 0, 1, 2, 3, 2, 0, 1, 1, 3, 1, 0, 2, 1];

/// How deep a dive is at each of the profile's samples, as a share of its depth.
const PROFILES: [[f64; 7]; 2] = [
    [0.0, 0.3, 0.7, 0.95, 1.0, 0.55, 0.0],
    [0.0, 0.6, 0.95, 1.0, 0.7, 0.3, 0.0],
];

const TEXT_COLUMN: &str = "DataDiver";
const KEY_COLUMN: &str = "Key";
const DEPTH_COLUMN: &str = "dive depth (m)";
const TIME_COLUMN: &str = "time under (s)";
const CATCH_COLUMN: &str = "fish caught";
const PROFILE_COLUMN: &str = "dive profile";
const SAMPLES: [&str; 7] = ["s1", "s2", "s3", "s4", "s5", "s6", "s7"];

/// The mascot's columns, named like a spreadsheet's.
fn pixel_columns() -> Vec<String> {
    (b'A'..b'A' + MASCOT[0].len() as u8).map(|letter| char::from(letter).to_string()).collect()
}

fn columns() -> Vec<String> {
    let mut columns = pixel_columns();
    columns.extend([TEXT_COLUMN, KEY_COLUMN, DEPTH_COLUMN, TIME_COLUMN, CATCH_COLUMN, PROFILE_COLUMN, "kind", "tip"].map(String::from));
    columns.extend(SAMPLES.map(String::from));
    columns
}

/// The page's rows as CSV, for DataDiver `version`.
pub fn csv(version: &str) -> String {
    let mut out = String::new();
    push_row(&mut out, columns());
    for (index, ((pixels, line), depth)) in MASCOT.iter().zip(LINES).zip(DEPTHS).enumerate() {
        let (kind, text, key, tip) = line;
        let text = if kind == "version" { format!("Version {version}") } else { text.to_owned() };
        let seconds = (6.0 + depth * 3.5 + (index * 7 % 5) as f64).round();
        let profile = PROFILES[index % 2];
        let mut row: Vec<String> = pixels.chars().map(String::from).collect();
        row.extend([
            text,
            key.to_owned(),
            format!("{depth:.1}"),
            format!("{seconds}"),
            CATCHES[index].to_string(),
            format!("{:.1}", -depth),
            kind.to_owned(),
            tip.to_owned(),
        ]);
        row.extend(profile.iter().map(|share| format!("{:.2}", -depth * share)));
        push_row(&mut out, row);
    }
    out
}

fn push_row(out: &mut String, cells: Vec<String>) {
    let cells: Vec<String> = cells
        .into_iter()
        .map(|cell| {
            if cell.contains([',', '"', '\n']) {
                format!("\"{}\"", cell.replace('"', "\"\""))
            } else {
                cell
            }
        })
        .collect();
    out.push_str(&cells.join(","));
    out.push('\n');
}

/// The page as an unsaved table view of the file at `path`.
pub fn view(path: &str) -> DocumentDefinition {
    let mut definition = DocumentDefinition::draft_table(VIEW_NAME.into(), path.into());
    let pixels = pixel_columns();
    let format = |rules: &str, columns: Vec<String>| ConditionalFormat {
        rules: rules.into(),
        columns,
        selector: String::new(),
    };
    let one = |name: &str| vec![name.to_owned()];
    definition.formats = vec![
        format(PIXEL_RULES, pixels.clone()),
        format(TEXT_RULES, one(TEXT_COLUMN)),
        format(KEY_RULES, one(KEY_COLUMN)),
        format(DEPTH_RULES, one(DEPTH_COLUMN)),
        format(TIME_RULES, one(TIME_COLUMN)),
        format(CATCH_RULES, one(CATCH_COLUMN)),
        format(PROFILE_RULES, one(PROFILE_COLUMN)),
    ];
    let column = |width: f32, alignment: Alignment| ColumnPresentation {
        width,
        alignment,
        format_override: None,
    };
    let mut presentation: BTreeMap<String, ColumnPresentation> = pixels
        .iter()
        .map(|name| (name.clone(), column(PIXEL_WIDTH, Alignment::Center)))
        .collect();
    presentation.insert(TEXT_COLUMN.into(), column(280.0, Alignment::Left));
    presentation.insert(KEY_COLUMN.into(), column(110.0, Alignment::Center));
    presentation.insert(DEPTH_COLUMN.into(), column(110.0, Alignment::Right));
    presentation.insert(TIME_COLUMN.into(), column(80.0, Alignment::Right));
    presentation.insert(CATCH_COLUMN.into(), column(80.0, Alignment::Right));
    presentation.insert(PROFILE_COLUMN.into(), column(100.0, Alignment::Left));
    let mut hidden = vec!["kind".to_owned(), "tip".to_owned()];
    hidden.extend(SAMPLES.map(String::from));
    definition.view = ViewKind::Table(TableConfig {
        column_order: columns(),
        presentation,
        hidden,
        ..TableConfig::default()
    });
    definition
}

#[cfg(test)]
mod tests {
    use super::*;
    use diver_format::{Column, ValueKind};

    #[test]
    fn mascot_is_square_and_every_letter_has_a_color() {
        for row in MASCOT {
            assert_eq!(row.len(), MASCOT.len(), "{row}");
            for letter in row.chars().filter(|letter| *letter != '.') {
                assert!(PIXEL_RULES.contains(&format!("c = '{letter}'")), "no color for {letter}");
            }
        }
    }

    #[test]
    fn rows_have_every_column_and_the_version() {
        let text = csv("9.8.7");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1 + MASCOT.len());
        assert!(text.contains("Version 9.8.7"));
        let columns = columns().len();
        for line in &lines {
            // Every comma outside quotes separates cells.
            let mut quoted = false;
            let separators = line
                .chars()
                .filter(|ch| {
                    if *ch == '"' {
                        quoted = !quoted;
                    }
                    *ch == ',' && !quoted
                })
                .count();
            assert_eq!(separators + 1, columns, "{line}");
        }
    }

    #[test]
    fn every_format_checks_without_errors() {
        let kind = |name: &str| match name {
            DEPTH_COLUMN | TIME_COLUMN | CATCH_COLUMN | PROFILE_COLUMN => ValueKind::Number,
            name if SAMPLES.contains(&name) => ValueKind::Number,
            _ => ValueKind::Text,
        };
        let columns: Vec<Column> = columns().iter().map(|name| Column::new(name.clone(), kind(name))).collect();
        let view = view("about.csv");
        view.validate().unwrap();
        for format in &view.formats {
            let checked = diver_format::check(&format.rules, &columns);
            let errors: Vec<_> = checked.errors().collect();
            assert!(errors.is_empty(), "{}: {errors:?}", format.rules);
            assert!(!checked.program.is_empty());
        }
    }
}
