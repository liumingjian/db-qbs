-- M0-1 / #2 步骤 1：导出列元数据为 CSV
-- 用法：NLS_LANG=AMERICAN_AMERICA.AL32UTF8 sqlplus -L -S user/pass@src @02-columns.sql > columns-t_r_fr_aststat.csv
-- 产出：columns-t_r_fr_aststat.csv 提交到 docs/spikes/fixtures/
-- 注意：目标表经 dblink，元数据必须走 ALL_TAB_COLUMNS@FA，本地字典里没有它。

SET PAGESIZE 0
SET LINESIZE 500
SET FEEDBACK OFF
SET HEADING OFF
SET TRIMSPOOL ON
SET VERIFY OFF
WHENEVER OSERROR EXIT 1
WHENEVER SQLERROR EXIT FAILURE

-- CSV 表头
PROMPT COLUMN_ID,COLUMN_NAME,DATA_TYPE,DATA_PRECISION,DATA_SCALE,DATA_LENGTH,CHAR_LENGTH,CHAR_USED,NULLABLE,DATA_DEFAULT_PRESENT

SELECT column_id
       || ',' || column_name
       || ',' || data_type
       || ',' || NVL(TO_CHAR(data_precision), '')
       || ',' || NVL(TO_CHAR(data_scale), '')
       || ',' || TO_CHAR(data_length)
       || ',' || NVL(TO_CHAR(char_length), '')
       || ',' || NVL(char_used, '')
       || ',' || nullable
       || ',' || CASE WHEN default_length IS NULL THEN 'N' ELSE 'Y' END
  FROM all_tab_columns@FA
 WHERE owner = 'HTBR45'
   AND table_name = 'T_R_FR_ASTSTAT'
 ORDER BY column_id;

EXIT
