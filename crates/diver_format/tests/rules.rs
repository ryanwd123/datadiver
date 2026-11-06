//! Rules checked against a table and run over its cells.

use diver_format::catalog::Shape;
use diver_format::style::{Bar, Icon, LineStyle};
use diver_format::*;

fn table() -> (Vec<Column>, Vec<Vec<Value>>) {
    let columns = vec![
        Column::new("name", ValueKind::Text),
        Column::new("sales", ValueKind::Number),
        Column::new("target", ValueKind::Number),
        Column::new("trend", ValueKind::List),
    ];
    let rows = [
        ("ann", Some(10.0), 5.0, "[1, 2, 3]"),
        ("bob", Some(-5.0), 0.0, "[3, 2]"),
        ("ann", Some(30.0), 40.0, "[]"),
        ("cy", None, 1.0, "[1, null, 4]"),
        ("dee", Some(20.0), 20.0, "[0]"),
    ];
    let mut data = vec![Vec::new(); 4];
    for (name, sales, target, trend) in rows {
        data[0].push(Value::Text(name.into()));
        data[1].push(sales.map_or(Value::Null, Value::Number));
        data[2].push(Value::Number(target));
        data[3].push(Value::from_text(Some(trend), ValueKind::List));
    }
    (columns, data)
}

/// The style of every cell of column `target`.
fn run(rules: &str, target: usize) -> Vec<CellStyle> {
    let (columns, data) = table();
    let checked = check(rules, &columns);
    assert!(checked.diagnostics.is_empty(), "{rules}: {:?}", checked.diagnostics);
    let stats = TableStats::new(&data);
    (0..data[0].len())
        .map(|r| {
            let values: Vec<Value> = data.iter().map(|c| c[r].clone()).collect();
            checked.program.style(target, &ValuesRow { values: &values, index: r }, &stats)
        })
        .collect()
}

fn messages(rules: &str) -> Vec<String> {
    let (columns, _) = table();
    check(rules, &columns).diagnostics.into_iter().map(|d| d.message).collect()
}

const RED: Color = Color::rgb(255, 0, 0);
const GREEN: Color = Color::rgb(0, 128, 0);

#[test]
fn comparing_with_another_column_of_the_row() {
    let styles = run("if c > \"target\" then bg:red , border:1px, border_weight:2px, text:green", 1);
    assert_eq!(styles[0].background, Some(RED));
    assert_eq!(styles[0].text, Some(GREEN));
    let border = styles[0].border.unwrap();
    assert_eq!((border.width, border.style), (2.0, LineStyle::Solid));
    // -5 > 0, 30 > 40, null > 1 and 20 > 20 are all false.
    assert!(styles[1..].iter().all(CellStyle::is_plain));
}

#[test]
fn display_shows_another_value_in_a_format() {
    let shown = |rules: &str, target: usize| -> Vec<Option<String>> {
        run(rules, target).into_iter().map(|s| s.display).collect()
    };
    let first = shown("display(c[0], blank='-'); if c[0] > 1 then bold", 3);
    assert_eq!(first, [Some("1".into()), Some("3".into()), Some("-".into()), Some("1".into()), Some("0".into())]);
    assert!(run("display(c[0]); if c[0] > 1 then bold", 3)[1].bold);
    let money = shown("display(c * 1000, '$#,##0;($#,##0)')", 1);
    assert_eq!(money[..3], [Some("$10,000".into()), Some("($5,000)".into()), Some("$30,000".into())]);
    // A blank value shows nothing unless `blank=` says what.
    assert_eq!(money[3], Some(String::new()));
    let named = shown("if c > \"target\" then display(upper(\"name\"))", 1);
    assert_eq!(named[..2], [Some("ANN".into()), None]);
    assert!(messages("display()")[0].contains("needs a value"));
    assert!(messages("display(c, 2)")[0].contains("text in quotes"));
    assert!(messages("display(c, '0 \"x')")[0].contains("isn't closed"));
    assert!(messages("display(c, '0', 'x')")[0].contains("a value and a format"));
}

#[test]
fn tooltips_add_a_line_for_each_rule_that_sets_one() {
    let tips = |rules: &str| -> Vec<Option<String>> { run(rules, 1).into_iter().map(|s| s.tooltip).collect() };
    let gap = tips("if c > \"target\" then tooltip('Over target'); tooltip(\"target\", '\"Target: \"#,##0')");
    assert_eq!(gap[0].as_deref(), Some("Over target\nTarget: 5"));
    assert_eq!(gap[1].as_deref(), Some("Target: 0"));
    // A blank value adds no line unless `blank=` says what.
    assert_eq!(tips("tooltip(c)")[3], None);
    assert_eq!(tips("tooltip(c, blank='No sales')")[3].as_deref(), Some("No sales"));
    // A tooltip leaves the cell's look alone, so it doesn't count as display.
    let styles = run("tooltip(upper(\"name\"))", 1);
    assert_eq!(styles[0].display, None);
    assert_eq!(styles[0].tooltip.as_deref(), Some("ANN"));
    assert!(messages("tooltip()")[0].contains("needs a value"));
    assert!(messages("tooltip(c, '0', 'x')")[0].contains("a value and a format"));
}

#[test]
fn duplicates_and_blanks() {
    let names = run("if duplicate(c) then bg: mistyrose", 0);
    let shaded: Vec<bool> = names.iter().map(|s| s.background.is_some()).collect();
    assert_eq!(shaded, [true, false, true, false, false]);
    let blanks = run("if c is null then bg: #eee; if unique(c) then italic", 1);
    assert!(blanks[3].background.is_some() && !blanks[3].italic);
    assert!(blanks[0].italic);
}

#[test]
fn elif_else_and_cascade_with_stop() {
    let styles = run("if c > 0 then text: green elif c < 0 then text: red else text: gray; bold", 1);
    assert_eq!(styles[0].text, Some(GREEN));
    assert_eq!(styles[1].text, Some(RED));
    assert_eq!(styles[3].text, Color::named("gray"));
    assert!(styles.iter().all(|s| s.bold));
    let stopped = run("if c < 0 then text: red, stop; bold", 1);
    assert!(!stopped[1].bold && stopped[0].bold);
    let later_wins = run("text: red; if c > 15 then text: green", 1);
    assert_eq!((later_wins[0].text, later_wins[2].text), (Some(RED), Some(GREEN)));
}

#[test]
fn color_scales_run_low_to_high() {
    let styles = run("scale(black, white)", 1);
    assert_eq!(styles[1].background, Some(Color::rgb(0, 0, 0)));
    assert_eq!(styles[2].background, Some(Color::rgb(255, 255, 255)));
    assert_eq!(styles[3].background, None);
    let middle = run("bg: scale(red, white, green, mid=0)", 1);
    assert_eq!(middle[1].background, Some(RED));
    let text = run("text: scale(black, white, min=0, max=20)", 1);
    assert_eq!(text[0].text, Some(Color::rgb(128, 128, 128)));
    assert_eq!(text[2].text, Some(Color::rgb(255, 255, 255)));
}

#[test]
fn bars_grow_from_zero() {
    let styles = run("bar(blue, red)", 1);
    // -5 to 30: zero sits at 5/35 of the width.
    let zero = 5.0 / 35.0;
    let bar = styles[2].bar.unwrap();
    assert!((bar.start - zero).abs() < 1e-6 && (bar.end - 1.0).abs() < 1e-6);
    let negative = styles[1].bar.unwrap();
    assert_eq!(negative.color, RED);
    assert!(negative.start.abs() < 1e-6 && (negative.end - zero).abs() < 1e-6);
    let positive = run("bar: green", 2);
    assert_eq!(positive[2].bar, Some(Bar { start: 0.0, end: 1.0, color: GREEN, gradient: false, axis: None }));
    assert_eq!(positive[1].bar.map(|b| b.end), Some(0.0));
}

#[test]
fn icon_sets_split_the_range_in_thirds() {
    let styles = run("icons(arrows)", 1);
    let shapes: Vec<Option<Shape>> = styles.iter().map(|s| s.icon.map(|i| i.shape)).collect();
    use Shape::*;
    assert_eq!(shapes, [Some(ArrowRight), Some(ArrowDown), Some(ArrowUp), None, Some(ArrowUp)]);
    let reversed = run("icons(arrows, reverse, low=0, high=15)", 1);
    assert_eq!(reversed[1].icon.map(|i| i.shape), Some(ArrowUp));
    assert_eq!(reversed[0].icon.map(|i| i.shape), Some(ArrowRight));
    let single = run("if c > 0 then icon: check", 1);
    assert_eq!(single[0].icon, Some(Icon { shape: Check, color: Color::rgb(0x21, 0xa3, 0x66) }));
}

#[test]
fn ranks_tops_and_aggregates() {
    let top = run("if top(c, 2) then bold", 1);
    assert_eq!(top.iter().map(|s| s.bold).collect::<Vec<_>>(), [false, false, true, false, true]);
    let above = run("if c > avg(c) then bold", 1);
    // The mean of 10, -5, 30 and 20 is 13.75.
    assert_eq!(above.iter().map(|s| s.bold).collect::<Vec<_>>(), [false, false, true, false, true]);
    let ranked = run("if rank(c) = 1 or percent_rank(c) = 0 then bold", 1);
    assert_eq!(ranked.iter().map(|s| s.bold).collect::<Vec<_>>(), [false, true, true, false, false]);
    let other = run("if \"sales\" >= percentile(\"sales\", 50%) then underline", 0);
    assert_eq!(other.iter().map(|s| s.underline).collect::<Vec<_>>(), [false, false, true, false, true]);
}

#[test]
fn text_tests_and_functions() {
    let styles = run("if c in ('bob', 'cy') or c like 'a%' then bold; if len(c) = 3 and c contains 'E' then italic", 0);
    assert_eq!(styles.iter().map(|s| s.bold).collect::<Vec<_>>(), [true, true, true, true, false]);
    assert!(styles[4].italic && !styles[0].italic);
    let rows = run("if row() % 2 = 0 then bg: #f2f2f2", 0);
    assert_eq!(rows.iter().map(|s| s.background.is_some()).collect::<Vec<_>>(), [false, true, false, true, false]);
    let between = run("if c not between 0 and 20 then strike", 1);
    assert_eq!(between.iter().map(|s| s.strike).collect::<Vec<_>>(), [false, true, true, false, false]);
}

#[test]
fn sparklines_read_lists_or_columns() {
    let lists = run("sparkline(c, kind=bars), hide_text", 3);
    let spark = lists[3].sparkline.as_ref().unwrap();
    assert_eq!(spark.values, [Some(1.0), None, Some(4.0)]);
    assert!(spark.bars && lists[3].hide_text);
    let row = run("sparkline(\"sales\", \"target\", color=red)", 0);
    assert_eq!(row[0].sparkline.as_ref().unwrap().values, [Some(10.0), Some(5.0)]);
}

#[test]
fn problems_say_what_to_do() {
    assert_eq!(messages("if c > \"salse\" then bold"), ["no column \"salse\"; did you mean \"sales\"?"]);
    assert_eq!(
        messages("if c = \"failed\" then bold"),
        ["no column \"failed\": double quotes name a column, single quotes are text ('failed')"]
    );
    assert_eq!(messages("if c > 0 then bg: rde"), ["unknown color `rde`; did you mean `red`?"]);
    assert_eq!(messages("if c > 0 then bolf"), ["unknown formatting `bolf`; did you mean `bold`?"]);
    assert_eq!(messages("if c > 0 then red"), ["`red` is a color: write `bg: red` or `text: red`"]);
    assert_eq!(messages("bg: red bold"), ["add a `,` before `bold`"]);
    assert_eq!(messages("if sales > 1 then bold"), ["to use the column, write it in double quotes: \"sales\""]);
    assert_eq!(messages("if avg(5) > 1 then bold"), ["`avg` takes a column: `c` or a \"column\""]);
    assert_eq!(messages("bar(green, fil=gradient)"), ["`bar` has no `fil=`; did you mean `fill=`?"]);
    assert_eq!(messages("icons(arrow)"), ["unknown icon set `arrow`; did you mean `arrows`?"]);
    assert_eq!(messages("bold: 1"), ["`bold` takes no value: just `bold`"]);
    assert_eq!(messages("bg"), ["`bg` needs a value: bg: <color> | scale(…)"]);
    assert_eq!(messages("bg: #12345"), ["`#12345` isn't a color: use #rgb or #rrggbb"]);
}

#[test]
fn rules_with_errors_are_left_out_and_the_rest_run() {
    let (columns, _) = table();
    let checked = check("if c > 0 then bold; if c > \"nope\" then italic; underline", &columns);
    assert_eq!(checked.diagnostics.len(), 1);
    assert_eq!(checked.program.rules.len(), 2);
}

#[test]
fn needs_list_what_a_host_must_compute() {
    use diver_format::program::Need;
    let (columns, _) = table();
    let checked = check("if duplicate(c) then bold; scale(red, green); if c > avg(\"target\") then italic", &columns);
    let needs = checked.program.needs(1);
    assert!(needs.contains(&(1, Need::Frequency)));
    assert!(needs.contains(&(1, Need::Aggregate(Aggregate::Min))));
    assert!(needs.contains(&(2, Need::Aggregate(Aggregate::Mean))));
}

#[test]
fn list_items_and_struct_fields_in_conditions() {
    let columns = vec![
        Column::new("scores", ValueKind::List),
        Column::new("info", ValueKind::Struct),
    ];
    let rows = [
        ("[5, 1, 9]", "{'kind': 'gold', 'place name': 'Oslo', 'inner': {'n': 3}, 'xs': [7]}"),
        ("[0]", "{'kind': silver, 'place name': NULL, 'inner': {'n': 1}, 'xs': []}"),
        ("NULL", "NULL"),
    ];
    let data: Vec<Vec<Value>> = vec![
        rows.iter().map(|(list, _)| Value::from_text((*list != "NULL").then_some(*list), ValueKind::List)).collect(),
        rows.iter().map(|(_, info)| Value::from_text((*info != "NULL").then_some(*info), ValueKind::Struct)).collect(),
    ];
    let bold = |rules: &str, target: usize| -> Vec<bool> {
        let checked = check(rules, &columns);
        assert!(checked.diagnostics.is_empty(), "{rules}: {:?}", checked.diagnostics);
        let stats = TableStats::new(&data);
        (0..rows.len())
            .map(|r| {
                let values: Vec<Value> = data.iter().map(|c| c[r].clone()).collect();
                checked.program.style(target, &ValuesRow { values: &values, index: r }, &stats).bold
            })
            .collect()
    };
    assert_eq!(bold("if c[0] > 1 then bold", 0), [true, false, false]);
    assert_eq!(bold("if c[-1] = 9 then bold", 0), [true, false, false]);
    assert_eq!(bold("if c[5] is null then bold", 0), [true, true, true]);
    assert_eq!(bold("if c.kind = 'gold' then bold", 1), [true, false, false]);
    assert_eq!(bold("if \"info\".kind = 'silver' then bold", 0), [false, true, false]);
    assert_eq!(bold("if c.\"place name\" is null then bold", 1), [false, true, true]);
    assert_eq!(bold("if c.inner.n + \"scores\"[0] > 7 then bold", 1), [true, false, false]);
    assert_eq!(bold("if c.xs[0] = 7 and c['kind'] = 'gold' then bold", 1), [true, false, false]);
    assert_eq!(bold("if (c.inner).n = 1 then bold", 1), [false, true, false]);
}

#[test]
fn index_and_field_mistakes_are_reported() {
    let columns = vec![Column::new("info", ValueKind::Struct)];
    let messages = |rules: &str| -> Vec<String> {
        check(rules, &columns).diagnostics.into_iter().map(|d| d.message).collect()
    };
    assert_eq!(messages("if c[0 > 1 then bold"), ["expected `]`, found `then`"]);
    assert_eq!(messages("if c. > 1 then bold"), ["expected a field name after `.`, found `>`"]);
}
