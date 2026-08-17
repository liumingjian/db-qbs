// #104 —— NUMBER 纯小数（s > p）与负标度（s < 0）两种形态的端到端往返取证。
// 源端：Oracle 建列 → 写边界值 → 走驱动取数路径拿规范形式（db_qbs_shared::canon_number 校验）。
// 目标端：把规范形式生成 MySQL 探针 SQL 从 stdout 吐出，由 run-number-shapes-probe.sh 接力灌进 MySQL。
// 探针性质，不进主干；可重复执行。
use std::env;
use std::error::Error;

use db_qbs_shared::canon_number;
use oracle::Connection;

struct Case {
    ord: i32,
    id: &'static str,
    /// Oracle 侧的字面量；`NULL` 走空值路径。
    literal: &'static str,
    note: &'static str,
}

/// 组 1：纯小数 `NUMBER(4,6)` —— 值域 |x| < 0.01，4 位有效数字落在小数第 3~6 位。
const FRAC_CASES: &[Case] = &[
    Case { ord: 1, id: "frac_min_pos",    literal: "0.000001",  note: "最小正值" },
    Case { ord: 2, id: "frac_min_neg",    literal: "-0.000001", note: "最小负值（绝对值）" },
    Case { ord: 3, id: "frac_max_pos",    literal: "0.009999",  note: "最大正值" },
    Case { ord: 4, id: "frac_max_neg",    literal: "-0.009999", note: "最小负值" },
    Case { ord: 5, id: "frac_zero",       literal: "0",         note: "零" },
    Case { ord: 6, id: "frac_null",       literal: "NULL",      note: "空值" },
    Case { ord: 7, id: "frac_subscale",   literal: "0.00000051", note: "低于标度：源端是否自己舍" },
    Case { ord: 8, id: "frac_subscale_dn", literal: "0.00000049", note: "低于标度，向下" },
    Case { ord: 9, id: "frac_overflow",   literal: "0.01",      note: "越界：预期源端当场炸" },
];

/// 组 2：负标度 `NUMBER(8,-2)` —— 百位对齐，8 位有效数字 ⇒ 最大 10 位整数。
const NEG_CASES: &[Case] = &[
    Case { ord: 1, id: "neg_max_pos",   literal: "9999999900",  note: "最大正值（推导：8 位有效 ×100）" },
    Case { ord: 2, id: "neg_max_neg",   literal: "-9999999900", note: "最小负值" },
    Case { ord: 3, id: "neg_zero",      literal: "0",           note: "零" },
    Case { ord: 4, id: "neg_null",      literal: "NULL",        note: "空值" },
    Case { ord: 5, id: "neg_unaligned", literal: "12345",       note: "非百位对齐：源端自己舍" },
    Case { ord: 6, id: "neg_half",      literal: "12350",       note: "半值：舍入方向" },
    Case { ord: 7, id: "neg_overflow",  literal: "9999999999",  note: "越界：预期源端当场炸" },
];

struct Fetched {
    ord: i32,
    case_id: String,
    /// 驱动直接给出的字符串 —— 这就是取数路径产出的规范形式候选。
    driver_string: Option<String>,
    dump: Option<String>,
    insert_error: Option<String>,
    note: &'static str,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("TOTAL ERROR: number-shapes probe could not run: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let user = env_or("ORACLE_USER", "spike");
    let password = env_or("ORACLE_PASSWORD", "spike123");
    let dsn = env_or("ORACLE_DSN", "//oracle:1521/XE");

    println!("== #104 NUMBER 纯小数 / 负标度 端到端往返取证（源端段） ==");
    println!("DSN: {dsn}  user: {user}");

    let connection = Connection::connect(&user, &password, &dsn)?;
    print_session_facts(&connection)?;

    let frac = probe(&connection, "t_ns_frac", "NUMBER(4,6)", FRAC_CASES)?;
    let negs = probe(&connection, "t_ns_negs", "NUMBER(8,-2)", NEG_CASES)?;

    println!("\n=== 组 1：NUMBER(4,6) 纯小数，源端事实 ===");
    print_group(&frac);
    println!("\n=== 组 2：NUMBER(8,-2) 负标度，源端事实 ===");
    print_group(&negs);

    println!("\n-- <<<MYSQL-SQL-BEGIN>>>");
    emit_mysql_sql(&frac, &negs);
    println!("-- <<<MYSQL-SQL-END>>>");

    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn print_session_facts(connection: &Connection) -> oracle::Result<()> {
    let numeric_characters: String = connection.query_row_as(
        "SELECT value FROM nls_session_parameters WHERE parameter = 'NLS_NUMERIC_CHARACTERS'",
        &[],
    )?;
    let version: String =
        connection.query_row_as("SELECT banner FROM v$version WHERE ROWNUM = 1", &[])?;
    println!("Oracle: {version}");
    println!("NLS_NUMERIC_CHARACTERS: {numeric_characters:?}");
    Ok(())
}

fn probe(
    connection: &Connection,
    table: &str,
    column_type: &str,
    cases: &[Case],
) -> Result<Vec<Fetched>, Box<dyn Error>> {
    // 幂等：先拆再建。表不存在时 ORA-00942 是预期的。
    let _ = connection.execute(&format!("DROP TABLE {table}"), &[]);
    connection.execute(
        &format!("CREATE TABLE {table} (ord NUMBER(3) PRIMARY KEY, case_id VARCHAR2(40), v {column_type})"),
        &[],
    )?;

    let mut errors = Vec::new();
    for case in cases {
        let statement = format!(
            "INSERT INTO {table} (ord, case_id, v) VALUES ({}, '{}', {})",
            case.ord, case.id, case.literal
        );
        match connection.execute(&statement, &[]) {
            Ok(_) => {}
            // 越界之类的失败本身就是结论，记下来继续跑，不能中断整轮。
            Err(error) => errors.push((case.ord, error.to_string())),
        }
    }
    connection.commit()?;

    let rows = connection.query(
        &format!("SELECT ord, case_id, v, DUMP(v) FROM {table} ORDER BY ord"),
        &[],
    )?;

    let mut fetched = Vec::new();
    for row in rows {
        let row = row?;
        let ord: i32 = row.get(0)?;
        let case = cases
            .iter()
            .find(|case| case.ord == ord)
            .ok_or("fetched an ord that was never inserted")?;
        fetched.push(Fetched {
            ord,
            case_id: row.get(1)?,
            driver_string: row.get(2)?,
            dump: row.get(3)?,
            insert_error: None,
            note: case.note,
        });
    }

    for (ord, message) in errors {
        let case = cases.iter().find(|case| case.ord == ord).expect("case exists");
        fetched.push(Fetched {
            ord,
            case_id: case.id.to_string(),
            driver_string: None,
            dump: None,
            insert_error: Some(message.lines().next().unwrap_or("").to_string()),
            note: case.note,
        });
    }
    fetched.sort_by_key(|row| row.ord);

    Ok(fetched)
}

fn print_group(rows: &[Fetched]) {
    println!(
        "{:<4} {:<18} {:<26} {:<14} {:<38} {}",
        "ord", "case_id", "驱动字符串", "canon_number", "DUMP", "备注"
    );
    for row in rows {
        let driver = match (&row.driver_string, &row.insert_error) {
            (Some(value), _) => format!("{value:?}"),
            (None, Some(error)) => format!("INSERT 失败: {error}"),
            (None, None) => "NULL".to_string(),
        };
        let verdict = match &row.driver_string {
            Some(value) => match canon_number(value) {
                Ok(_) => "PASS".to_string(),
                Err(error) => format!("FAIL({error})"),
            },
            None => "-".to_string(),
        };
        println!(
            "{:<4} {:<18} {:<26} {:<14} {:<38} {}",
            row.ord,
            row.case_id,
            driver,
            verdict,
            row.dump.clone().unwrap_or_else(|| "-".to_string()),
            row.note
        );
    }
}

/// 目标端各形状：`(MySQL 表名, DECIMAL 形状, 这一列是什么)`。
const FRAC_SHAPES: &[(&str, &str, &str)] = &[
    ("ns_frac_exact",  "DECIMAL(6,6)",  "推导形状 (s,s)"),
    ("ns_frac_wide",   "DECIMAL(38,6)", "偏离：精度放大"),
    ("ns_frac_narrow", "DECIMAL(6,4)",  "偏离：标度不足"),
];

const NEG_SHAPES: &[(&str, &str, &str)] = &[
    ("ns_neg_exact",  "DECIMAL(10,0)", "推导形状 (p+|s|,0)"),
    ("ns_neg_narrow", "DECIMAL(8,0)",  "偏离：整数位不够"),
];

fn emit_mysql_sql(frac: &[Fetched], negs: &[Fetched]) {
    println!("USE qbs;");
    println!("SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci;");
    println!("SET SESSION sql_mode = 'STRICT_ALL_TABLES';");
    println!("SELECT @@version AS mysql_version, @@sql_mode AS sql_mode, @@character_set_connection AS conn_cs;");

    emit_shapes(frac, FRAC_SHAPES, "组 1：NUMBER(4,6) → 目标端各形状");
    emit_shapes(negs, NEG_SHAPES, "组 2：NUMBER(8,-2) → 目标端各形状");
}

fn emit_shapes(rows: &[Fetched], shapes: &[(&str, &str, &str)], title: &str) {
    println!("\nSELECT '=== {title} ===' AS `-`;");
    for (table, shape, purpose) in shapes {
        println!("DROP TABLE IF EXISTS {table};");
        println!(
            "CREATE TABLE {table} (ord INT PRIMARY KEY, case_id VARCHAR(40), src VARCHAR(64), val {shape}) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"
        );
        // 一行一条 INSERT：整数位不够是硬 ERROR 1264，合并成一条会把整组带走。
        for row in rows {
            let Some(value) = row.driver_string.as_deref() else {
                continue;
            };
            println!(
                "INSERT INTO {table} (ord, case_id, src, val) VALUES ({}, '{}', '{}', '{}');",
                row.ord, row.case_id, value, value
            );
        }
        println!("SELECT '--- {table} {shape}（{purpose}）---' AS `-`;");
        println!(
            "SELECT ord, case_id, src, HEX(src) AS src_hex, CAST(val AS CHAR) AS readback, \
HEX(CAST(val AS CHAR)) AS readback_hex, \
IF(HEX(CAST(val AS CHAR)) = HEX(src), 'PASS', 'FAIL') AS byte_verdict, \
IF(val = CAST(src AS DECIMAL(65,30)), 'value-eq', 'VALUE-CHANGED') AS value_verdict \
FROM {table} ORDER BY ord;"
        );
    }
}
