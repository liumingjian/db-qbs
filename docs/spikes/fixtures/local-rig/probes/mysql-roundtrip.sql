-- #13 目标端往返实测：规范形式字符串 → MySQL 列 → 读回，逐字节比对。
-- 不需要 Oracle 参与：规范形式是**已知的输入字符串**，本探针只问「MySQL 还给不给回同一串字节」。
-- 全程用 HEX() 比对，不用字符串相等 —— 排序规则、PAD 语义都不参与判定。
-- 可重复执行；自建自删，不进主干。
USE qbs;
-- 连接字符集必须显式钉死：mysql 客户端在容器里默认可能落到 latin1，
-- 那会让中文以双重编码进库，整组结论作废。
SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci;
SET SESSION sql_mode = 'STRICT_ALL_TABLES';
SELECT @@version AS mysql_version, @@character_set_server AS cs,
       @@collation_server AS coll, @@character_set_connection AS conn_cs,
       @@collation_connection AS conn_coll, @@sql_mode AS sql_mode;

-- ============================================================
-- 组 1：CHAR vs VARCHAR —— 尾部空格保不保得住（§7.4 第 5 条取证）
-- ============================================================
DROP TABLE IF EXISTS rt_char;
CREATE TABLE rt_char (
  row_id     INT NOT NULL PRIMARY KEY,
  kind       VARCHAR(40) NOT NULL,
  src        VARCHAR(64) NOT NULL,   -- 规范形式的权威字节
  as_char    CHAR(10),
  as_varchar VARCHAR(10)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO rt_char (row_id, kind, src, as_char, as_varchar) VALUES
  (1, 'AB + 8 空格',     'AB        ', 'AB        ', 'AB        '),
  (2, '甲乙 + 8 空格',   '甲乙        ', '甲乙        ', '甲乙        '),
  (3, '10 个空格',       '          ', '          ', '          '),
  (4, '空串',            '',           '',           ''),
  (5, '无尾空格 ABCD',   'ABCD',       'ABCD',       'ABCD');

SELECT '=== 组 1：CHAR vs VARCHAR 尾空格往返 ===' AS `-`;
SELECT row_id, kind,
       HEX(src)                AS src_hex,
       HEX(as_char)            AS char_hex,
       HEX(as_varchar)         AS varchar_hex,
       IF(HEX(as_char)    = HEX(src), 'PASS', 'FAIL') AS char_verdict,
       IF(HEX(as_varchar) = HEX(src), 'PASS', 'FAIL') AS varchar_verdict
  FROM rt_char ORDER BY row_id;

SELECT '--- 长度对照（CHAR_LENGTH / OCTET_LENGTH）---' AS `-`;
SELECT row_id, kind,
       CHAR_LENGTH(src) AS src_chars, OCTET_LENGTH(src) AS src_bytes,
       CHAR_LENGTH(as_char) AS char_chars, OCTET_LENGTH(as_char) AS char_bytes,
       CHAR_LENGTH(as_varchar) AS vc_chars, OCTET_LENGTH(as_varchar) AS vc_bytes
  FROM rt_char ORDER BY row_id;

SELECT '--- CAST(CHAR 列 AS BINARY) 能不能救回填充 ---' AS `-`;
SELECT row_id, HEX(CAST(as_char AS BINARY)) AS char_as_binary_hex FROM rt_char ORDER BY row_id;

SELECT '--- PAD_CHAR_TO_FULL_LENGTH 下 CHAR 的读回（注意它补到定长，不是还原原值）---' AS `-`;
SET SESSION sql_mode = 'STRICT_ALL_TABLES,PAD_CHAR_TO_FULL_LENGTH';
SELECT row_id, kind, HEX(src) AS src_hex, HEX(as_char) AS char_hex_padded,
       IF(HEX(as_char) = HEX(src), 'PASS', 'FAIL') AS char_verdict
  FROM rt_char ORDER BY row_id;
SET SESSION sql_mode = 'STRICT_ALL_TABLES';

SELECT '--- NO PAD 排序规则：尾空格参与比较（§7.3 的前提）---' AS `-`;
SELECT 'AB' = 'AB        ' COLLATE utf8mb4_0900_ai_ci AS nopad_eq_should_be_0,
       'AB' = 'AB        ' COLLATE utf8mb4_general_ci AS pad_eq_general_ci;

-- ============================================================
-- 组 2：DECIMAL 标度 —— 规范形式会不会被标度改回去（§7.4 第 1 条取证）
-- ============================================================
DROP TABLE IF EXISTS rt_dec;
CREATE TABLE rt_dec (
  row_id INT NOT NULL PRIMARY KEY,
  kind   VARCHAR(40) NOT NULL,
  src    VARCHAR(64) NOT NULL,   -- 规范形式
  d_s0   DECIMAL(38,0),
  d_s2   DECIMAL(38,2),
  d_s6   DECIMAL(38,6),
  d_s20  DECIMAL(65,20)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 每个值都写进四种标度：有的等于源标度，有的更大，有的更小。
-- 更小的那一档在严格模式下可能报错/告警 —— 那本身就是结论。
INSERT INTO rt_dec VALUES (1, '1.23',    '1.23',    1.23,  1.23,  1.23,  1.23);
INSERT INTO rt_dec VALUES (2, '0.5',     '0.5',     0.5,   0.5,   0.5,   0.5);
INSERT INTO rt_dec VALUES (3, '-0.01',   '-0.01',  -0.01, -0.01, -0.01, -0.01);
INSERT INTO rt_dec VALUES (4, '0',       '0',       0,     0,     0,     0);
INSERT INTO rt_dec (row_id, kind, src, d_s0) VALUES
  (5, '38 位整数', '12345678901234567890123456789012345678',
      12345678901234567890123456789012345678);

SELECT '=== 组 2：DECIMAL 标度往返 ===' AS `-`;
SELECT row_id, kind, src,
       CAST(d_s0  AS CHAR) AS back_s0,
       CAST(d_s2  AS CHAR) AS back_s2,
       CAST(d_s6  AS CHAR) AS back_s6,
       CAST(d_s20 AS CHAR) AS back_s20
  FROM rt_dec ORDER BY row_id;

SELECT '--- 逐字节判定（HEX 比对，PASS 表示读回即规范形式）---' AS `-`;
SELECT row_id, kind,
       IF(HEX(CAST(d_s0  AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_s0,
       IF(HEX(CAST(d_s2  AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_s2,
       IF(HEX(CAST(d_s6  AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_s6,
       IF(HEX(CAST(d_s20 AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_s20
  FROM rt_dec ORDER BY row_id;

SELECT '--- 38 位整数写进标度更大的列会怎样（预期：超出 (38,2) 值域）---' AS `-`;
UPDATE rt_dec SET d_s2 = 12345678901234567890123456789012345678 WHERE row_id = 5;

-- ============================================================
-- 组 3：DATETIME / DATETIME(6)（§7.4 第 3、4 条相关）
-- ============================================================
DROP TABLE IF EXISTS rt_dt;
CREATE TABLE rt_dt (
  row_id INT NOT NULL PRIMARY KEY,
  kind   VARCHAR(40) NOT NULL,
  src    VARCHAR(40) NOT NULL,
  d0     DATETIME,
  d6     DATETIME(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO rt_dt VALUES
  (1, 'DATE 规范形式（无小数）', '2026-08-13 14:35:09',
      '2026-08-13 14:35:09', '2026-08-13 14:35:09'),
  (2, 'TIMESTAMP 规范形式（6 位）', '2026-08-13 14:35:09.120000',
      '2026-08-13 14:35:09.120000', '2026-08-13 14:35:09.120000');

SELECT '=== 组 3：DATETIME 往返 ===' AS `-`;
SELECT row_id, kind, src,
       CAST(d0 AS CHAR) AS back_d0,
       CAST(d6 AS CHAR) AS back_d6,
       IF(HEX(CAST(d0 AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_d0,
       IF(HEX(CAST(d6 AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS v_d6
  FROM rt_dt ORDER BY row_id;

-- ============================================================
-- 组 4：NULL 与空串（ADR-0003 的 NULL 专用标记不得与空串碰撞）
-- ============================================================
DROP TABLE IF EXISTS rt_null;
CREATE TABLE rt_null (
  row_id INT NOT NULL PRIMARY KEY,
  kind   VARCHAR(40) NOT NULL,
  v      VARCHAR(20),
  c      CHAR(10)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO rt_null VALUES
  (1, 'SQL NULL',   NULL, NULL),
  (2, '空串',       '',   ''),
  (3, '10 个空格',  '          ', '          '),
  (4, '字面量 NULL 三字母', 'NULL', 'NULL');

SELECT '=== 组 4：NULL vs 空串 vs 全空格 ===' AS `-`;
SELECT row_id, kind,
       (v IS NULL)        AS v_is_null,
       (v = '')           AS v_eq_empty,
       HEX(v)             AS v_hex,
       CHAR_LENGTH(v)     AS v_chars,
       (c IS NULL)        AS c_is_null,
       HEX(c)             AS c_hex,
       CHAR_LENGTH(c)     AS c_chars
  FROM rt_null ORDER BY row_id;

SELECT '--- 四个值在 VARCHAR 上两两可区分吗（DISTINCT 计数，NULL 单算）---' AS `-`;
SELECT COUNT(*) AS rows_total,
       COUNT(v) AS non_null_rows,
       COUNT(DISTINCT v) AS distinct_non_null,
       COUNT(DISTINCT BINARY v) AS distinct_binary
  FROM rt_null;

SELECT '--- PAD_CHAR_TO_FULL_LENGTH 下 CHAR 上空串与全空格是否碰撞 ---' AS `-`;
SET SESSION sql_mode = 'STRICT_ALL_TABLES,PAD_CHAR_TO_FULL_LENGTH';
SELECT row_id, kind, HEX(c) AS c_hex_padded FROM rt_null ORDER BY row_id;
SET SESSION sql_mode = 'STRICT_ALL_TABLES';

SELECT '== 探针跑完 ==' AS `-`;
