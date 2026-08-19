WHENEVER SQLERROR EXIT SQL.SQLCODE
CONNECT spike/spike123@//oracle:1521/XE
SET ECHO ON
SET FEEDBACK ON

-- 第一版验收（C1-C6）的源端 fixture —— ADR-0040 §4。
--
-- **另起文件，不改 oracle.sql**：M1 的 `t_m1_narrow` / `t_m1_wide` 是常量基线（ADR-0040 §1），
-- 往里加列或加行会让 2026-08-16 那几份 9/9 报告对不上。照 M3 的 `oracle-m3.sql` 先例。
--
-- **C6 不在这里建表**：它量的是 10 万行宽表的内存形状，源表就是 M1 的 `t_m1_wide`
-- （ADR-0040 §3.3 字面「同一张宽表」）。这里只建 C2-C5 要的四张小表；C6 的目标端
-- 另起一张 `V1_WIDE`（见 mysql-v1.sql），免得 C6 跑完把 M1_WIDE 的行留给下一份台架。
--
-- 字符列一律 ASCII 且目标端留足四倍长度：字符/字节那道坎归 ADR-0033，本轮不去踩它。

DECLARE
  TYPE table_name_list IS TABLE OF VARCHAR2(30);
  table_names table_name_list := table_name_list(
    'T_V1_C2', 'T_V1_C3', 'T_V1_C4', 'T_V1_C5'
  );
BEGIN
  FOR index_value IN 1..table_names.COUNT LOOP
  BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE ' || table_names(index_value) || ' PURGE';
  EXCEPTION WHEN OTHERS THEN
    IF SQLCODE != -942 THEN RAISE; END IF;
  END;
  END LOOP;
END;
/

-- C2：源列名与目标列名**成心取得不一样**，跑通后按目标名核对（ADR-0040 §4 C2 ①）。
CREATE TABLE t_v1_c2 (
  row_id     NUMBER(8),
  src_name   VARCHAR2(20 CHAR),
  src_amount NUMBER(10,2),
  load_date  DATE
);

INSERT ALL
  INTO t_v1_c2 VALUES (1, 'alpha', 10.25, DATE '2026-08-14')
  INTO t_v1_c2 VALUES (2, 'bravo', 20.50, DATE '2026-08-14')
  INTO t_v1_c2 VALUES (3, 'delta', 30.75, DATE '2026-08-14')
SELECT * FROM dual;

-- C3：一个常量条件、一个运行时填的条件，各跑一次，行数按预期变（C3 ①）。
-- 分组刻意不等量：grp='A' 三行、grp='B' 两行，行数变了才说明条件真的进了 SQL。
CREATE TABLE t_v1_c3 (
  row_id    NUMBER(8),
  grp       VARCHAR2(4 CHAR),
  load_date DATE
);

INSERT ALL
  INTO t_v1_c3 VALUES (1, 'A', DATE '2026-08-14')
  INTO t_v1_c3 VALUES (2, 'A', DATE '2026-08-14')
  INTO t_v1_c3 VALUES (3, 'A', DATE '2026-08-14')
  INTO t_v1_c3 VALUES (4, 'B', DATE '2026-08-14')
  INTO t_v1_c3 VALUES (5, 'B', DATE '2026-08-14')
SELECT * FROM dual;

-- C4：主键 upsert 的幂等（C4 ①-⑤）。第 ④ 条要改一列源值重跑，所以 v_text 必须可改。
CREATE TABLE t_v1_c4 (
  row_id    NUMBER(8),
  v_text    VARCHAR2(20 CHAR),
  load_date DATE
);

INSERT ALL
  INTO t_v1_c4 VALUES (1, 'first', DATE '2026-08-14')
  INTO t_v1_c4 VALUES (2, 'second', DATE '2026-08-14')
  INTO t_v1_c4 VALUES (3, 'third', DATE '2026-08-14')
  INTO t_v1_c4 VALUES (4, 'fourth', DATE '2026-08-14')
  INTO t_v1_c4 VALUES (5, 'fifth', DATE '2026-08-14')
SELECT * FROM dual;

-- C5：映射预检三分支的三条负向/正向用例（C5 ①②③）。同一张源表打三张不同形状的目标表，
-- 分支差别全在目标端 DDL 上——源端只提供一份干净的两列数据。
CREATE TABLE t_v1_c5 (
  row_id    NUMBER(8),
  v_text    VARCHAR2(20 CHAR),
  load_date DATE
);

INSERT ALL
  INTO t_v1_c5 VALUES (1, 'c5-one', DATE '2026-08-14')
  INTO t_v1_c5 VALUES (2, 'c5-two', DATE '2026-08-14')
SELECT * FROM dual;

DECLARE
  c2_rows NUMBER;
  c3_rows NUMBER;
  c4_rows NUMBER;
  c5_rows NUMBER;
BEGIN
  SELECT COUNT(*) INTO c2_rows FROM t_v1_c2;
  SELECT COUNT(*) INTO c3_rows FROM t_v1_c3;
  SELECT COUNT(*) INTO c4_rows FROM t_v1_c4;
  SELECT COUNT(*) INTO c5_rows FROM t_v1_c5;
  IF c2_rows != 3 OR c3_rows != 5 OR c4_rows != 5 OR c5_rows != 2 THEN
    RAISE_APPLICATION_ERROR(-20001, 'v1 acceptance fixtures have unexpected row counts');
  END IF;
END;
/

EXIT
