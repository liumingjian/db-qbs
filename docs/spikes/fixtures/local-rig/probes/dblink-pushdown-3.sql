-- #6 探针第三轮 —— 第二轮的字节数被「所有行同值」扭曲了：
-- SQL*Net 对重复列值有去重，5000 行 × 124 字符只收到 59660 字节。
-- 这里把宽表灌成随机值，重测「不下推要多花多少网络」。
SET ECHO OFF
SET FEEDBACK OFF
SET LINESIZE 200
SET PAGESIZE 100
SET SERVEROUTPUT ON SIZE UNLIMITED
WHENEVER SQLERROR CONTINUE

PROMPT ==> 把 70 列灌成随机值（消除 SQL*Net 的重复值去重）
DECLARE
  stmt VARCHAR2(32767) := 'UPDATE t_wide_probe SET ';
BEGIN
  FOR i IN 1 .. 68 LOOP
    stmt := stmt || CASE WHEN i > 1 THEN ', ' END ||
            'c' || LPAD(i, 2, '0') || ' = DBMS_RANDOM.STRING(''X'', 60)';
  END LOOP;
  EXECUTE IMMEDIATE stmt;
  COMMIT;
  DBMS_STATS.GATHER_TABLE_STATS(USER, 'T_WIDE_PROBE');
END;
/

DECLARE
  b NUMBER; e NUMBER; n NUMBER; L NUMBER; t0 NUMBER; t1 NUMBER;
  FUNCTION netbytes RETURN NUMBER IS
    v NUMBER;
  BEGIN
    SELECT m.value INTO v FROM v$mystat m JOIN v$statname s ON m.statistic# = s.statistic#
     WHERE s.name = 'bytes received via SQL*Net from dblink';
    RETURN v;
  END;
  PROCEDURE say(tag VARCHAR2, n NUMBER, bytes NUMBER, cs NUMBER) IS
  BEGIN
    DBMS_OUTPUT.PUT_LINE(RPAD(tag, 30) || ' 行=' || n ||
      ' 收字节=' || bytes || ' (' || ROUND(bytes / n, 1) || ' B/行)' ||
      ' 厘秒=' || cs);
  END;
BEGIN
  b := netbytes; n := 0; L := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  say('A 内层 SELECT */外层3列', n, e - b, t1 - t0);

  b := netbytes; n := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02
              FROM (SELECT a.row_id, a.c01, a.c02 FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c02);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  say('B 内层已投影/外层3列', n, e - b, t1 - t0);

  b := netbytes; n := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT t.row_id, t.c01, t.c02, l.tag
              FROM (SELECT * FROM t_wide_probe@fa a
                     WHERE a.d_aststat = TRUNC(SYSDATE - 1)) t
              JOIN t_local_dim l ON l.row_id = t.row_id) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.tag);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  say('混合 远程+本地join', n, e - b, t1 - t0);

  b := netbytes; n := 0; t0 := DBMS_UTILITY.GET_TIME;
  FOR r IN (SELECT * FROM t_wide_probe@fa a
             WHERE a.d_aststat = TRUNC(SYSDATE - 1)) LOOP
    n := n + 1; L := L + LENGTH(r.c01) + LENGTH(r.c68);
  END LOOP;
  t1 := DBMS_UTILITY.GET_TIME; e := netbytes;
  say('对照 取全 70 列', n, e - b, t1 - t0);
END;
/
EXIT
