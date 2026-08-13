-- 台架初始化 step 4 —— 指回自身的 dblink，供 #6 看列投影是否下推。
-- 真实源端有一张表经 dblink 指向更远端的库（见 docs/spikes/fixtures/README.md）；
-- 这里用「loopback dblink」复现那条链路的形状：同一个库，但走远程优化器路径。
CONNECT spike/spike123@//localhost:1521/XE
SET ECHO ON
SET FEEDBACK ON

-- 名字沿用生产里的 @FA，让 #6 的 SQL 与 #2 采集脚本里的写法一致。
CREATE DATABASE LINK fa
  CONNECT TO spike IDENTIFIED BY spike123
  USING 'localhost:1521/XE';

-- 冒烟：能连通即算成功
SELECT 'dblink ok: ' || COUNT(*) AS smoke FROM t_types_probe@fa;

-- #6 的观察点：只投影一列时，远程 SQL 里到底带没带列裁剪。
-- 看计划用：
--   EXPLAIN PLAN FOR SELECT n_bare FROM t_bulk_probe@fa WHERE row_id < 10;
--   SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY);
-- 关键是 REMOTE 行的 "Remote SQL Information" 段 —— 那里才是真正发到远端的 SQL。
EXPLAIN PLAN FOR SELECT n_amount FROM t_bulk_probe@fa WHERE row_id < 10;
SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY);

EXIT
