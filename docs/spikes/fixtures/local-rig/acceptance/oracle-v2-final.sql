WHENEVER SQLERROR EXIT SQL.SQLCODE
CONNECT spike/spike123@//oracle:1521/XE
SET ECHO ON
SET FEEDBACK ON

-- #157 终局演练的「客户表」—— 装机那天要搬的那张表在演练台上的替身。
--
-- **另起文件，不改前面几份**：`oracle.sql`（M1 基线）、`oracle-v1.sql`（C1–C6）都是常量，
-- 往里加表会让此前那几份报告对不上（ADR-0040 §1 的同一条纪律）。这里只碰 `T_V2_*`。
--
-- 形状照客户那张表的最小面：一个数字主键、一个字符列、一个金额列、一个业务日期列。
-- **两个业务日期、行数刻意不等量**（08-20 五行、08-19 两行）：过滤条件真的进了 SQL，
-- 目标库里就该只有五行；不等量才分得出「过滤生效」与「整表搬了一遍」。
--
-- 字符列一律 ASCII：字符/字节那道坎归 ADR-0033，终局演练不去踩它（与 oracle-v1.sql 同）。

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE t_v2_trial PURGE';
EXCEPTION WHEN OTHERS THEN
  IF SQLCODE != -942 THEN RAISE; END IF;
END;
/

CREATE TABLE t_v2_trial (
  row_id    NUMBER(8),
  cust_name VARCHAR2(20 CHAR),
  amount    NUMBER(12,2),
  load_date DATE
);

INSERT ALL
  INTO t_v2_trial VALUES (1, 'alpha',   1024.50, DATE '2026-08-20')
  INTO t_v2_trial VALUES (2, 'bravo',      0.01, DATE '2026-08-20')
  INTO t_v2_trial VALUES (3, 'charlie', 99999.99, DATE '2026-08-20')
  INTO t_v2_trial VALUES (4, 'delta',    250.00, DATE '2026-08-20')
  INTO t_v2_trial VALUES (5, 'echo',    3333.33, DATE '2026-08-20')
  INTO t_v2_trial VALUES (6, 'foxtrot',   12.34, DATE '2026-08-19')
  INTO t_v2_trial VALUES (7, 'golf',      56.78, DATE '2026-08-19')
SELECT * FROM dual;
COMMIT;

SELECT COUNT(*) AS total_rows FROM t_v2_trial;
SELECT TO_CHAR(load_date,'YYYY-MM-DD') AS load_date, COUNT(*) AS rows_of_day
  FROM t_v2_trial GROUP BY TO_CHAR(load_date,'YYYY-MM-DD') ORDER BY 1;
EXIT
