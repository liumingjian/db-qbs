-- 目标端等价表 —— 与 t_types_probe 一一对应，用来验证「Oracle 读成字符串 → 原样写 MySQL」
-- 这条链路是否真的一字不差（ADR-0003）。
-- 全部 utf8mb4，与 CONTEXT.md 的目标端口径一致。
ALTER DATABASE qbs CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
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
-- 而 ADR-0003 要求 CHAR 保留尾空格。#3 要专门验这一条，
-- 结论可能是目标端必须用 VARCHAR/BINARY 而不是 CHAR。
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
