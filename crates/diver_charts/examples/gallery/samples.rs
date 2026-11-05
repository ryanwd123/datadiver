//! The gallery's examples, grouped by chart type.

use diver_charts::BarChart;

pub struct Sample {
    pub group: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub chart: BarChart,
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Repeatable noise in [0, 1), so the samples look the same every run.
fn noise(seed: u64) -> f64 {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(0x6a09_e667);
    x ^= x >> 31;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

pub fn all() -> Vec<Sample> {
    let mut out = Vec::new();
    let bar = "Bar charts";

    out.push(Sample {
        group: bar,
        name: "Columns",
        about: "One series over twelve months: no legend, the title names it.",
        chart: BarChart::new(MONTHS)
            .series("Revenue", [182_400.0, 171_900.0, 205_300.0, 219_800.0, 236_100.0, 248_700.0, 231_500.0, 226_900.0, 252_300.0, 268_400.0, 291_700.0, 334_200.0])
            .title("Monthly revenue")
            .subtitle("2025, all stores")
            .value_title("Revenue ($)"),
    });

    out.push(Sample {
        group: bar,
        name: "Value labels",
        about: "Values printed at the bar ends, compact where they are large.",
        chart: BarChart::new(["Widgets", "Gadgets", "Sprockets", "Gizmos", "Doohickeys"])
            .series("Units sold", [48_210.0, 39_870.0, 27_455.0, 12_930.0, 8_104.0])
            .title("Top products by units sold")
            .subtitle("Last 90 days")
            .value_labels(),
    });

    let mut sources = vec![
        ("customers_2025_final.parquet", 1_284_330.0f64),
        ("orders.csv", 9_420_118.0),
        ("order_items_with_a_very_long_descriptive_name.parquet", 31_877_402.0),
        ("products.xlsx", 18_244.0),
        ("returns.csv", 412_903.0),
        ("inventory_snapshots.parquet", 5_602_771.0),
        ("web_sessions.jsonl", 22_190_644.0),
        ("stores.csv", 1_204.0),
        ("employees.xlsx", 3_381.0),
        ("shipments.parquet", 7_788_015.0),
    ];
    sources.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.push(Sample {
        group: bar,
        name: "Horizontal, sorted",
        about: "Long names read level on the left; the longest is cut to fit.",
        chart: BarChart::new(sources.iter().map(|s| s.0))
            .series("Rows", sources.iter().map(|s| s.1))
            .title("Rows by source file")
            .horizontal()
            .value_labels(),
    });

    out.push(Sample {
        group: bar,
        name: "Grouped",
        about: "Three series side by side in each quarter, with a legend.",
        chart: BarChart::new(["Q1", "Q2", "Q3", "Q4"])
            .series("North", [420.0, 465.0, 510.0, 590.0])
            .series("Central", [380.0, 360.0, 402.0, 455.0])
            .series("South", [290.0, 340.0, 365.0, 371.0])
            .title("Sales by region")
            .subtitle("Thousands of dollars, 2025")
            .category_title("Quarter"),
    });

    let weeks: Vec<String> = (1..=10).map(|w| format!("Wk {w}")).collect();
    let tickets = |base: f64, seed: u64| -> Vec<f64> { (0..10).map(|w| (base * (0.7 + 0.6 * noise(seed + w))).round()).collect() };
    out.push(Sample {
        group: bar,
        name: "Stacked",
        about: "Parts of each week's total, with the total at the top of the stack.",
        chart: BarChart::new(weeks.clone())
            .series("Low", tickets(64.0, 10))
            .series("Medium", tickets(41.0, 30))
            .series("High", tickets(18.0, 50))
            .series("Urgent", tickets(6.0, 70))
            .title("Support tickets by priority")
            .stacked()
            .value_labels(),
    });

    out.push(Sample {
        group: bar,
        name: "Negative values",
        about: "Bars hang below a zero baseline; the rounded end is the data end.",
        chart: BarChart::new(MONTHS)
            .series("Net change", [12.4, 8.1, -3.6, -9.8, 4.2, 15.7, 21.3, -1.2, -14.6, 6.9, 11.0, 18.5])
            .title("Net subscriber change")
            .subtitle("Thousands, month over month")
            .value_labels(),
    });

    out.push(Sample {
        group: bar,
        name: "Grouped, negative",
        about: "Two series around zero, side by side.",
        chart: BarChart::new(["Hardware", "Software", "Services", "Support", "Training", "Licensing"])
            .series("2024", [4.2, 11.8, -2.1, 1.4, -0.6, 7.3])
            .series("2025", [5.9, 9.4, 1.7, -1.1, 0.8, 8.8])
            .title("Operating margin by segment")
            .value_title("Margin (%)"),
    });

    let states = [
        "Alabama", "Alaska", "Arizona", "Arkansas", "California", "Colorado", "Connecticut", "Delaware", "Florida",
        "Georgia", "Hawaii", "Idaho", "Illinois", "Indiana", "Iowa", "Kansas", "Kentucky", "Louisiana", "Maine",
        "Maryland", "Massachusetts", "Michigan", "Minnesota", "Mississippi", "Missouri", "Montana", "Nebraska",
        "Nevada", "New Hampshire", "New Jersey",
    ];
    out.push(Sample {
        group: bar,
        name: "Many categories",
        about: "Thirty columns: names turn 45° when they cannot sit level.",
        chart: BarChart::new(states)
            .series("Stores", (0..states.len()).map(|i| (20.0 + 380.0 * noise(i as u64 * 7).powi(2)).round()))
            .title("Stores by state"),
    });

    let teams = ["Platform", "Data", "Mobile", "Web", "Infra", "Security"];
    let hours = |base: f64, seed: u64| -> Vec<f64> { (0..teams.len()).map(|t| (base * (0.4 + 1.2 * noise(seed + t as u64))).round()).collect() };
    out.push(Sample {
        group: bar,
        name: "Stacked horizontal",
        about: "A stack per team, running right.",
        chart: BarChart::new(teams)
            .series("Building", hours(18.0, 100))
            .series("Reviewing", hours(7.0, 200))
            .series("Meetings", hours(9.0, 300))
            .series("On call", hours(4.0, 400))
            .series("Learning", hours(3.0, 500))
            .title("Where the week goes")
            .subtitle("Average hours per engineer")
            .horizontal()
            .stacked()
            .value_title("Hours"),
    });

    let years: Vec<String> = (2016..=2025).map(|y| y.to_string()).collect();
    let mut many = BarChart::new(years.clone())
        .title("Downloads by platform")
        .subtitle("Past the eighth, series fold into \"Other\"")
        .stacked();
    let platforms = ["Windows", "macOS", "Linux", "iOS", "Android", "Web", "ChromeOS", "FreeBSD", "Solaris", "Haiku", "Plan 9"];
    for (p, name) in platforms.iter().enumerate() {
        let share = 1.0 / (p as f64 + 1.0).powf(1.3);
        many = many.series(*name, (0..years.len()).map(|y| (90_000.0 * share * (1.0 + y as f64 * 0.18) * (0.85 + 0.3 * noise((p * 31 + y) as u64))).round()));
    }
    out.push(Sample { group: bar, name: "Eleven series", about: "The palette has eight colors; the rest become one gray \"Other\".", chart: many });

    out.push(Sample {
        group: bar,
        name: "Missing values",
        about: "A missing value draws no bar; its tooltip says so.",
        chart: BarChart::new(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"])
            .series_opt("This week", [Some(312.0), Some(298.0), None, Some(344.0), Some(401.0), None, None])
            .series_opt("Last week", [Some(287.0), Some(305.0), Some(322.0), Some(318.0), Some(377.0), Some(452.0), Some(398.0)])
            .title("Daily orders"),
    });

    out
}
