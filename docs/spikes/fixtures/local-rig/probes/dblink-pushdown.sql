-- #6 探针 —— dblink 场景下列投影是否下推。
-- 跑法（台架起着的前提下）：
--   ./scripts/run-dblink-probe.sh
-- 不是 initdb 脚本，不要放进 oracle/，那个目录只在建库时跑一次。
--
-- 台架限制：这里的 dblink 是 loopback（指回同一个 XE 实例），
-- 复现的是「远程优化器路径」这条链路的形状，不是真实的跨机网络。
-- 因此下面的字节数是**真实 TNS 流量**（走了本地 listener），但 RTT 不真实。

SET ECHO OFF
SET FEEDBACK OFF
SET LINESIZE 200
SET PAGESIZE 1000
SET SERVEROUTPUT ON SIZE UNLIMITED
SET TRIMSPOOL ON
WHENEVER SQLERROR CONTINUE

PROMPT ================================================================
PROMPT 0. 准备：宽表（70 列，贴近生产 t_r_fr_aststat 的形状）
PROMPT ================================================================

BEGIN EXECUTE IMMEDIATE 'DROP TABLE t_wide_probe PURGE'; EXCEPTION WHEN OTHERS THEN NULL; END;
/

DECLARE
  ddl VARCHAR2(32767) := 'CREATE TABLE t_wide_probe (row_id NUMBER(8) NOT NULL, d_aststat DATE';
  pad VARCHAR2(64) := RPAD('x', 60, 'x');
BEGIN
  FOR i IN 1 .. 68 LOOP
    ddl := ddl || ', c' || LPAD(i, 2, '0') || ' VARCHAR2(64) DEFAULT ''' || pad || '''';
  END LOOP;
  ddl := ddl || ')';
  EXECUTE IMMEDIATE ddl;
END;
/

INSERT INTO t_wide_probe (row_id, d_aststat)
SELECT LEVEL, TRUNC(SYSDATE - 1) FROM dual CONNECT BY LEVEL <= 5000;
COMMIT;

BEGIN
  DBMS_STATS.GATHER_TABLE_STATS(USER, 'T_WIDE_PROBE');
END;
/

SELECT COUNT(*) AS 宽表行数, COUNT(*) * 70 * 60 / 1024 / 1024 AS 全列约MB FROM t_wide_probe;

PROMPT
PROMPT ================================================================
PROMPT 1. 形状 A（生产原样）：内层 SELECT *，外层才投影
PROMPT ================================================================

EXPLAIN PLAN SET STATEMENT_ID = 'A' FOR
SELECT t.row_id, t.c01, t.c02
FROM (
    SELECT * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t;

SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'A', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 2. 形状 B（ADR-0004 建议）：投影写进内层子查询
PROMPT ================================================================

EXPLAIN PLAN SET STATEMENT_ID = 'B' FOR
SELECT t.row_id, t.c01, t.c02
FROM (
    SELECT a.row_id, a.c01, a.c02 FROM t_wide_probe@fa a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t;

SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'B', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 3. 实测：两种形状各自从 dblink 收了多少字节
PROMPT ================================================================

DECLARE
  b NUMBER; e NUMBER; n NUMBER; t0 NUMBER; t1 NUMBER;
  FUNCTION netbytes RETURN NUMBER IS
    v NUMBER;
  BEGIN
    SELECT m.value INTO v
      FROM v$mystat m JOIN v$statname s ON m.statistic# = s.statistic#
     WHERE s.name = 'bytes received via SQL*Net from dblink';
    RETURN v;
  END;
BEGIN
  -- 形状 A
  b := netbytes; n := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1;
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('形状 A（内层 SELECT *） 行数=' || n ||
                       ' dblink收字节=' || (e - b) || ' 耗时厘秒=' || (t1 - t0));

  -- 形状 B
  b := netbytes; n := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT a.row_id, a.c01, a.c02 FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1;
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  DBMS_OUTPUT.PUT_LINE('形状 B（内层已投影） 行数=' || n ||
                       ' dblink收字节=' || (e - b) || ' 耗时厘秒=' || (t1 - t0));
END;
/

PROMPT
PROMPT ================================================================
PROMPT 4. 绑定变量能否穿过 dblink（ADR-0004 依赖这一条）
PROMPT ================================================================

VARIABLE biz_date VARCHAR2(10)
BEGIN :biz_date := TO_CHAR(TRUNC(SYSDATE - 1), 'YYYY-MM-DD'); END;
/

SELECT COUNT(*) AS 绑定变量命中行数
FROM (
    SELECT * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TO_DATE(:biz_date, 'YYYY-MM-DD')
) t;

EXPLAIN PLAN SET STATEMENT_ID = 'C' FOR
SELECT t.row_id
FROM (
    SELECT * FROM t_wide_probe@fa a
    WHERE a.d_aststat = TO_DATE(:biz_date, 'YYYY-MM-DD')
) t;

SELECT * FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'C', 'ALL'));

PROMPT
PROMPT ================================================================
PROMPT 5. dblink 故障时的错误特征（M4 要能区分本地库问题 / dblink 问题）
PROMPT ================================================================

BEGIN EXECUTE IMMEDIATE 'DROP DATABASE LINK fa_no_listener'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
BEGIN EXECUTE IMMEDIATE 'DROP DATABASE LINK fa_bad_host';    EXCEPTION WHEN OTHERS THEN NULL; END;
/
BEGIN EXECUTE IMMEDIATE 'DROP DATABASE LINK fa_bad_cred';    EXCEPTION WHEN OTHERS THEN NULL; END;
/

CREATE DATABASE LINK fa_no_listener CONNECT TO spike IDENTIFIED BY spike123 USING 'oracle:1599/XE';
CREATE DATABASE LINK fa_bad_host    CONNECT TO spike IDENTIFIED BY spike123 USING 'no-such-host:1521/XE';
CREATE DATABASE LINK fa_bad_cred    CONNECT TO spike IDENTIFIED BY wrongpw   USING 'oracle:1521/XE';

DECLARE
  PROCEDURE probe(tag VARCHAR2, stmt VARCHAR2) IS
    n NUMBER;
  BEGIN
    EXECUTE IMMEDIATE stmt INTO n;
    DBMS_OUTPUT.PUT_LINE(tag || ' -> 没报错（意外）rows=' || n);
  EXCEPTION WHEN OTHERS THEN
    DBMS_OUTPUT.PUT_LINE('--- ' || tag || ' ---');
    DBMS_OUTPUT.PUT_LINE('SQLCODE=' || SQLCODE);
    DBMS_OUTPUT.PUT_LINE(SQLERRM);
  END;
BEGIN
  probe('监听端口不通',   'SELECT COUNT(*) FROM t_wide_probe@fa_no_listener');
  probe('主机名解析不了', 'SELECT COUNT(*) FROM t_wide_probe@fa_bad_host');
  probe('口令错',         'SELECT COUNT(*) FROM t_wide_probe@fa_bad_cred');
  probe('远端表不存在',   'SELECT COUNT(*) FROM t_no_such_table@fa');
  probe('本地表不存在（对照）', 'SELECT COUNT(*) FROM t_no_such_local_table');
END;
/

PROMPT
PROMPT ================================================================
PROMPT 6. 附：dblink 打开中的会话视图（M4 排障线索）
PROMPT ================================================================
COLUMN db_link FORMAT A20
SELECT db_link, in_transaction FROM v$dblink;

EXIT
