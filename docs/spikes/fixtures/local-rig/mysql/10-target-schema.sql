-- 目标端等价表 —— 与 t_types_probe 一一对应，用来验证「Oracle 读成字符串 → 原样写 MySQL」
-- 这条链路是否真的一字不差（ADR-0003）。
-- 全部 utf8mb4，与 CONTEXT.md 的目标端口径一致。
-- 不写死 COLLATE：`utf8mb4_0900_ai_ci` 是 8.0 才有的，5.7 上这条语句直接建不起来，
-- 而这一份种子要同时喂 8.0 与 5.7 两个台架（#262）。省掉 COLLATE 就取该版本
-- utf8mb4 的默认字符序——8.0 仍是 utf8mb4_0900_ai_ci，5.7 是 utf8mb4_general_ci，
-- 正好是各自 `collation-server` 那一档，两边都对。
ALTER DATABASE qbs CHARACTER SET utf8mb4;
USE qbs;

CREATE TABLE t_types_probe (
  row_id     INT           NOT NULL,
  kind       VARCHAR(40)   NOT NULL,
  -- DECIMAL(65,30) 是 MySQL 的上限，能装下 Oracle NUMBER 的 38 位有效数字。
  -- 标度取小于源值会静默舍入 —— ADR-0003 明确要让校验抓到它，所以这里故意给足。
  n_bare     DECIMAL(65,20),
  n_int38    DECIMAL(38,0),
  n_scale10  DECIMAL(38,10),
  n_money    DECIMAL(18,2),
  n_neg      DECIMAL(65,20),
  d_date     DATETIME,
  ts_frac    DATETIME(6),
  v_ascii    VARCHAR(100),
  v_cn       VARCHAR(400),
  nv_cn      VARCHAR(200),
  c_pad      CHAR(10),
  nc_pad     CHAR(10),
  r_raw      VARBINARY(64),
  cl_text    LONGTEXT,
  ncl_text   LONGTEXT,
  bl_bin     LONGBLOB,
  bf_float   FLOAT,
  bd_double  DOUBLE,
  PRIMARY KEY (row_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 注意 CHAR(10)：MySQL 检索 CHAR 时**默认剥掉尾部空格**，
-- 而 ADR-0003 要求 CHAR 保留尾空格。
-- #13 已实测坐实（spike 0001 §7.8 组 1）：CHAR 剥空格、CAST AS BINARY 救不回、
-- PAD_CHAR_TO_FULL_LENGTH 反而凭空补空格；VARCHAR 逐字节原样。
-- 结论：目标端建表**不得出现 CHAR**，CHAR(n)/NCHAR(n) 一律映射到 VARCHAR(n)。
-- 本表的 c_pad / nc_pad 保留 CHAR 是**故意的反例列**，不是推荐写法。
CREATE TABLE t_char_pad_probe (
  row_id      INT NOT NULL,
  as_char     CHAR(10),
  as_varchar  VARCHAR(10),
  PRIMARY KEY (row_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE t_bulk_probe (
  row_id    INT NOT NULL,
  n_amount  DECIMAL(18,2),
  v_text    VARCHAR(200),
  d_biz     DATETIME,
  PRIMARY KEY (row_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
