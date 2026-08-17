// #98 —— Oracle 驱动对公元前日期给正年还是负年，canon_date 丢不丢纪元。
// 只测源端：Oracle 建 DATE 列 → 写公元前 / 公元早期边界值 → 走**生产同款取数路径**
// （`Option<Timestamp>` + `db_qbs_shared::canon_date`）看驱动交出来的年份符号与规范形式。
// 本票只出事实，不下判定；目标端那一半 #35 已测完（probes/mysql-datetime-domain.sql）。
// 探针性质，不进主干；可重复执行。
use std::env;
use std::error::Error;

use db_qbs_shared::canon_date;
use oracle::sql_type::Timestamp;
use oracle::Connection;

struct Case {
    ord: i32,
    id: &'static str,
    /// Oracle 侧字面量；`NULL` 走空值路径。`SYYYY` 才带纪元符号。
    literal: &'static str,
    note: &'static str,
}

const CASES: &[Case] = &[
    Case { ord: 1, id: "bc_4712_min",  literal: "TO_DATE('-4712-01-01 00:00:00','SYYYY-MM-DD HH24:MI:SS')", note: "Oracle DATE 域下界（公元前 4712）" },
    Case { ord: 2, id: "bc_4712_late", literal: "TO_DATE('-4712-12-31 23:59:59','SYYYY-MM-DD HH24:MI:SS')", note: "公元前 4712 年末" },
    Case { ord: 3, id: "bc_0001",      literal: "TO_DATE('-0001-12-31 12:00:00','SYYYY-MM-DD HH24:MI:SS')", note: "公元前 1 年（纪元边界近侧）" },
    Case { ord: 4, id: "bc_0044",      literal: "TO_DATE('-0044-03-15 00:00:00','SYYYY-MM-DD HH24:MI:SS')", note: "公元前 44 年（两位数年，正负同形）" },
    Case { ord: 5, id: "ad_0001",      literal: "TO_DATE('0001-01-01 00:00:00','SYYYY-MM-DD HH24:MI:SS')",  note: "公元 1 年（纪元边界远侧）" },
    Case { ord: 6, id: "ad_0044",      literal: "TO_DATE('0044-03-15 00:00:00','SYYYY-MM-DD HH24:MI:SS')",  note: "公元 44 年（与 ord 4 同数字，验纪元可分）" },
    Case { ord: 7, id: "ad_0999",      literal: "TO_DATE('0999-12-31 23:59:59','SYYYY-MM-DD HH24:MI:SS')",  note: "公元 999（MySQL 文档域下界之前）" },
    Case { ord: 8, id: "ad_1000",      literal: "TO_DATE('1000-01-01 00:00:00','SYYYY-MM-DD HH24:MI:SS')",  note: "MySQL DATETIME 文档域下界" },
    Case { ord: 9, id: "ad_9999",      literal: "TO_DATE('9999-12-31 23:59:59','SYYYY-MM-DD HH24:MI:SS')",  note: "Oracle DATE 域上界" },
    Case { ord: 10, id: "ad_normal",   literal: "TO_DATE('2026-08-16 12:34:56','SYYYY-MM-DD HH24:MI:SS')",  note: "对照：普通当代日期" },
    Case { ord: 11, id: "null_row",    literal: "NULL", note: "空值路径" },
];

struct Fetched {
    ord: i32,
    case_id: String,
    /// `TO_CHAR(v,'SYYYY-MM-DD HH24:MI:SS')` —— Oracle 自己怎么看这个值（纪元的权威）。
    oracle_syyyy: Option<String>,
    /// `TO_CHAR(v,'YYYY-MM-DD AD')` —— 纪元位单独打出来。
    oracle_era: Option<String>,
    /// `DUMP(v)` —— 第 1 字节 = 世纪+100，< 100 即公元前。
    dump: Option<String>,
    /// 驱动交出来的 `Timestamp`（生产 `read_date` 同款绑定类型）。
    driver_year: Option<i32>,
    driver_debug: Option<String>,
    /// 生产取数路径的产物：`canon_date(y,m,d,h,mi,s)`。
    canon: Option<Result<String, String>>,
    insert_error: Option<String>,
    note: &'static str,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("TOTAL ERROR: bc-date probe could not run: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let user = env_or("ORACLE_USER", "spike");
    let password = env_or("ORACLE_PASSWORD", "spike123");
    let dsn = env_or("ORACLE_DSN", "//oracle:1521/XE");

    println!("== #98 公元前日期：驱动年份符号与 canon_date 纪元取证（源端段） ==");
    println!("DSN: {dsn}  user: {user}");

    let connection = Connection::connect(&user, &password, &dsn)?;
    print_session_facts(&connection)?;

    let rows = probe(&connection, "t_bc_date")?;

    println!("\n=== Oracle 侧事实：源端自己怎么看这个值 ===");
    print_oracle_side(&rows);

    println!("\n=== 驱动 + canon_date 事实：生产取数路径产出什么 ===");
    print_driver_side(&rows);

    println!("\n=== 纪元是否丢失：逐行判读 ===");
    print_era_verdict(&rows);

    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn print_session_facts(connection: &Connection) -> oracle::Result<()> {
    let version: String =
        connection.query_row_as("SELECT banner FROM v$version WHERE ROWNUM = 1", &[])?;
    let date_format: String = connection.query_row_as(
        "SELECT value FROM nls_session_parameters WHERE parameter = 'NLS_DATE_FORMAT'",
        &[],
    )?;
    let calendar: String = connection.query_row_as(
        "SELECT value FROM nls_session_parameters WHERE parameter = 'NLS_CALENDAR'",
        &[],
    )?;
    println!("Oracle: {version}");
    println!("NLS_DATE_FORMAT: {date_format:?}   NLS_CALENDAR: {calendar:?}");
    Ok(())
}

fn probe(connection: &Connection, table: &str) -> Result<Vec<Fetched>, Box<dyn Error>> {
    // 幂等：先拆再建。表不存在时 ORA-00942 是预期的。
    let _ = connection.execute(&format!("DROP TABLE {table}"), &[]);
    connection.execute(
        &format!("CREATE TABLE {table} (ord NUMBER(3) PRIMARY KEY, case_id VARCHAR2(40), v DATE)"),
        &[],
    )?;

    let mut errors = Vec::new();
    for case in CASES {
        let statement = format!(
            "INSERT INTO {table} (ord, case_id, v) VALUES ({}, '{}', {})",
            case.ord, case.id, case.literal
        );
        match connection.execute(&statement, &[]) {
            Ok(_) => {}
            // 源端就写不进去本身也是结论，记下继续跑。
            Err(error) => errors.push((case.ord, error.to_string())),
        }
    }
    connection.commit()?;

    let result = connection.query(
        &format!(
            "SELECT ord, case_id, \
TO_CHAR(v,'SYYYY-MM-DD HH24:MI:SS'), \
TO_CHAR(v,'YYYY-MM-DD HH24:MI:SS \"era=\"AD'), \
DUMP(v), v FROM {table} ORDER BY ord"
        ),
        &[],
    )?;

    let mut fetched = Vec::new();
    for row in result {
        let row = row?;
        let ord: i32 = row.get(0)?;
        let case = CASES
            .iter()
            .find(|case| case.ord == ord)
            .ok_or("fetched an ord that was never inserted")?;

        // 生产 `read_date` 的绑定类型，一字不差。
        let timestamp: Option<Timestamp> = row.get(5)?;
        let (driver_year, driver_debug, canon) = match timestamp {
            None => (None, None, None),
            Some(ts) => {
                let canon = canon_date(
                    ts.year(),
                    ts.month(),
                    ts.day(),
                    ts.hour(),
                    ts.minute(),
                    ts.second(),
                )
                .map_err(|error| error.to_string());
                (
                    Some(ts.year()),
                    Some(format!(
                        "y={} m={} d={} {:02}:{:02}:{:02} | to_string={:?}",
                        ts.year(),
                        ts.month(),
                        ts.day(),
                        ts.hour(),
                        ts.minute(),
                        ts.second(),
                        ts.to_string()
                    )),
                    Some(canon),
                )
            }
        };

        fetched.push(Fetched {
            ord,
            case_id: row.get(1)?,
            oracle_syyyy: row.get(2)?,
            oracle_era: row.get(3)?,
            dump: row.get(4)?,
            driver_year,
            driver_debug,
            canon,
            insert_error: None,
            note: case.note,
        });
    }

    for (ord, message) in errors {
        let case = CASES.iter().find(|case| case.ord == ord).expect("case exists");
        fetched.push(Fetched {
            ord,
            case_id: case.id.to_string(),
            oracle_syyyy: None,
            oracle_era: None,
            dump: None,
            driver_year: None,
            driver_debug: None,
            canon: None,
            insert_error: Some(message.lines().next().unwrap_or("").to_string()),
            note: case.note,
        });
    }
    fetched.sort_by_key(|row| row.ord);

    Ok(fetched)
}

fn print_oracle_side(rows: &[Fetched]) {
    println!(
        "{:<4} {:<14} {:<24} {:<32} {:<44} {}",
        "ord", "case_id", "SYYYY", "YYYY + 纪元位", "DUMP", "备注"
    );
    for row in rows {
        if let Some(error) = &row.insert_error {
            println!("{:<4} {:<14} INSERT 失败: {}   [{}]", row.ord, row.case_id, error, row.note);
            continue;
        }
        println!(
            "{:<4} {:<14} {:<24} {:<32} {:<44} {}",
            row.ord,
            row.case_id,
            opt(&row.oracle_syyyy),
            opt(&row.oracle_era),
            opt(&row.dump),
            row.note
        );
    }
}

fn print_driver_side(rows: &[Fetched]) {
    println!(
        "{:<4} {:<14} {:<12} {:<66} {}",
        "ord", "case_id", "driver_year", "驱动 Timestamp 字段", "canon_date 产物"
    );
    for row in rows {
        if row.insert_error.is_some() {
            println!("{:<4} {:<14} (源端未写入)", row.ord, row.case_id);
            continue;
        }
        let canon = match &row.canon {
            Some(Ok(value)) => format!("Ok({value:?})"),
            Some(Err(error)) => format!("Err({error})"),
            None => "NULL（空值路径，不进 canon_date）".to_string(),
        };
        println!(
            "{:<4} {:<14} {:<12} {:<66} {}",
            row.ord,
            row.case_id,
            row.driver_year
                .map(|year| year.to_string())
                .unwrap_or_else(|| "-".to_string()),
            opt(&row.driver_debug),
            canon
        );
    }
}

/// 纪元判读：源端是不是公元前 × 驱动年份符号 × canon_date 是否放行。
/// 「公元前 + 正年 + Ok」= 静默改值路径成立；「公元前 + 负年 + Err」= 现有断言已挡住。
fn print_era_verdict(rows: &[Fetched]) {
    println!("{:<4} {:<14} {:<10} {:<12} {:<10} {}", "ord", "case_id", "源端纪元", "driver_year", "canon", "判读");
    for row in rows {
        if row.insert_error.is_some() {
            continue;
        }
        let Some(era_text) = &row.oracle_era else {
            continue;
        };
        let is_bc = era_text.contains("era=BC");
        let era = if is_bc { "BC" } else { "AD" };
        let (canon_tag, canon_ok) = match &row.canon {
            Some(Ok(_)) => ("Ok", true),
            Some(Err(_)) => ("Err", false),
            None => continue,
        };
        let verdict = match (is_bc, row.driver_year.map(|year| year > 0), canon_ok) {
            (true, Some(true), true) => "*** 纪元丢失且被放行：静默改值路径成立 ***",
            (true, Some(true), false) => "纪元丢失，但 canon_date 另有理由拒（看 Err）",
            (true, Some(false), _) => "驱动给负年 → 纪元保住，年份域断言当场拒",
            (false, _, true) => "公元日期正常放行",
            (false, _, false) => "公元日期被拒（意外，看 Err）",
            (true, None, _) => "公元前但驱动无年份（意外）",
        };
        println!(
            "{:<4} {:<14} {:<10} {:<12} {:<10} {}",
            row.ord,
            row.case_id,
            era,
            row.driver_year
                .map(|year| year.to_string())
                .unwrap_or_else(|| "-".to_string()),
            canon_tag,
            verdict
        );
    }
}

fn opt(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "-".to_string())
}
