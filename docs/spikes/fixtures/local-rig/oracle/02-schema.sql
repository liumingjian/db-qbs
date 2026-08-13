-- 台架初始化 step 2 —— 等价表：覆盖 ADR-0003 规范形式表里的每一种类型。
-- 这不是客户表的复制（真实列清单是 #2 的产出），而是「类型面」的等价物：
-- ADR-0003 列到的每一种规范形式，这里都至少有一列。
CONNECT spike/spike123@//localhost:1521/XE
SET ECHO ON
SET FEEDBACK ON

-- ---- 主探针表 ----------------------------------------------------------
-- 命名对齐 CONTEXT.md 的口径：这是一张「源表」的替身。
CREATE TABLE t_types_probe (
  row_id          NUMBER(4)      NOT NULL,   -- 行编号，与 t_canon_expected 关联
  kind            VARCHAR2(40)   NOT NULL,   -- 这一行在测什么边界
  -- NUMBER 家族：ADR-0003 的核心风险面
  n_bare          NUMBER,                    -- 无精度声明，值域最宽、最危险
  n_int38         NUMBER(38,0),              -- 满精度整数
  n_scale10       NUMBER(38,10),             -- 高标度
  n_money         NUMBER(18,2),              -- 典型金额
  n_neg           NUMBER,                    -- 负数
  -- 日期时间
  d_date          DATE,                      -- 必须带非零时分秒
  ts_frac         TIMESTAMP(6),              -- 固定 6 位小数秒
  -- 字符类
  v_ascii         VARCHAR2(100),
  v_cn            VARCHAR2(400),             -- 中文（本台架只能测 UTF-8 路径，见 README）
  nv_cn           NVARCHAR2(200),
  c_pad           CHAR(10),                  -- 尾部空格必须保留
  nc_pad          NCHAR(10),
  -- 二进制 / 大对象 / 浮点：11g 遗留类型面
  r_raw           RAW(64),
  cl_text         CLOB,
  ncl_text        NCLOB,
  bl_bin          BLOB,
  bf_float        BINARY_FLOAT,
  bd_double       BINARY_DOUBLE,
  CONSTRAINT pk_t_types_probe PRIMARY KEY (row_id)
);

-- ---- LONG / LONG RAW 各自单独一张表 ------------------------------------
-- Oracle 限制：一张表最多一个 LONG 或 LONG RAW 列，所以拆表，不是风格问题。
-- ODPI-C 对 LONG 支持有限且不能与 LOB 混取 —— #3 必须单独立用例。
CREATE TABLE t_long_probe (
  row_id  NUMBER(4) NOT NULL,
  kind    VARCHAR2(40) NOT NULL,
  l_text  LONG,
  CONSTRAINT pk_t_long_probe PRIMARY KEY (row_id)
);

CREATE TABLE t_longraw_probe (
  row_id  NUMBER(4) NOT NULL,
  kind    VARCHAR2(40) NOT NULL,
  l_bin   LONG RAW,
  CONSTRAINT pk_t_longraw_probe PRIMARY KEY (row_id)
);

-- ---- 期望规范形式表 ----------------------------------------------------
-- #3 的断言直接 join 这张表，不要把期望值硬编码在 Rust 里：
-- 期望值是 ADR-0003 的产物，改 ADR 应该只改这张表。
-- expected 为 NULL 表示「该值就是 SQL NULL」，规范形式走 ADR-0003 的 NULL 专用标记。
CREATE TABLE t_canon_expected (
  row_id       NUMBER(4)     NOT NULL,
  column_name  VARCHAR2(30)  NOT NULL,
  expected     VARCHAR2(200),
  note         VARCHAR2(200),
  CONSTRAINT pk_t_canon_expected PRIMARY KEY (row_id, column_name)
);

-- ---- 10 万行表：给 #5 看形状用（绝对数字作废，见 README）----------------
CREATE TABLE t_bulk_probe (
  row_id    NUMBER(8)      NOT NULL,
  n_amount  NUMBER(18,2),
  v_text    VARCHAR2(200),
  d_biz     DATE,
  CONSTRAINT pk_t_bulk_probe PRIMARY KEY (row_id)
);

EXIT
