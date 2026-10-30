array_agg(x)
array_agg(x ORDER BY y)
avg(x)
corr(x, y)
count(x)
count(*)
count(DISTINCT x)
covar_pop(x, y)
first(x)
last(x)
max(x)
median(x)
min(x)
quantile_cont(x, 0.9)
quantile_disc(x, 0.9)
stddev(x)
string_agg(label, ', ')
string_agg(DISTINCT label, ', ' ORDER BY label)
sum(x)
total(x)
variance(x)
count(*) AS Rows
sum("Score") -- trailing comment
year(day) AS y
date_part('month', day) AS m
amount > 1 AS big
x > 1 AND label = 'b'
label ILIKE 'a%'
label LIKE 'b%' OR label NOT LIKE '%c'
x IS NOT NULL
x IS NULL OR y IS NOT NULL
x IN (1, 2, 4)
x NOT IN (1, 2)
x BETWEEN 1 AND 3
x NOT BETWEEN 1 AND 3
CASE WHEN x > 2 THEN 'big' ELSE 'small' END
CASE label WHEN 'a' THEN 1 ELSE 0 END AS flag
x::VARCHAR
CAST(x AS INTEGER)
TRY_CAST(label AS DOUBLE)
x * 2 + y / 3 - 1
-x
abs(x - y)
round(x, 1)
coalesce(x, y, 0)
nullif(x, 1)
upper(label) || '_' || lower(label)
concat(label, '-', label)
substr(label, 1, 2)
length(label)
strftime(day, '%Y-%m')
strptime('2024-01-01', '%Y-%m-%d')
date_part('year', day)
sum(x) FILTER (WHERE y > 1)
max(x) - min(x) AS spread
sum(x) / count(*) AS mean
x IS DISTINCT FROM y
label ~ '^a'
regexp_like(label, 'b')
row_number() OVER (ORDER BY x) AS rn
sum(x) OVER (PARTITION BY label ORDER BY y) AS running
x % 2 = 0
"x" + "y"
DATE '2024-01-01'
INTERVAL '1 day'
