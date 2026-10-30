SELECT * FROM t1 ASOF LEFT JOIN t2 USING (a) JOIN LATERAL (SELECT 1) s ON true, 'f.parquet' f, read_csv('x.csv', header := true) r;
SELECT * FROM t PIVOT (sum(x) FOR y IN ('a', 'b')) p;
