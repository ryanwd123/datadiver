//! Parse and complete on a typical query and on a 5k-line script.

use criterion::{Criterion, criterion_group, criterion_main};
use diver_sql::{TextSize, complete, complete_with, parse};

const QUERY: &str = "WITH recent AS (
    SELECT o.order_id, o.customer_id, o.amount, o.\"order date\" AS ordered
    FROM orders o
    WHERE o.\"order date\" >= DATE '2024-01-01'
      AND o.amount > 0
),
totals AS (
    SELECT customer_id, sum(amount) AS total, count(*) AS n
    FROM recent
    GROUP BY ALL
)
SELECT c.name,
       c.region,
       t.total,
       t.n,
       rank() OVER (PARTITION BY c.region ORDER BY t.total DESC) AS rnk,
       CASE WHEN t.total > 1000 THEN 'big'
            WHEN t.total > 100 THEN 'medium'
            ELSE 'small' END AS size,
       list_transform([1, 2, 3], x -> x * t.n) AS l
FROM customers c
JOIN totals t USING (customer_id)
LEFT JOIN read_parquet('extra/*.parquet') e ON e.customer_id = c.customer_id
WHERE c.region IN ('north', 'south')
  AND c.name ILIKE 'a%'
QUALIFY rnk <= 10
ORDER BY c.region, t.total DESC
LIMIT 100;
";

fn script() -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut corpus = String::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        corpus.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
        corpus.push('\n');
    }
    let mut out = String::new();
    while out.lines().count() < 5000 {
        out.push_str(&corpus);
        out.push_str(QUERY);
    }
    out
}

fn bench(c: &mut Criterion) {
    let script = script();
    let cursor = TextSize::new(QUERY.find("c.region,").unwrap() as u32 + 2);
    let script_cursor = TextSize::new((script.len() / 2) as u32);
    let catalog = diver_sql::EmptyCatalog;
    c.bench_function("parse query", |b| b.iter(|| parse(QUERY)));
    c.bench_function("complete query", |b| b.iter(|| complete(QUERY, cursor)));
    c.bench_function("complete_with query", |b| {
        b.iter(|| complete_with(QUERY, cursor, &catalog))
    });
    c.bench_function("parse 5k-line script", |b| b.iter(|| parse(&script)));
    c.bench_function("complete in 5k-line script", |b| {
        b.iter(|| complete(&script, script_cursor))
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
