WHENEVER SQLERROR EXIT SQL.SQLCODE
CONNECT spike/spike123@//oracle:1521/XE
SET ECHO ON
SET FEEDBACK ON

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE t_m1_narrow PURGE';
EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF;
END;
/
BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE t_m1_wide PURGE';
EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF;
END;
/

CREATE TABLE t_m1_narrow (
  row_id NUMBER(8),
  v_text VARCHAR2(200),
  d_biz  DATE
);

INSERT INTO t_m1_narrow (row_id, v_text, d_biz)
SELECT LEVEL, 'M1-' || LPAD(TO_CHAR(LEVEL), 8, '0'), DATE '2026-08-14'
  FROM dual
CONNECT BY LEVEL <= 100000;

DECLARE
  ddl VARCHAR2(32767) := 'CREATE TABLE t_m1_wide (row_id NUMBER(8), d_biz DATE';
  insert_sql VARCHAR2(32767) :=
    'INSERT INTO t_m1_wide SELECT LEVEL, DATE ''2026-08-14''';
BEGIN
  FOR i IN 1..68 LOOP
    ddl := ddl || ', c' || LPAD(i, 2, '0') || ' VARCHAR2(48)';
    insert_sql := insert_sql ||
      ', RPAD(TO_CHAR(LEVEL), 48, CHR(65 + MOD(' || i || ', 26)))';
  END LOOP;
  ddl := ddl || ')';
  insert_sql := insert_sql || ' FROM dual CONNECT BY LEVEL <= 100000';
  EXECUTE IMMEDIATE ddl;
  EXECUTE IMMEDIATE insert_sql;
END;
/

COMMIT;

DECLARE
  narrow_rows NUMBER;
  wide_rows NUMBER;
BEGIN
  SELECT COUNT(*) INTO narrow_rows FROM t_m1_narrow;
  SELECT COUNT(*) INTO wide_rows FROM t_m1_wide;
  IF narrow_rows != 100000 OR wide_rows != 100000 THEN
    RAISE_APPLICATION_ERROR(-20001, 'M1 acceptance fixtures must contain 100000 rows');
  END IF;
END;
/

EXIT
