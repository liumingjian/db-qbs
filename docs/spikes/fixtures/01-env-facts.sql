-- M0-1 / #2 步骤 0：环境事实采集
-- 目的：补齐 #7 遗留的两个待确认项，它们是 #3 的输入。
--   1) 11g 小版本 —— 19c Instant Client 对 11g 的认证下限是 11.2.0.4；
--      若低于此，#3 的客户端版本要往下退（18c / 12.2 / 11.2）。
--   2) 源端字符集 —— #3 的中文往返测试需要知道它。
-- 用法：sqlplus user/pass@src @01-env-facts.sql
-- 产出：把输出整段贴回 issue #2。

SET LINESIZE 200
SET PAGESIZE 100
SET FEEDBACK OFF

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
