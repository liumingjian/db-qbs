-- M0-1 / #2 步骤 2：实际出现的类型集合（#3 的覆盖清单）
-- 用法：NLS_LANG=AMERICAN_AMERICA.AL32UTF8 sqlplus -L user/pass@src @03-type-census.sql
-- 产出：把输出贴回 issue #2，并写进 docs/spikes/0001-oracle-driver.md 第 1 节。

SET LINESIZE 200
SET PAGESIZE 100
SET FEEDBACK OFF
WHENEVER OSERROR EXIT 1
WHENEVER SQLERROR EXIT SQL.SQLCODE

PROMPT ===== 类型 x 精度组合去重（每一行都是 #3 必须覆盖的一个用例）=====
SELECT data_type,
       data_precision,
       data_scale,
       COUNT(*)                       AS col_count,
       MIN(column_name)               AS sample_col,
       LISTAGG(column_name, ' ') WITHIN GROUP (ORDER BY column_id) AS columns
  FROM all_tab_columns@FA
 WHERE owner = 'HTBR45'
   AND table_name = 'T_R_FR_ASTSTAT'
 GROUP BY data_type, data_precision, data_scale
 ORDER BY data_type, data_precision, data_scale;

PROMPT
PROMPT ===== ADR-0003 白名单外类型点名检查（出现即回炉 #11）=====
-- 直接取 ADR-0003 白名单的反集，避免新增或罕见类型因未被点名而漏报。
-- 普通 TIMESTAMP(n) 在白名单内；带时区的 TIMESTAMP 变体不匹配 TIMESTAMP(%)。
SELECT column_name, data_type
  FROM all_tab_columns@FA
 WHERE owner = 'HTBR45'
   AND table_name = 'T_R_FR_ASTSTAT'
   AND NOT (data_type IN ('NUMBER','DATE','VARCHAR2','NVARCHAR2','CHAR','NCHAR')
            OR data_type = 'TIMESTAMP'
            OR data_type LIKE 'TIMESTAMP(%)')
 ORDER BY column_id;

PROMPT
PROMPT ===== NUMBER 无精度声明的列（NUMBER 而非 NUMBER(p,s)，值域最宽、最危险）=====
SELECT column_name
  FROM all_tab_columns@FA
 WHERE owner = 'HTBR45'
   AND table_name = 'T_R_FR_ASTSTAT'
   AND data_type = 'NUMBER'
   AND data_precision IS NULL
 ORDER BY column_id;

EXIT
