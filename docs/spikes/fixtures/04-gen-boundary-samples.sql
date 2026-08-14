-- M0-1 / #2 步骤 3（第一步）：生成边界值采样脚本
-- 这是一个「写 SQL 的 SQL」——列名事先未知，所以由字典表生成采样语句。
-- 用法：
--   sqlplus -S user/pass@src @04-gen-boundary-samples.sql > 05-boundary-samples.sql
--   审一眼 05-boundary-samples.sql（尤其确认没有被 LINESIZE 截断），再：
--   sqlplus -S user/pass@src @05-boundary-samples.sql > samples-t_r_fr_aststat.csv
-- 产出：samples-t_r_fr_aststat.csv 提交到 docs/spikes/fixtures/
--
-- 采样口径（对应 ADR-0003 的规范形式表）：
--   NUMBER      → 最长字符串长度（最大精度）/ 最小值（负数）/ 最大值 / 最高标度小数 / 零的条数 / NULL 条数
--   DATE/TS     → 最早 / 最晚 / 含非零时分秒的条数 / NULL 条数
--   字符类       → 最长的一条 / 含多字节（中文）的一条 / NULL 条数
-- 每种类型由这几路共同保证 ≥3 个边界样本。
--
-- 注意：LONG / LONG RAW / LOB / XMLType 不能用聚合函数，本脚本不覆盖；
--       若 03-type-census.sql 命中了这些类型，单独在 #2 里记一笔交给 #3 专项处理。

SET PAGESIZE 0
SET LINESIZE 4000
SET LONG 200000
SET FEEDBACK OFF
SET HEADING OFF
SET TRIMSPOOL ON
SET VERIFY OFF
WHENEVER OSERROR EXIT 1
WHENEVER SQLERROR EXIT SQL.SQLCODE

PROMPT SET PAGESIZE 0
PROMPT SET LINESIZE 4000
PROMPT SET FEEDBACK OFF
PROMPT SET HEADING OFF
PROMPT SET TRIMSPOOL ON
PROMPT WHENEVER OSERROR EXIT 1
PROMPT WHENEVER SQLERROR EXIT SQL.SQLCODE
PROMPT ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,';
PROMPT PROMPT COLUMN_NAME,DATA_TYPE,KIND,VALUE,VALUE_LEN

-- ---- NUMBER 列 ----
SELECT 'SELECT ''' || column_name || ',' || data_type || ',''||kind||'',''||NVL(v,''<NULL>'')||'',''||NVL(LENGTH(v),0) FROM ('
       || ' SELECT ''max_len'' kind, TO_CHAR(MAX(LENGTH(TO_CHAR(' || column_name || ')))) v FROM ' || tbl
       || ' UNION ALL SELECT ''min_val'', TO_CHAR(MIN(' || column_name || ')) FROM ' || tbl
       || ' UNION ALL SELECT ''max_val'', TO_CHAR(MAX(' || column_name || ')) FROM ' || tbl
       || ' UNION ALL SELECT ''max_scale_val'', TO_CHAR(MAX(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name || ') THEN ' || column_name || ' END)) FROM ' || tbl
       || ' UNION ALL SELECT ''zero_cnt'', TO_CHAR(COUNT(CASE WHEN ' || column_name || ' = 0 THEN 1 END)) FROM ' || tbl
       || ' UNION ALL SELECT ''null_cnt'', TO_CHAR(COUNT(CASE WHEN ' || column_name || ' IS NULL THEN 1 END)) FROM ' || tbl
       || ' );'
  FROM all_tab_columns@FA, (SELECT 'htbr45.t_r_fr_aststat@FA' tbl FROM dual)
 WHERE owner = 'HTBR45' AND table_name = 'T_R_FR_ASTSTAT' AND data_type = 'NUMBER'
 ORDER BY column_id;

-- ---- DATE / TIMESTAMP 列 ----
SELECT 'SELECT ''' || column_name || ',' || data_type || ',''||kind||'',''||NVL(v,''<NULL>'')||'',''||NVL(LENGTH(v),0) FROM ('
       || ' SELECT ''min_val'' kind, TO_CHAR(MIN(' || column_name || '), ''YYYY-MM-DD HH24:MI:SS'') v FROM ' || tbl
       || ' UNION ALL SELECT ''max_val'', TO_CHAR(MAX(' || column_name || '), ''YYYY-MM-DD HH24:MI:SS'') FROM ' || tbl
       || ' UNION ALL SELECT ''with_time_cnt'', TO_CHAR(COUNT(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name || ') THEN 1 END)) FROM ' || tbl
       || ' UNION ALL SELECT ''null_cnt'', TO_CHAR(COUNT(CASE WHEN ' || column_name || ' IS NULL THEN 1 END)) FROM ' || tbl
       || ' );'
  FROM all_tab_columns@FA, (SELECT 'htbr45.t_r_fr_aststat@FA' tbl FROM dual)
 WHERE owner = 'HTBR45' AND table_name = 'T_R_FR_ASTSTAT'
   AND (data_type = 'DATE' OR data_type LIKE 'TIMESTAMP%')
 ORDER BY column_id;

-- ---- 字符类列 ----
SELECT 'SELECT ''' || column_name || ',' || data_type || ',''||kind||'',''||NVL(v,''<NULL>'')||'',''||NVL(LENGTH(v),0) FROM ('
       || ' SELECT ''max_len_val'' kind, MAX(' || column_name || ') KEEP (DENSE_RANK LAST ORDER BY LENGTH(' || column_name || ')) v FROM ' || tbl
       || ' UNION ALL SELECT ''multibyte_val'', MAX(CASE WHEN LENGTHB(' || column_name || ') > LENGTH(' || column_name || ') THEN ' || column_name || ' END) FROM ' || tbl
       || ' UNION ALL SELECT ''null_cnt'', TO_CHAR(COUNT(CASE WHEN ' || column_name || ' IS NULL THEN 1 END)) FROM ' || tbl
       || ' );'
  FROM all_tab_columns@FA, (SELECT 'htbr45.t_r_fr_aststat@FA' tbl FROM dual)
 WHERE owner = 'HTBR45' AND table_name = 'T_R_FR_ASTSTAT'
   AND data_type IN ('VARCHAR2','NVARCHAR2','CHAR','NCHAR')
 ORDER BY column_id;

PROMPT EXIT
EXIT
