-- #21 Oracle 11g pagination boundary reproducibility probe.
--
-- Four cases: local/dblink x non-total/total ordering. Each case takes page 3
-- (rows 10001..15000) as its baseline, repeats the query 10 times, then repeats
-- it after an independently committed source-table insert before the page.
-- Every comparison prints missing/added/symmetric-difference row counts.

SET ECHO OFF
SET FEEDBACK OFF
SET HEADING ON
SET LINESIZE 180
SET PAGESIZE 200
SET SERVEROUTPUT ON SIZE UNLIMITED
SET TRIMSPOOL ON
WHENEVER SQLERROR EXIT SQL.SQLCODE

BEGIN EXECUTE IMMEDIATE 'DROP TABLE t_page_probe_result PURGE'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
BEGIN EXECUTE IMMEDIATE 'DROP TABLE t_page_probe_baseline PURGE'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
BEGIN EXECUTE IMMEDIATE 'DROP TABLE t_page_probe PURGE'; EXCEPTION WHEN OTHERS THEN NULL; END;
/

CREATE TABLE t_page_probe (
  row_id   NUMBER(8) NOT NULL,
  d_biz    DATE NOT NULL,
  payload  VARCHAR2(40) NOT NULL,
  CONSTRAINT pk_t_page_probe PRIMARY KEY (row_id)
);

-- All 30000 original rows deliberately tie on d_biz.
INSERT INTO t_page_probe (row_id, d_biz, payload)
SELECT LEVEL,
       TO_DATE('2026-08-14', 'YYYY-MM-DD'),
       'row-' || TO_CHAR(LEVEL)
  FROM dual
CONNECT BY LEVEL <= 30000;
COMMIT;

BEGIN
  DBMS_STATS.GATHER_TABLE_STATS(USER, 'T_PAGE_PROBE');
END;
/

CREATE TABLE t_page_probe_baseline (
  case_name  VARCHAR2(30) NOT NULL,
  row_id     NUMBER(8) NOT NULL,
  CONSTRAINT pk_t_page_probe_baseline PRIMARY KEY (case_name, row_id)
);

CREATE TABLE t_page_probe_result (
  phase           VARCHAR2(20) NOT NULL,
  round_no        NUMBER(3) NOT NULL,
  path             VARCHAR2(10) NOT NULL,
  order_key        VARCHAR2(10) NOT NULL,
  missing_rows     NUMBER(8) NOT NULL,
  added_rows       NUMBER(8) NOT NULL,
  symmetric_diff   NUMBER(8) NOT NULL
);

DECLARE
  c_page_lo CONSTANT PLS_INTEGER := 10001;
  c_page_hi CONSTANT PLS_INTEGER := 15000;

  FUNCTION page_sql(p_suffix VARCHAR2, p_order_by VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
    RETURN 'SELECT row_id FROM (' ||
           'SELECT row_id, ROW_NUMBER() OVER (ORDER BY ' || p_order_by || ') rn ' ||
           'FROM t_page_probe' || p_suffix ||
           ') WHERE rn BETWEEN ' || c_page_lo || ' AND ' || c_page_hi;
  END;

  PROCEDURE save_baseline(
    p_case_name VARCHAR2,
    p_suffix    VARCHAR2,
    p_order_by  VARCHAR2
  ) IS
    q VARCHAR2(32767);
  BEGIN
    q := page_sql(p_suffix, p_order_by);
    EXECUTE IMMEDIATE
      'INSERT INTO t_page_probe_baseline (case_name, row_id) ' ||
      'SELECT :case_name, row_id FROM (' || q || ')'
      USING p_case_name;
  END;

  PROCEDURE measure(
    p_phase      VARCHAR2,
    p_round_no   PLS_INTEGER,
    p_path       VARCHAR2,
    p_order_key  VARCHAR2,
    p_case_name  VARCHAR2,
    p_suffix     VARCHAR2,
    p_order_by   VARCHAR2
  ) IS
    q        VARCHAR2(32767);
    missing  NUMBER;
    added    NUMBER;
  BEGIN
    q := page_sql(p_suffix, p_order_by);

    EXECUTE IMMEDIATE
      'SELECT COUNT(*) FROM (' ||
      'SELECT row_id FROM t_page_probe_baseline WHERE case_name = :case_name ' ||
      'MINUS ' || q || ')'
      INTO missing USING p_case_name;

    EXECUTE IMMEDIATE
      'SELECT COUNT(*) FROM (' || q ||
      ' MINUS SELECT row_id FROM t_page_probe_baseline WHERE case_name = :case_name)'
      INTO added USING p_case_name;

    INSERT INTO t_page_probe_result
      (phase, round_no, path, order_key, missing_rows, added_rows, symmetric_diff)
    VALUES
      (p_phase, p_round_no, p_path, p_order_key, missing, added, missing + added);
  END;

  PROCEDURE concurrent_write IS
    PRAGMA AUTONOMOUS_TRANSACTION;
  BEGIN
    INSERT INTO t_page_probe (row_id, d_biz, payload)
    VALUES (0, TO_DATE('2026-08-13', 'YYYY-MM-DD'), 'concurrent-before-page');
    COMMIT;
  END;

BEGIN
  save_baseline('LOCAL_NON_TOTAL',  '',    'd_biz');
  save_baseline('LOCAL_TOTAL',      '',    'd_biz, row_id');
  save_baseline('DBLINK_NON_TOTAL', '@fa', 'd_biz');
  save_baseline('DBLINK_TOTAL',     '@fa', 'd_biz, row_id');
  COMMIT;

  FOR i IN 1 .. 10 LOOP
    measure('STATIC_REPEAT', i, 'LOCAL',  'NON_TOTAL', 'LOCAL_NON_TOTAL',  '',    'd_biz');
    measure('STATIC_REPEAT', i, 'LOCAL',  'TOTAL',     'LOCAL_TOTAL',      '',    'd_biz, row_id');
    measure('STATIC_REPEAT', i, 'DBLINK', 'NON_TOTAL', 'DBLINK_NON_TOTAL', '@fa', 'd_biz');
    measure('STATIC_REPEAT', i, 'DBLINK', 'TOTAL',     'DBLINK_TOTAL',     '@fa', 'd_biz, row_id');
  END LOOP;
  COMMIT;

  -- A separately committed transaction writes a row before the measured page.
  -- It models the production source changing between the original read and retry.
  concurrent_write;
  measure('AFTER_WRITE', 1, 'LOCAL',  'NON_TOTAL', 'LOCAL_NON_TOTAL',  '',    'd_biz');
  measure('AFTER_WRITE', 1, 'LOCAL',  'TOTAL',     'LOCAL_TOTAL',      '',    'd_biz, row_id');
  measure('AFTER_WRITE', 1, 'DBLINK', 'NON_TOTAL', 'DBLINK_NON_TOTAL', '@fa', 'd_biz');
  measure('AFTER_WRITE', 1, 'DBLINK', 'TOTAL',     'DBLINK_TOTAL',     '@fa', 'd_biz, row_id');
  COMMIT;
END;
/

PROMPT === ENVIRONMENT ===
SELECT banner FROM v$version WHERE banner LIKE 'Oracle Database%';

PROMPT === BASELINES (each must contain 5000 rows) ===
SELECT case_name, COUNT(*) AS rows_in_baseline
  FROM t_page_probe_baseline
 GROUP BY case_name
 ORDER BY case_name;

PROMPT === ACTUAL DIFFERENCE COUNTS ===
SELECT phase, round_no, path, order_key,
       missing_rows, added_rows, symmetric_diff
  FROM t_page_probe_result
 ORDER BY CASE phase WHEN 'STATIC_REPEAT' THEN 1 ELSE 2 END,
          round_no, path, order_key;

EXIT
