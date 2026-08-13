-- 台架初始化 step 1 —— 以特权用户执行（gvenzl 入口脚本按文件名顺序跑）。
-- 只发 spike schema 后面几步真正要用到的权限，不给 DBA。
SET ECHO ON
SET FEEDBACK ON

GRANT CREATE SESSION, CREATE TABLE, CREATE VIEW, CREATE DATABASE LINK TO spike;
ALTER USER spike QUOTA UNLIMITED ON users;

-- #6 要看 dblink 场景的执行计划，需要能读 PLAN_TABLE 与游标视图。
GRANT SELECT ON v_$sql            TO spike;
GRANT SELECT ON v_$sql_plan       TO spike;
GRANT SELECT ON v_$session        TO spike;
GRANT SELECT ON v_$sql_plan_statistics_all TO spike;
GRANT SELECT_CATALOG_ROLE         TO spike;

EXIT
