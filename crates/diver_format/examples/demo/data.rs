//! The demo's sample table and starting format sets.

use diver_format::value::{format_date, today_utc};
use diver_format::{Column, FormatSet, Value, ValueKind};

pub struct Table {
    pub columns: Vec<Column>,
    /// Values by column.
    pub values: Vec<Vec<Value>>,
    /// What the grid shows, by column.
    pub text: Vec<Vec<String>>,
    pub widths: Vec<f32>,
}

impl Table {
    pub fn rows(&self) -> usize {
        self.values.first().map_or(0, Vec::len)
    }
}

/// A small deterministic generator, so captures look the same every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() * items.len() as f64) as usize % items.len()]
    }
}

fn grouped(n: f64, decimals: usize) -> String {
    let text = format!("{:.*}", decimals, n.abs());
    let (whole, fraction) = text.split_once('.').map_or((text.as_str(), ""), |(w, f)| (w, f));
    let mut out = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let sign = if n < 0.0 { "-" } else { "" };
    if fraction.is_empty() { format!("{sign}{out}") } else { format!("{sign}{out}.{fraction}") }
}

pub fn sample(rows: usize) -> Table {
    let spec: [(&str, ValueKind, f32); 15] = [
        ("id", ValueKind::Number, 48.0),
        ("region", ValueKind::Text, 62.0),
        ("product", ValueKind::Text, 104.0),
        ("units", ValueKind::Number, 56.0),
        ("price", ValueKind::Number, 64.0),
        ("revenue", ValueKind::Number, 110.0),
        ("target", ValueKind::Number, 80.0),
        ("growth", ValueKind::Number, 82.0),
        ("ship_date", ValueKind::Date, 86.0),
        ("status", ValueKind::Text, 64.0),
        ("q1", ValueKind::Number, 50.0),
        ("q2", ValueKind::Number, 50.0),
        ("q3", ValueKind::Number, 50.0),
        ("q4", ValueKind::Number, 50.0),
        ("trend", ValueKind::List, 104.0),
    ];
    let products = [
        "Widget", "Gadget", "Gizmo", "Sprocket", "Flange", "Doohickey", "Bracket", "Hinge", "Valve", "Rotor",
        "Spindle", "Gasket", "Lever", "Pulley", "Coupler", "Bearing", "Nozzle", "Ratchet", "Sensor", "Bolt",
        "Clamp", "Damper", "Filter", "Gear", "Hub", "Joint", "Knob", "Latch", "Mount", "Nut", "Pin", "Rivet",
        "Shaft", "Spring", "Switch", "Tube", "Washer", "Wheel", "Axle", "Cam", "Crank", "Disc", "Fuse", "Lens",
    ];
    let mut rng = Lcg(42);
    let today = today_utc();
    let mut values = vec![Vec::new(); spec.len()];
    let mut text = vec![Vec::new(); spec.len()];
    for row in 0..rows {
        let units = (rng.next() * 400.0).round() + 5.0;
        let price = (rng.next() * 90.0 + 4.0) * 4.0;
        let price = (price.round()) / 4.0;
        let revenue = units * price;
        let target = revenue * (0.6 + rng.next() * 0.8);
        let growth = ((rng.next() - 0.4) * 60.0).round() / 100.0;
        let days = (rng.next() * 140.0) as i64 - 15;
        let status = match rng.next() {
            x if x < 0.08 => None,
            x if x < 0.18 => Some("failed"),
            x if x < 0.36 => Some("late"),
            _ => Some("ok"),
        };
        let quarters: Vec<f64> = (0..4).map(|_| (rng.next() * 100.0).round()).collect();
        let trend: Vec<f64> = (0..8).map(|i| ((rng.next() * 10.0) + i as f64 * (growth * 10.0)).round()).collect();
        let cells: Vec<(Value, String)> = vec![
            (Value::Number(1001.0 + row as f64), format!("{}", 1001 + row)),
            {
                let region = rng.pick(&["North", "South", "East", "West"]);
                (Value::Text(region.into()), region.into())
            },
            {
                let product = rng.pick(&products);
                (Value::Text(product.into()), product.into())
            },
            (Value::Number(units), grouped(units, 0)),
            (Value::Number(price), format!("{price:.2}")),
            (Value::Number(revenue), grouped(revenue, 2)),
            (Value::Number(target.round()), grouped(target.round(), 0)),
            (Value::Number(growth), format!("{:+.0}%", growth * 100.0)),
            {
                let date = format_date(today - days);
                (Value::Text(date.clone()), date)
            },
            (status.map_or(Value::Null, |s| Value::Text(s.into())), status.unwrap_or("").into()),
            (Value::Number(quarters[0]), format!("{}", quarters[0])),
            (Value::Number(quarters[1]), format!("{}", quarters[1])),
            (Value::Number(quarters[2]), format!("{}", quarters[2])),
            (Value::Number(quarters[3]), format!("{}", quarters[3])),
            {
                let list = Value::List(trend.iter().map(|v| Value::Number(*v)).collect());
                let shown = list.text().into_owned();
                (list, shown)
            },
        ];
        for (i, (value, shown)) in cells.into_iter().enumerate() {
            values[i].push(value);
            text[i].push(shown);
        }
    }
    Table {
        columns: spec.iter().map(|(name, kind, _)| Column::new(*name, *kind)).collect(),
        values,
        text,
        widths: spec.iter().map(|(.., w)| *w).collect(),
    }
}

pub fn starting_sets() -> Vec<FormatSet> {
    let set = |rules: &str, columns: &[&str]| FormatSet {
        rules: rules.to_string(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
    };
    vec![
        set(
            "-- Bars as long as the revenue; bold where it beat the target.\n\
             bar(#638ec6, fill=gradient);\n\
             if c > \"target\" then bold, border: 1px #2e7d32",
            &["revenue"],
        ),
        set(
            "if c > 0 then text: #006100, icon: up\nelif c < 0 then text: #c00000, icon: down\nelse text: gray",
            &["growth"],
        ),
        set("if duplicate(c) then bg: #ffc7ce, text: #9c0006", &["product"]),
        set("scale(#f8696b, #ffeb84, #63be7b)", &["units"]),
        set(
            "if c = 'failed' then bg: mistyrose, text: firebrick, bold;\n\
             if c = 'late' then text: darkorange, italic;\n\
             if c is null then bg: #eeeeee",
            &["status"],
        ),
        set(
            "if days_ago(c) < 0 then text: steelblue, underline\n\
             elif days_ago(c) <= 30 then bold\n\
             else text: gray",
            &["ship_date"],
        ),
        set("icons(traffic)", &["q1", "q2", "q3", "q4"]),
        set("sparkline(c), hide_text", &["trend"]),
    ]
}
