#!/usr/bin/env bash
# 冒烟 —— 把 #9 声称已实测的四件事，每次起台架都重跑一遍。
# 任何一条挂掉，台架就不能用来支撑 #3 / #6 的结论。
set -euo pipefail
cd "$(dirname "$0")/.."

run_sql() {
  docker compose exec -T client bash -c \
    "printf '%s\n' \"\$1\" 'EXIT' | sqlplus -S spike/spike123@//oracle:1521/XE" _ "$1"
}

echo "-- 1) 版本与字符集（19.32 客户端 → 11.2.0.2 服务端，不应有兼容性告警）"
run_sql "SET PAGESIZE 50 LINESIZE 200
SELECT banner FROM v\$version;
SELECT parameter, value FROM nls_database_parameters
 WHERE parameter IN ('NLS_CHARACTERSET','NLS_NCHAR_CHARACTERSET');"

echo "-- 2) 38 位 NUMBER 原样往返（ADR-0003 的前提）"
run_sql "SET PAGESIZE 50 LINESIZE 200
COLUMN v FORMAT A45
SELECT TO_CHAR(n_int38) AS v, LENGTH(TO_CHAR(n_int38)) AS len
  FROM t_types_probe WHERE row_id = 2;"

echo "-- 3) 等价表与期望值都在位（断言，不只是打印）"
assert_eq() {  # $1=说明 $2=期望 $3=实测
  if [[ "$3" != "$2" ]]; then
    echo "!! $1：期望 $2，实测 $3"
    exit 1
  fi
  echo "    $1 = $3 ✓"
}
counts=$(run_sql "SET PAGESIZE 0 FEEDBACK OFF HEADING OFF
SELECT (SELECT COUNT(*) FROM t_types_probe)    || ' ' ||
       (SELECT COUNT(*) FROM t_canon_expected) || ' ' ||
       (SELECT COUNT(*) FROM t_bulk_probe)     || ' ' ||
       (SELECT COUNT(*) FROM t_long_probe)     || ' ' ||
       (SELECT COUNT(*) FROM t_longraw_probe)
  FROM dual;" | tr '\t\r' '  ' | tr -s ' ' | grep -oE '[0-9]+( [0-9]+){4}')
read -r probe expected bulk lng lngraw <<<"$counts"
assert_eq "t_types_probe 行数"    7      "$probe"
assert_eq "t_canon_expected 单元格" 28     "$expected"
assert_eq "t_bulk_probe 行数"      100000 "$bulk"
assert_eq "t_long_probe 行数"      1      "$lng"
assert_eq "t_longraw_probe 行数"   1      "$lngraw"

echo "-- 4) dblink 通"
via=$(run_sql "SET PAGESIZE 0 FEEDBACK OFF HEADING OFF
SELECT COUNT(*) FROM t_types_probe@fa;" | tr -d ' \t\r' | grep -E '^[0-9]+$' | tail -1)
assert_eq "经 @fa 读到的行数" 7 "$via"

echo "-- 5) MySQL 目标端 utf8mb4 就位"
docker compose exec -T mysql mysql -uspike -pspike123 qbs -e \
  "SELECT @@character_set_server AS cs, @@collation_server AS coll; SHOW TABLES;" 2>/dev/null

echo "== 冒烟全过 =="
