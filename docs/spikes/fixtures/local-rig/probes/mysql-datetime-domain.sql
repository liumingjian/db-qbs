-- #35 MySQL DATETIME 值域实测。只需要 MySQL，不起 Oracle / client。
--
-- 问的是什么：Oracle DATE 域（公元前 4712 起）比 MySQL DATETIME 的**文档域**
-- （1000-01-01 起）宽。域外的值写进 DATETIME 到底是**静默写进去**（那就是
-- ADR-0006 那张「行数相等但值被静默改掉」表上的新一行），还是**报错**（那就落在
-- ADR-0009「失败即整 run 失败」的既有形状里，不需要新防线）。
--
-- 一律用 ADR-0015 开连接仪式写死的那串 sql_mode，不是 MySQL 默认串——
-- 默认串含 NO_ZERO_DATE / NO_ZERO_IN_DATE，仪式那串没有，行为不同。
-- 判定用 HEX() 逐字节比对，与 mysql-roundtrip.sql 同一口径。

SET NAMES utf8mb4;
SELECT VERSION() AS mysql_version, @@GLOBAL.sql_mode AS server_default_sql_mode;

SET SESSION sql_mode = 'STRICT_ALL_TABLES';
SELECT @@SESSION.sql_mode AS session_sql_mode;

DROP TABLE IF EXISTS t_dt_domain;
CREATE TABLE t_dt_domain (id INT PRIMARY KEY, src VARCHAR(32), dt DATETIME) ENGINE=InnoDB;

-- 组 A：域内与文档域以下。每条单独一句，逐条看 warning。
INSERT INTO t_dt_domain VALUES (1, '0001-01-01 00:00:00', '0001-01-01 00:00:00');
INSERT INTO t_dt_domain VALUES (2, '0999-12-31 23:59:59', '0999-12-31 23:59:59');
INSERT INTO t_dt_domain VALUES (3, '1000-01-01 00:00:00', '1000-01-01 00:00:00');
INSERT INTO t_dt_domain VALUES (4, '9999-12-31 23:59:59', '9999-12-31 23:59:59');
INSERT INTO t_dt_domain VALUES (5, '0000-01-01 00:00:00', '0000-01-01 00:00:00');
INSERT INTO t_dt_domain VALUES (6, '0000-00-00 00:00:00', '0000-00-00 00:00:00');
-- 第 7 条是本探针的要害：Oracle 的公元前 4712 年若被 canon_date 丢掉纪元，
-- 产出的就是这个字符串，而它是一个**完全合法**的公元年份。
INSERT INTO t_dt_domain VALUES (7, '4712-01-01 00:00:00', '4712-01-01 00:00:00');

-- 组 B：格式化后仍越界的形态（负年份 = 驱动给了带符号的年；五位年 = 上溢）。
INSERT INTO t_dt_domain VALUES (8,  '-4712-01-01 00:00:00', '-4712-01-01 00:00:00');
INSERT INTO t_dt_domain VALUES (9,  '-0001-01-01 00:00:00', '-0001-01-01 00:00:00');
INSERT INTO t_dt_domain VALUES (10, '10000-01-01 00:00:00', '10000-01-01 00:00:00');

-- 落库逐字节读回：src 是输入串，dt 是读回串，HEX 相等即原样往返。
SELECT id, src, dt,
       HEX(src) AS src_hex,
       HEX(CAST(dt AS CHAR)) AS dt_hex,
       (HEX(src) = HEX(CAST(dt AS CHAR))) AS roundtrip_ok
FROM t_dt_domain ORDER BY id;

-- 组 C：多值 INSERT（ADR-0015 §2 的写法）里混一个文档域以下的值，
-- 看它会不会把整条子语句连坐。
DROP TABLE IF EXISTS t_dt_multi;
CREATE TABLE t_dt_multi (id INT, dt DATETIME) ENGINE=InnoDB;
INSERT INTO t_dt_multi (id, dt) VALUES
  (1, '2026-08-13 00:00:00'), (2, '0001-01-01 00:00:00'), (3, '2026-08-14 00:00:00');
SELECT COUNT(*) AS rows_in_multi FROM t_dt_multi;

-- 组 D：非严格模式对照。证明「硬报错」这件事是 ADR-0015 那条 sql_mode 断言买来的，
-- 不是 MySQL 自带的——严格模式关掉之后，同样的输入静默变成 0000-00-00。
SET SESSION sql_mode = '';
DROP TABLE IF EXISTS t_dt_lax;
CREATE TABLE t_dt_lax (id INT, dt DATETIME) ENGINE=InnoDB;
INSERT INTO t_dt_lax VALUES (1, '-4712-01-01 00:00:00');
INSERT INTO t_dt_lax VALUES (2, '10000-01-01 00:00:00');
SELECT id, dt FROM t_dt_lax ORDER BY id;
