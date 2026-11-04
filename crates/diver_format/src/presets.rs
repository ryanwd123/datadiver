//! Ready-made rules. Choosing one inserts its text, which can then be edited.

use crate::lexer::quote_column;
use crate::value::{Column, ValueKind};

pub struct Preset {
    pub name: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    /// The rule, with `{other}` for another number column and `{numbers}`
    /// for the number columns.
    template: &'static str,
}

impl Preset {
    /// The preset's rule for a table with `columns`, formatting `target`.
    pub fn text(&self, columns: &[Column], target: Option<&str>) -> String {
        let numbers: Vec<&Column> = columns.iter().filter(|c| c.kind == ValueKind::Number).collect();
        let other = numbers
            .iter()
            .find(|c| Some(c.name.as_str()) != target)
            .map_or("\"column\"".to_string(), |c| quote_column(&c.name));
        let listed: Vec<String> = numbers.iter().take(6).map(|c| quote_column(&c.name)).collect();
        let listed = if listed.is_empty() { "\"column1\", \"column2\"".to_string() } else { listed.join(", ") };
        self.template.replace("{other}", &other).replace("{numbers}", &listed)
    }
}

const fn p(name: &'static str, group: &'static str, description: &'static str, template: &'static str) -> Preset {
    Preset { name, group, description, template }
}

pub const PRESETS: &[Preset] = &[
    p(
        "Duplicate values",
        "Highlight",
        "Values that appear more than once in the column",
        "if duplicate(c) then bg: #ffc7ce, text: #9c0006",
    ),
    p(
        "Unique values",
        "Highlight",
        "Values that appear only once",
        "if unique(c) then bg: #c6efce, text: #006100",
    ),
    p("Blanks", "Highlight", "Empty cells", "if c is null then bg: #eeeeee"),
    p("Negative numbers", "Highlight", "Below zero in red", "if c < 0 then text: #c00000"),
    p(
        "Positive and negative",
        "Highlight",
        "Green above zero, red below",
        "if c > 0 then text: #006100 elif c < 0 then text: #c00000",
    ),
    p(
        "Text contains",
        "Highlight",
        "Cells containing some text",
        "if c contains 'error' then bg: #ffc7ce, text: #9c0006",
    ),
    p(
        "Greater than another column",
        "Highlight",
        "Compare with a column in the same row",
        "if c > {other} then bg: #c6efce, text: #006100",
    ),
    p(
        "Dates in the last 7 days",
        "Highlight",
        "Recent dates in bold",
        "if days_ago(c) between 0 and 7 then bold, text: #1f4e79",
    ),
    p("Past dates", "Highlight", "Dates before today, dimmed", "if c < today() then text: gray"),
    p("Top 10", "Top and bottom", "The 10 highest values", "if top(c, 10) then bg: #ffeb9c, text: #9c5700"),
    p("Bottom 10", "Top and bottom", "The 10 lowest values", "if bottom(c, 10) then bg: #ffc7ce, text: #9c0006"),
    p("Top 10%", "Top and bottom", "The highest tenth", "if top_percent(c, 10%) then bg: #c6efce, bold"),
    p(
        "Above average",
        "Top and bottom",
        "Values over the column's mean",
        "if c > avg(c) then bg: #c6efce, text: #006100",
    ),
    p(
        "Below average",
        "Top and bottom",
        "Values under the column's mean",
        "if c < avg(c) then bg: #ffc7ce, text: #9c0006",
    ),
    p(
        "Outliers",
        "Top and bottom",
        "More than two standard deviations from the mean",
        "if abs(zscore(c)) > 2 then bg: #ffeb9c, bold",
    ),
    p("Red to green", "Color scale", "Low red, middle yellow, high green", "scale(#f8696b, #ffeb84, #63be7b)"),
    p("Green to red", "Color scale", "Low green, high red", "scale(#63be7b, #ffeb84, #f8696b)"),
    p("White to green", "Color scale", "Darker green for higher values", "scale(white, #63be7b)"),
    p(
        "Blue, white, red around zero",
        "Color scale",
        "Diverging from zero",
        "scale(#5a8ac6, white, #f8696b, mid=0)",
    ),
    p("Blue bars", "Data bars", "A bar as long as the value", "bar(#638ec6)"),
    p("Gradient bars", "Data bars", "Bars that fade out", "bar(#63be7b, fill=gradient)"),
    p("Positive and negative bars", "Data bars", "Green right of zero, red left", "bar(#63be7b, #f8696b)"),
    p("Arrows", "Icons", "Down, sideways and up by thirds of the range", "icons(arrows)"),
    p("Traffic lights", "Icons", "Red, yellow and green", "icons(traffic)"),
    p("Stars", "Icons", "Empty to full stars", "icons(stars)"),
    p("Checks", "Icons", "Cross, warning, check", "icons(checks)"),
    p(
        "Up and down arrows",
        "Icons",
        "An arrow by the sign of the value",
        "if c > 0 then icon: up elif c < 0 then icon: down",
    ),
    p("Row sparkline", "Sparklines", "A line through the row's number columns", "sparkline({numbers})"),
    p("List sparkline", "Sparklines", "The cell's own list as bars", "sparkline(c, kind=bars), hide_text"),
    p("First list item", "Display", "Show a list's first item, keeping the rest for rules", "display(c[0], blank='-')"),
    p("Thousands", "Display", "Numbers in thousands, like 1,235K", "display(c, '#,##0,\"K\"')"),
    p("Percent", "Display", "Fractions as percents, like 12.3%", "display(c, '0.0%')"),
    p("Accounting", "Display", "Negative amounts in parentheses", "display(c, '$#,##0.00;($#,##0.00);\"-\"')"),
    p("Long date", "Display", "Dates like Mar 15, 2024", "display(c, 'mmm d, yyyy')"),
    p("Zebra rows", "Other", "Shade every other row", "if row() % 2 = 0 then bg: #f2f2f2"),
    p("Bold header-like values", "Other", "Bold and underlined", "bold, underline"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_checks_cleanly() {
        let columns = [
            Column::new("sales", ValueKind::Number),
            Column::new("cost", ValueKind::Number),
            Column::new("name", ValueKind::Text),
        ];
        for preset in PRESETS {
            let text = preset.text(&columns, Some("sales"));
            let checked = crate::check(&text, &columns);
            assert!(checked.diagnostics.is_empty(), "{}: {text}: {:?}", preset.name, checked.diagnostics);
            assert_eq!(checked.program.rules.len(), 1, "{}", preset.name);
        }
        assert_eq!(PRESETS[6].text(&columns, Some("sales")), "if c > \"cost\" then bg: #c6efce, text: #006100");
    }
}
