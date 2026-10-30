COPY (SELECT 1) TO 'out.parquet' (FORMAT parquet);
ATTACH 'x.db' AS x (READ_ONLY);
SET GLOBAL threads = 4;
PIVOT t ON y USING sum(x) GROUP BY z;
PRAGMA table_info('t');
frobnicate the database;
