-- #6 探针第二轮 —— 补掉第一轮的两个洞：
--   洞 1：PL/SQL 循环体不引用列，列被优化掉，字节数量不出差异 → 这里真消费列值。
--   洞 2：第一轮的查询只碰远程对象，Oracle 判成 "fully remote statement"，
--        整条语句发到远端跑，压根没有 REMOTE 行源，也就没有 Remote SQL Information。
--        真实生产查询很可能还 join 本地表 → 这里补一个混合场景，逼出 REMOTE 行源。
SET ECHO OFF
SET FEEDBACK OFF
SET LINESIZE 200
SET PAGESIZE 1000
SET SERVEROUTPUT ON SIZE UNLIMITED
WHENEVER SQLERROR CONTINUE

PROMPT ================================================================
PROMPT 0. 准备：一张本地小维表，用来把查询变成「本地 + 远程」混合
PROMPT ================================================================
BEGIN EXECUTE IMMEDIATE 'DROP TABLE t_local_dim PURGE'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
CREATE TABLE t_local_dim (row_id NUMBER(8) PRIMARY KEY, tag VARCHAR2(20));
INSERT INTO t_local_dim SELECT LEVEL, 'tag' || LEVEL FROM dual CONNECT BY LEVEL <= 5000;
COMMIT;
BEGIN DBMS_STATS.GATHER_TABLE_STATS(USER, 'T_LOCAL_DIM'); END;
/

PROMPT
PROMPT ================================================================
PROMPT 1. 混合形状 A（生产原样：内层 SELECT *）—— 看 Remote SQL Information
PROMPT ================================================================
EXPLAIN PLAN SET STATEMENT_ID = 'MA' FOR
SELECT t.row_id, t.c01, t.c02, l.tag
FROM (
    SELECT * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t
JOIN t_local_dim l ON l.row_id = t.row_id;
SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'MA', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 2. 混合形状 B（ADR-0004 建议：投影写进内层）
PROMPT ================================================================
EXPLAIN PLAN SET STATEMENT_ID = 'MB' FOR
SELECT t.row_id, t.c01, t.c02, l.tag
FROM (
    SELECT a.row_id, a.c01, a.c02 FROM t_wide_probe@fa a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t
JOIN t_local_dim l ON l.row_id = t.row_id;
SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'MB', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 3. 混合形状 A + NO_MERGE（内层子查询不许被合并 —— 最坏情况）
PROMPT ================================================================
EXPLAIN PLAN SET STATEMENT_ID = 'MN' FOR
SELECT t.row_id, t.c01, t.c02, l.tag
FROM (
    SELECT /*+ NO_MERGE */ * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t
JOIN t_local_dim l ON l.row_id = t.row_id;
SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'MN', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 4. 实测字节 —— 这一轮真的消费列值，防止列被优化掉
PROMPT ================================================================
DECLARE
  b NUMBER; e NUMBER; n NUMBER; L NUMBER; t0 NUMBER; t1 NUMBER;
  FUNCTION netbytes RETURN NUMBER IS
    v NUMBER;
  BEGIN
    SELECT m.value INTO v FROM v$mystat m JOIN v$statname s ON m.statistic# = s.statistic#
     WHERE s.name = 'bytes received via SQL*Net from dblink';
    RETURN v;
  END;
BEGIN
  -- A：内层 SELECT *，外层只要 3 列（真消费）
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02) + LENGTH(r.row_id);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('A 内层 SELECT * / 外层 3 列  行=' || n || ' 收字节=' || (e - b) ||
                       ' 列值总长=' || L || ' 厘秒=' || (t1 - t0));

  -- B：投影写进内层
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT a.row_id, a.c01, a.c02 FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02) + LENGTH(r.row_id);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('B 内层已投影 / 外层 3 列    行=' || n || ' 收字节=' || (e - b) ||
                       ' 列值总长=' || L || ' 厘秒=' || (t1 - t0));

  -- 对照：真的把 70 列全取回来，证明字节计数器对列宽敏感
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT * FROM t_wide_probe@fa a
             WHERE a.d_aststat = TRUNC(SYSDATE - 1)) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c10) + LENGTH(r.c30) + LENGTH(r.c50) + LENGTH(r.c68);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('对照 取全 70 列            行=' || n || ' 收字节=' || (e - b) ||
                       ' 厘秒=' || (t1 - t0));

  -- 混合：远程 + 本地 join，内层 SELECT *
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02, l.tag
              FROM (SELECT * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t
              JOIN t_local_dim l ON l.row_id = t.row_id) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02) + LENGTH(r.tag);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('混合 A 远程+本地join       行=' || n || ' 收字节=' || (e - b) ||
                       ' 厘秒=' || (t1 - t0));

  -- 混合 + NO_MERGE：内层不许合并，最坏情况
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02, l.tag
              FROM (SELECT /*+ NO_MERGE */ * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t
              JOIN t_local_dim l ON l.row_id = t.row_id) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02) + LENGTH(r.tag);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('混合 A + NO_MERGE          行=' || n || ' 收字节=' || (e - b) ||
                       ' 厘秒=' || (t1 - t0));
END;
/

PROMPT
PROMPT ================================================================
PROMPT 5. 绑定变量在混合场景下发到远端长什么样（看 Remote SQL）
PROMPT ================================================================
VARIABLE biz_date VARCHAR2(10)
BEGIN :biz_date := TO_CHAR(TRUNC(SYSDATE - 1), 'YYYY-MM-DD'); END;
/
EXPLAIN PLAN SET STATEMENT_ID = 'MC' FOR
SELECT t.row_id, t.c01, l.tag
FROM (
    SELECT * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TO_DATE(:biz_date, 'YYYY-MM-DD')
) t
JOIN t_local_dim l ON l.row_id = t.row_id;
SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'MC', 'ALL'));

PROMPT -- 实跑一遍，然后从 v$sql 里把真正发到远端的 SQL 捞出来
SELECT COUNT(*) AS 混合绑定变量命中
FROM (SELECT * FROM t_wide_probe@fa a WHERE a.d_aststat = TO_DATE(:biz_date,'YYYY-MM-DD')) t
JOIN t_local_dim l ON l.row_id = t.row_id;

EXIT
