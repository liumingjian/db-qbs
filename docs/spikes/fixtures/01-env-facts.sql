-- #2 上线前复验步骤 0：环境事实采集
-- 目的：一次采齐只能在客户环境确认的服务端事实。
--   1) 11g 小版本与字符集。
--   2) @FA 上目标对象的类型与同义词去向。
--   3) 真正执行远端查询的库的 undo_retention。
-- 用法：sqlplus user/pass@src @01-env-facts.sql
-- 产出：把输出整段贴回 issue #2。

SET LINESIZE 200
SET PAGESIZE 100
SET FEEDBACK OFF
WHENEVER OSERROR EXIT 1
WHENEVER SQLERROR EXIT SQL.SQLCODE

PROMPT ===== 1. 本地库版本 =====
SELECT banner FROM v$version;

PROMPT ===== 2. dblink 远端库版本（真正跑查询的那一端）=====
SELECT banner FROM v$version@FA;

PROMPT ===== 3. 本地字符集 =====
SELECT parameter, value
  FROM nls_database_parameters
 WHERE parameter IN ('NLS_CHARACTERSET','NLS_NCHAR_CHARACTERSET');

PROMPT ===== 4. dblink 远端字符集 =====
SELECT parameter, value
  FROM nls_database_parameters@FA
 WHERE parameter IN ('NLS_CHARACTERSET','NLS_NCHAR_CHARACTERSET');

PROMPT ===== 5. 目标表行数量级（给 #5 定基准）=====
SELECT COUNT(*) AS total_rows FROM htbr45.t_r_fr_aststat@FA;

PROMPT ===== 6. @FA 上目标对象类型 =====
SELECT owner, object_name, object_type
  FROM all_objects@FA
 WHERE owner = 'HTBR45'
   AND object_name = 'T_R_FR_ASTSTAT'
 ORDER BY object_type;

PROMPT ===== 7. @FA 上同名同义词去向（无行即不是同义词）=====
SELECT owner, synonym_name, table_owner, table_name, db_link
  FROM all_synonyms@FA
 WHERE owner IN ('HTBR45', 'PUBLIC')
   AND synonym_name = 'T_R_FR_ASTSTAT'
 ORDER BY owner;

PROMPT ===== 8. dblink 远端 undo_retention =====
-- 若远端账号无权查询 V$PARAMETER，请 DBA 执行同一查询并把结果并入产出。
SELECT name, value
  FROM v$parameter@FA
 WHERE name = 'undo_retention';

EXIT
