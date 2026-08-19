WHENEVER SQLERROR EXIT SQL.SQLCODE
CONNECT spike/spike123@//oracle:1521/XE
SET ECHO ON
SET FEEDBACK ON

DECLARE
  TYPE table_name_list IS TABLE OF VARCHAR2(30);
  table_names table_name_list := table_name_list(
    'T_M3_B1', 'T_M3_B2', 'T_M3_B3', 'T_M3_B4', 'T_M3_B5', 'T_M3_B6'
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

CREATE TABLE t_m3_b1 (
  row_id       NUMBER(8),
  n_regular    NUMBER(38,2),
  n_fraction   NUMBER(4,6),
  n_negative   NUMBER(8,-2),
  n_bare       NUMBER,
  v_text       VARCHAR2(10 CHAR),
  nv_text      NVARCHAR2(10),
  c_text       CHAR(10 CHAR),
  nc_text      NCHAR(10),
  d_value      DATE,
  ts0          TIMESTAMP(0),
  ts3          TIMESTAMP(3),
  ts6          TIMESTAMP(6),
  load_date    DATE
);

INSERT ALL
  INTO t_m3_b1 VALUES (
    1, NULL, 0.000001, 9999999900, 0, 'ABCD', N'甲乙        ', 'AB', N'甲乙',
    TO_DATE('0001-01-01 00:00:00', 'YYYY-MM-DD HH24:MI:SS'),
    TIMESTAMP '2026-08-13 14:35:09',
    TIMESTAMP '2026-08-13 14:35:09.120',
    TIMESTAMP '2026-08-13 14:35:09.120000',
    DATE '2026-08-14'
  )
  INTO t_m3_b1 VALUES (
    2, 1.23, NULL, -9999999900, 1.2345, 'AB        ', N'ABCD', RPAD(' ', 10, ' '), N'          ',
    TO_DATE('0044-01-01 00:00:00', 'YYYY-MM-DD HH24:MI:SS'),
    TIMESTAMP '2026-08-13 14:35:09',
    TIMESTAMP '2026-08-13 14:35:09.120',
    TIMESTAMP '2026-08-13 14:35:09.999999',
    DATE '2026-08-14'
  )
  INTO t_m3_b1 VALUES (
    3, -0.01, 0.009999, NULL, -99999999999999.9999, RPAD(' ', 10, ' '), N'甲乙        ', '甲乙', N'AB',
    TO_DATE('0999-12-31 23:59:59', 'YYYY-MM-DD HH24:MI:SS'),
    TIMESTAMP '2026-08-13 14:35:09',
    TIMESTAMP '2026-08-13 14:35:09.120',
    TIMESTAMP '2026-08-13 14:35:09.120000',
    DATE '2026-08-14'
  )
  INTO t_m3_b1 VALUES (
    4, 123456789012345678901234567890123456.78, -0.009999, 12300, NULL, '甲乙        ', N'ABCD', NULL, N'甲乙',
    TO_DATE('9999-12-31 23:59:59', 'YYYY-MM-DD HH24:MI:SS'),
    TIMESTAMP '2026-08-13 14:35:09',
    TIMESTAMP '2026-08-13 14:35:09.120',
    TIMESTAMP '2026-08-13 00:00:00.000000',
    DATE '2026-08-14'
  )
  INTO t_m3_b1 VALUES (
    5, 0, 0, 12400, 0, NULL, N'          ', RPAD(' ', 10, ' '), N'          ',
    TO_DATE('2026-08-13 14:35:09', 'YYYY-MM-DD HH24:MI:SS'),
    NULL,
    NULL,
    NULL,
    DATE '2026-08-14'
  )
  INTO t_m3_b1 VALUES (
    6, 0, -0.000001, 0, 1.2345, 'ABCD', NULL, 'AB', NULL, NULL,
    TIMESTAMP '2026-08-13 14:35:09',
    TIMESTAMP '2026-08-13 14:35:09.120',
    TIMESTAMP '2026-08-13 14:35:09.120000',
    DATE '2026-08-14'
  )
SELECT 1 FROM dual;

-- row_id 是本轮新加的：主键必选（所有者 2026-08-18 裁定），B2/B3 原本一列可做主键的都没有。
CREATE TABLE t_m3_b2 (
  row_id       NUMBER(8),
  bf           BINARY_FLOAT,
  bd           BINARY_DOUBLE,
  payload      CLOB,
  v_text       VARCHAR2(10),
  c_char       CHAR(10),
  n_too_wide   NUMBER(38,-30),
  n_too_scale  NUMBER(4,35),
  n_missing    NUMBER(8,0),
  d_wrong      DATE,
  load_date    DATE
);

INSERT INTO t_m3_b2
  (row_id, bf, bd, payload, v_text, c_char, n_too_wide, n_too_scale, n_missing, d_wrong, load_date)
VALUES
  (1, 1.5, 2.5, TO_CLOB('B2 payload'), 'B2', 'B2', NULL, NULL, 7,
   DATE '2026-08-13', DATE '2026-08-14');

CREATE TABLE t_m3_b3 (
  row_id         NUMBER(8),
  ts_too_precise TIMESTAMP(9),
  load_date      DATE
);

INSERT INTO t_m3_b3 VALUES (
  1, TIMESTAMP '2026-08-13 14:35:09.123456789', DATE '2026-08-14'
);

CREATE TABLE t_m3_b4 (
  row_id    NUMBER(8),
  n_bare    NUMBER,
  load_date DATE
);

INSERT ALL
  INTO t_m3_b4 VALUES (1, 1.2345, DATE '2026-08-14')
  INTO t_m3_b4 VALUES (2, 1.23456, DATE '2026-08-14')
  INTO t_m3_b4 VALUES (3, -99999999999999.9999, DATE '2026-08-14')
  INTO t_m3_b4 VALUES (4, 0, DATE '2026-08-14')
  INTO t_m3_b4 VALUES (5, NULL, DATE '2026-08-14')
SELECT 1 FROM dual;

CREATE TABLE t_m3_b5 (
  row_id     NUMBER(8),
  n_regular  NUMBER(8,2),
  v_text     VARCHAR2(10),
  d_value    DATE,
  ts_value   TIMESTAMP(3),
  load_date  DATE
);

INSERT INTO t_m3_b5 VALUES (
  1, 12.34, 'B5', DATE '2026-08-13', TIMESTAMP '2026-08-13 14:35:09.120', DATE '2026-08-14'
);

CREATE TABLE t_m3_b6 (
  row_id    NUMBER(8),
  d_bc      DATE,
  load_date DATE
);

INSERT INTO t_m3_b6 VALUES (
  1,
  TO_DATE('-0044-01-01 00:00:00', 'SYYYY-MM-DD HH24:MI:SS'),
  DATE '2026-08-14'
);

COMMIT;

DECLARE
  rows_b1 NUMBER;
  rows_b2 NUMBER;
  rows_b3 NUMBER;
  rows_b4 NUMBER;
  rows_b5 NUMBER;
  rows_b6 NUMBER;
BEGIN
  SELECT COUNT(*) INTO rows_b1 FROM t_m3_b1;
  SELECT COUNT(*) INTO rows_b2 FROM t_m3_b2;
  SELECT COUNT(*) INTO rows_b3 FROM t_m3_b3;
  SELECT COUNT(*) INTO rows_b4 FROM t_m3_b4;
  SELECT COUNT(*) INTO rows_b5 FROM t_m3_b5;
  SELECT COUNT(*) INTO rows_b6 FROM t_m3_b6;
  IF rows_b1 != 6 OR rows_b2 != 1 OR rows_b3 != 1 OR rows_b4 != 5 OR rows_b5 != 1 OR rows_b6 != 1 THEN
    RAISE_APPLICATION_ERROR(-20001, 'M3 acceptance fixtures have unexpected row counts');
  END IF;
END;
/

EXIT
