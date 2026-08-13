-- 台架初始化 step 3 —— 边界值。
-- 每一行都对应 ADR-0003 规范形式表里的一个具体断言；期望值写进 t_canon_expected。
CONNECT spike/spike123@//localhost:1521/XE
SET ECHO ON
SET FEEDBACK ON
SET DEFINE OFF
ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,';

-- 1) 零与尾零 —— ADR-0003「去尾零 / 零一律为 0」
INSERT INTO t_types_probe (row_id, kind, n_bare, n_int38, n_scale10, n_money, n_neg)
VALUES (1, 'zero_and_trailing_zero', 0.000, 0, 1.2300000000, 100.00, -0);

-- 2) 满精度正数 —— 38 位有效数字，f64 与 rust_decimal 都装不下
INSERT INTO t_types_probe (row_id, kind, n_bare, n_int38)
VALUES (2, 'max_precision_positive',
        12345678901234567890123456789012345678,
        12345678901234567890123456789012345678);

-- 3) 满精度负数 —— 负号前置
INSERT INTO t_types_probe (row_id, kind, n_bare, n_int38, n_neg)
VALUES (3, 'max_precision_negative',
        -12345678901234567890123456789012345678,
        -12345678901234567890123456789012345678,
        -0.0100);

-- 4) 高标度 + 小于 1 的小数 + 日期时间
--    n_scale10 用满 38 位（28 位整数 + 10 位小数）
INSERT INTO t_types_probe (row_id, kind, n_scale10, n_bare, d_date, ts_frac)
VALUES (4, 'high_scale_and_datetime',
        1234567890123456789012345678.0123456789,
        0.5,
        TO_DATE('2026-08-13 14:35:09', 'YYYY-MM-DD HH24:MI:SS'),
        TO_TIMESTAMP('2026-08-13 14:35:09.120000', 'YYYY-MM-DD HH24:MI:SS.FF6'));

-- 5) 全 NULL —— NULL 必须与空串可区分（见 t_canon_expected 的 note）
INSERT INTO t_types_probe (row_id, kind) VALUES (5, 'all_null');

-- 6) 多字节与定长填充
--    v_ascii 写空串：Oracle 会把它存成 NULL，这是台架要暴露的事实，不是笔误。
INSERT INTO t_types_probe (row_id, kind, v_ascii, v_cn, nv_cn, c_pad, nc_pad)
VALUES (6, 'multibyte_and_padding',
        '',
        '资产净值合计',
        N'资产净值合计',
        'AB',
        N'甲乙');

-- 7) 二进制 / LOB / 机器浮点 —— ADR-0003 没有为这几类定规范形式（见 note）
INSERT INTO t_types_probe (row_id, kind, r_raw, cl_text, ncl_text, bl_bin, bf_float, bd_double)
VALUES (7, 'binary_lob_float',
        HEXTORAW('DEADBEEF00'),
        '一段 CLOB 文本，含中文与 ASCII mixed content',
        N'一段 NCLOB 文本',
        HEXTORAW('0001020304FF'),
        -- 后缀 f / d 不能省：不带后缀会被当成 NUMBER 字面量解析，
        -- 而 1.79E+308 超出 NUMBER 的值域 → ORA-01426 numeric overflow。
        -- 这条本身就是给 #3 的发现：BINARY_DOUBLE 的值域装不进 NUMBER，
        -- 意味着 ADR-0003「一律 TO_CHAR 走字符串」对这两个类型需要单独确认往返精度。
        1.5f,
        1.7976931348623157E+308d);

-- LONG / LONG RAW（各自单表）
INSERT INTO t_long_probe    (row_id, kind, l_text) VALUES (1, 'long_text', '这是一段 LONG 文本，ODPI-C 对它支持有限');
INSERT INTO t_longraw_probe (row_id, kind, l_bin)  VALUES (1, 'long_raw',  HEXTORAW('CAFEBABE'));

-- ---- 期望规范形式 ------------------------------------------------------
INSERT ALL
  INTO t_canon_expected VALUES (1,'N_BARE',    '0',   '尾零去尽后即为零')
  INTO t_canon_expected VALUES (1,'N_INT38',   '0',   NULL)
  INTO t_canon_expected VALUES (1,'N_SCALE10', '1.23','声明标度 10，尾零必须去掉')
  INTO t_canon_expected VALUES (1,'N_MONEY',   '100', '整数不带小数点')
  INTO t_canon_expected VALUES (1,'N_NEG',     '0',   '负零归一为 0，不能是 -0')
  INTO t_canon_expected VALUES (2,'N_BARE',    '12345678901234567890123456789012345678','38 位原样')
  INTO t_canon_expected VALUES (2,'N_INT38',   '12345678901234567890123456789012345678',NULL)
  INTO t_canon_expected VALUES (3,'N_BARE',    '-12345678901234567890123456789012345678','负号前置')
  INTO t_canon_expected VALUES (3,'N_NEG',     '-0.01','负数尾零同样要去')
  INTO t_canon_expected VALUES (4,'N_SCALE10', '1234567890123456789012345678.0123456789','28+10=38 位满精度')
  INTO t_canon_expected VALUES (4,'N_BARE',    '0.5', 'ADR-0003 只说「去前导零」，未说明小数点前的 0 是否保留；此处按保留记，若 ADR 澄清为 .5 需同步改')
  INTO t_canon_expected VALUES (4,'D_DATE',    '2026-08-13 14:35:09','时分秒非零，不能被抹平')
  INTO t_canon_expected VALUES (4,'TS_FRAC',   '2026-08-13 14:35:09.120000','固定 6 位补零 —— 与 NUMBER 的去尾零相反')
  INTO t_canon_expected VALUES (5,'N_BARE',    NULL,  'NULL 走专用标记')
  INTO t_canon_expected VALUES (5,'V_ASCII',   NULL,  NULL)
  INTO t_canon_expected VALUES (5,'D_DATE',    NULL,  NULL)
  INTO t_canon_expected VALUES (6,'V_ASCII',   NULL,  'Oracle 把空串存成 NULL —— 源端「空串」不存在，ADR-0003 的 NULL/空串之分只在目标端有意义')
  INTO t_canon_expected VALUES (6,'V_CN',      '资产净值合计','本台架字符集为 AL32UTF8，测的是 UTF-8 路径，不是 GBK')
  INTO t_canon_expected VALUES (6,'NV_CN',     '资产净值合计',NULL)
  INTO t_canon_expected VALUES (6,'C_PAD',     'AB        ','CHAR(10) 尾部空格保留，共 10 字符')
  INTO t_canon_expected VALUES (7,'R_RAW',     NULL,  'ADR-0003 未定义 RAW 的规范形式 —— #3 需回报，由 #8 决定是否补 ADR')
  INTO t_canon_expected VALUES (7,'CL_TEXT',   NULL,  'ADR-0003 未定义 LOB 的规范形式')
  INTO t_canon_expected VALUES (7,'BF_FLOAT',  NULL,  'ADR-0003 未定义 BINARY_FLOAT/DOUBLE 的规范形式；二进制浮点本身就有精度问题')
SELECT * FROM dual;

-- ---- 10 万行：只用来看流式 fetch 的内存形状，不看吞吐 --------------------
INSERT INTO t_bulk_probe (row_id, n_amount, v_text, d_biz)
SELECT LEVEL,
       ROUND(LEVEL * 1.23, 2),
       '行内容-' || LEVEL || '-资产净值',
       TO_DATE('2026-08-13', 'YYYY-MM-DD') + MOD(LEVEL, 30)
  FROM dual
CONNECT BY LEVEL <= 100000;

COMMIT;

EXIT
