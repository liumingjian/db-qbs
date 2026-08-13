//! #3 —— `oracle` crate (ODPI-C) 类型保真度探针。
//!
//! 一次性 spike，不进主干。跑在 #9 的本地台架上（`docs/spikes/fixtures/local-rig/`）。
//!
//! 判据只有一个层次分明的结构：
//!   闸门  —— `NUMBER` 能否以**字符串**取到 38 位完整精度（ADR-0003 的硬前提）。
//!            拿不到 = 驱动不可用 = 触发 ADR-0001 复审、回退 Java。
//!   保真  —— 逐单元格 join `t_canon_expected`，比对 ADR-0003 规范形式。
//!   回报  —— ADR-0003 白名单之外的类型（RAW / LOB / LONG / BINARY_FLOAT|DOUBLE）。
//!            #11 已决：V1 明确不支持，映射预检报错拒绝。这里不判对错，
//!            只如实记录驱动取到了什么，好在 #2 的真实类型清单命中时知道要回炉补什么。
//!
//! 期望值一律来自 `t_canon_expected`，**不硬编码**（见台架 README）。

mod canon;

use canon::{canon_number, hex_upper};
use oracle::sql_type::{OracleType, Timestamp};
use oracle::{Connection, Row};
use std::collections::HashMap;
use std::fmt::Write as _;

/// 一个单元格的读取结果。
enum Cell {
    /// 驱动取到了值，已转成规范形式（白名单外的类型则是可比对的回报表示）。
    Value { raw: String, canon: String },
    /// SQL NULL。
    Null,
    /// 驱动读不出来 —— 这本身就是结论。
    Error(String),
}

impl Cell {
    fn canon(&self) -> Option<&str> {
        match self {
            Cell::Value { canon, .. } => Some(canon),
            _ => None,
        }
    }
    fn display(&self) -> String {
        match self {
            Cell::Value { raw, canon } if raw == canon => format!("{canon:?}"),
            Cell::Value { raw, canon } => format!("{canon:?} (驱动原样: {raw:?})"),
            Cell::Null => "<SQL NULL>".to_string(),
            Cell::Error(e) => format!("<读取失败: {e}>"),
        }
    }
}

/// 一条判定。
struct Verdict {
    row_id: i32,
    column: String,
    oracle_type: String,
    outcome: Outcome,
    detail: String,
}

enum Outcome {
    Pass,
    Fail,
    /// ADR-0003 白名单之外 —— V1 明确不支持（#11），不判对错，只回报驱动取到了什么。
    Excluded,
    /// 台架没给期望值，且不属于「V1 排除」—— 只观测。
    Observed,
}

impl Outcome {
    fn tag(&self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Excluded => "EXCL",
            Outcome::Observed => "OBS",
        }
    }
}

fn main() {
    match run() {
        Ok(failures) => {
            if failures > 0 {
                eprintln!("\n结论：**不通过** —— {failures} 项保真度断言失败。");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("\n探针自身异常（不是保真度结论）: {e}");
            std::process::exit(2);
        }
    }
}

fn run() -> oracle::Result<usize> {
    let user = env_or("ORACLE_USER", "spike");
    let pass = env_or("ORACLE_PASSWORD", "spike123");
    let dsn = env_or("ORACLE_DSN", "//oracle:1521/XE");

    println!("== #3 ODPI-C 类型保真度探针 ==");
    println!("DSN: {dsn}  用户: {user}");
    let conn = Connection::connect(&user, &pass, &dsn)?;
    print_env(&conn)?;

    let gate_ok = gate_number_full_precision(&conn)?;

    let expected = load_expected(&conn)?;
    let mut verdicts = Vec::new();
    probe_table(&conn, "t_types_probe", &expected, &mut verdicts)?;
    probe_table(&conn, "t_long_probe", &expected, &mut verdicts)?;
    probe_table(&conn, "t_longraw_probe", &expected, &mut verdicts)?;

    report(&verdicts, gate_ok, &expected);

    Ok(verdicts
        .iter()
        .filter(|v| matches!(v.outcome, Outcome::Fail))
        .count()
        + usize::from(!gate_ok))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn print_env(conn: &Connection) -> oracle::Result<()> {
    let banner: String = conn.query_row_as("SELECT banner FROM v$version WHERE ROWNUM = 1", &[])?;
    let cs: String = conn.query_row_as(
        "SELECT value FROM nls_database_parameters WHERE parameter = 'NLS_CHARACTERSET'",
        &[],
    )?;
    println!("服务端: {banner}");
    println!("NLS_CHARACTERSET: {cs}（台架为 AL32UTF8，GBK 路径测不了，见台架 README 边界一节）");
    println!("客户端库: {}\n", oracle::Version::client()?);
    Ok(())
}

/// 闸门：`NUMBER` 必须能以字符串取到 38 位完整精度，且**不经过任何浮点/定点中间类型**。
///
/// 这里刻意用 `String` 取值 —— 若驱动内部走了 `f64`，38 位会在这一步就被截断，
/// 断言直接暴露。另外补一发「f64 必然出错」的对照：把同一个值取成 `f64` 再打印，
/// 让报告里同时有正例与反例，#8 不必自己脑补。
fn gate_number_full_precision(conn: &Connection) -> oracle::Result<bool> {
    println!("-- 闸门判据：NUMBER 38 位完整精度（字符串路径）--");
    let mut ok = true;

    for (col, sign) in [("n_int38", "正"), ("n_bare", "正")] {
        let sql = format!("SELECT {col} FROM t_types_probe WHERE row_id = 2");
        match conn.query_row_as::<String>(&sql, &[]) {
            Ok(s) => {
                let c = canon_number(&s);
                let expect = "12345678901234567890123456789012345678";
                let pass = c == expect;
                ok &= pass;
                println!(
                    "   [{}] {col}({sign}) 取成 String = {s:?}（{} 位有效数字）",
                    if pass { "PASS" } else { "FAIL" },
                    c.trim_start_matches('-').replace('.', "").len()
                );
                if !pass {
                    println!("        期望 {expect:?}，实测规范形式 {c:?}");
                }
            }
            Err(e) => {
                ok = false;
                println!("   [FAIL] {col} 取成 String 失败：{e}");
            }
        }
    }

    // 反例对照：同一个值走 f64。
    match conn.query_row_as::<f64>("SELECT n_int38 FROM t_types_probe WHERE row_id = 2", &[]) {
        Ok(f) => println!("   [对照] 同一值取成 f64 = {f}  ← 精度已丢，这正是 ADR-0003 要绕开的路径"),
        Err(e) => println!("   [对照] 同一值取成 f64 失败：{e}"),
    }

    println!(
        "   闸门：{}\n",
        if ok {
            "**通过** —— 字符串路径可拿到完整 38 位"
        } else {
            "**不通过** —— 见上方 FAIL 项"
        }
    );
    Ok(ok)
}

/// `(row_id, COLUMN_NAME) -> (expected, note)`；`expected` 为 `None` 表示该单元格应为 SQL NULL，
/// 或（当 note 以「V1 排除」开头时）该类型在 ADR-0003 白名单之外，不做断言。
type ExpectedMap = HashMap<(i32, String), (Option<String>, Option<String>)>;

fn load_expected(conn: &Connection) -> oracle::Result<ExpectedMap> {
    let mut map = ExpectedMap::new();
    let rows = conn.query("SELECT row_id, column_name, expected, note FROM t_canon_expected", &[])?;
    for row in rows {
        let row = row?;
        let row_id: i32 = row.get(0)?;
        let col: String = row.get(1)?;
        let expected: Option<String> = row.get(2)?;
        let note: Option<String> = row.get(3)?;
        map.insert((row_id, col.to_uppercase()), (expected, note));
    }
    println!("t_canon_expected 载入 {} 个期望单元格\n", map.len());
    Ok(map)
}

fn probe_table(
    conn: &Connection,
    table: &str,
    expected: &ExpectedMap,
    out: &mut Vec<Verdict>,
) -> oracle::Result<()> {
    let sql = format!("SELECT * FROM {table} ORDER BY row_id");
    let mut stmt = conn.statement(&sql).build()?;
    let rows = stmt.query(&[])?;

    let cols: Vec<(String, OracleType)> = rows
        .column_info()
        .iter()
        .map(|ci| (ci.name().to_uppercase(), ci.oracle_type().clone()))
        .collect();

    for row in rows {
        let row = row?;
        let row_id: i32 = row.get("ROW_ID")?;
        for (idx, (name, otype)) in cols.iter().enumerate() {
            if name == "ROW_ID" || name == "KIND" {
                continue;
            }
            let cell = read_cell(&row, idx, otype);
            out.push(judge(row_id, name, otype, &cell, expected));
        }
    }
    Ok(())
}

/// 按列的 Oracle 类型选取值路径。
///
/// 原则：**能走字符串就走字符串**（ADR-0003 的搬运方式），
/// 只有日期时间用驱动的结构化类型再自己格式化 —— 因为字符串路径会被 `NLS_DATE_FORMAT`
/// 左右，那是会话配置，不是保真度。
fn read_cell(row: &Row, idx: usize, otype: &OracleType) -> Cell {
    macro_rules! fetch {
        ($t:ty) => {
            match row.get::<usize, Option<$t>>(idx) {
                Ok(Some(v)) => v,
                Ok(None) => return Cell::Null,
                Err(e) => return Cell::Error(e.to_string()),
            }
        };
    }

    match otype {
        OracleType::Number(..) | OracleType::Float(..) | OracleType::Int64 | OracleType::UInt64 => {
            let raw = fetch!(String);
            let canon = canon_number(&raw);
            Cell::Value { raw, canon }
        }
        OracleType::Date => {
            let ts = fetch!(Timestamp);
            let s = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                ts.year(), ts.month(), ts.day(), ts.hour(), ts.minute(), ts.second()
            );
            Cell::Value { raw: s.clone(), canon: s }
        }
        OracleType::Timestamp(_) | OracleType::TimestampTZ(_) | OracleType::TimestampLTZ(_) => {
            let ts = fetch!(Timestamp);
            // ADR-0003：固定 6 位，不足补零 —— 与 NUMBER 的去尾零方向相反，故不能复用。
            let s = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                ts.year(), ts.month(), ts.day(), ts.hour(), ts.minute(), ts.second(),
                ts.nanosecond() / 1_000
            );
            Cell::Value { raw: s.clone(), canon: s }
        }
        OracleType::Raw(_) | OracleType::LongRaw | OracleType::BLOB | OracleType::BFILE => {
            let bytes = fetch!(Vec<u8>);
            let hex = hex_upper(&bytes);
            Cell::Value { raw: hex.clone(), canon: hex }
        }
        // 字符类 / LOB / 机器浮点：原样字符串，不做任何规范化 ——
        // CHAR 的尾空格必须活着，BINARY_FLOAT/DOUBLE 的原样值正是要回报的东西。
        _ => {
            let s = fetch!(String);
            Cell::Value { raw: s.clone(), canon: s }
        }
    }
}

fn judge(
    row_id: i32,
    column: &str,
    otype: &OracleType,
    cell: &Cell,
    expected: &ExpectedMap,
) -> Verdict {
    let key = (row_id, column.to_string());
    let (outcome, detail) = match expected.get(&key) {
        // ADR-0003 白名单之外 —— 只回报，不判对错。判据在数据里（t_canon_expected.note），
        // 改 ADR 只需改台架的期望表，不必动这里。
        Some((_, note)) if note.as_deref().is_some_and(|n| n.starts_with("V1 排除")) => (
            Outcome::Excluded,
            format!(
                "{}；驱动取到 {}",
                note.as_deref().unwrap_or_default(),
                cell.display()
            ),
        ),
        Some((Some(exp), note)) => {
            let actual = cell.canon();
            if actual == Some(exp.as_str()) {
                (Outcome::Pass, format!("{}", cell.display()))
            } else {
                (
                    Outcome::Fail,
                    format!(
                        "期望 {exp:?}，实测 {}{}",
                        cell.display(),
                        note.as_deref().map(|n| format!("；备注：{n}")).unwrap_or_default()
                    ),
                )
            }
        }
        Some((None, note)) => match cell {
            Cell::Null => (Outcome::Pass, "SQL NULL，与期望一致".to_string()),
            _ => (
                Outcome::Fail,
                format!(
                    "期望 SQL NULL，实测 {}{}",
                    cell.display(),
                    note.as_deref().map(|n| format!("；备注：{n}")).unwrap_or_default()
                ),
            ),
        },
        // 台架没给期望值的单元格：仍然要看驱动读不读得出来 —— 读失败是硬伤。
        None => match cell {
            Cell::Error(e) => (Outcome::Fail, format!("台架未给期望值，但驱动读取失败：{e}")),
            _ => (Outcome::Observed, cell.display()),
        },
    };

    Verdict {
        row_id,
        column: column.to_string(),
        oracle_type: format!("{otype}"),
        outcome,
        detail,
    }
}

fn report(verdicts: &[Verdict], gate_ok: bool, expected: &ExpectedMap) {
    println!("-- 逐单元格判定 --");
    let mut buf = String::new();
    for v in verdicts {
        // OBS 项不淹没报告：只在有期望值或读失败时详列，其余折叠成一行摘要。
        let _ = writeln!(
            buf,
            "[{:6}] row {:>2}  {:<10} {:<22} {}",
            v.outcome.tag(),
            v.row_id,
            v.column,
            v.oracle_type,
            v.detail
        );
    }
    print!("{buf}");

    let count = |f: fn(&Outcome) -> bool| verdicts.iter().filter(|v| f(&v.outcome)).count();
    let pass = count(|o| matches!(o, Outcome::Pass));
    let fail = count(|o| matches!(o, Outcome::Fail));
    let excl = count(|o| matches!(o, Outcome::Excluded));
    let obs = count(|o| matches!(o, Outcome::Observed));

    println!("\n-- 汇总 --");
    println!("闸门（NUMBER 38 位字符串路径）：{}", if gate_ok { "通过" } else { "**不通过**" });
    println!("PASS {pass} / FAIL {fail} / EXCL {excl}（V1 白名单外，#11）/ OBS {obs}（台架未给期望值）");
    println!("期望表单元格 {}，其中已判定 {}", expected.len(), pass + fail + excl);
}
