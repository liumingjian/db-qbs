//! #5 —— 10 万行流式 fetch 的**内存形状**探针（ODPI-C）。
//!
//! 一次性 spike，不进主干。跑在 #9 的本地台架上（`docs/spikes/fixtures/local-rig/`）。
//!
//! **台架能答与不能答**（见台架 README 的「边界」一节）：
//!   能答 —— 内存占用是随**批次大小**走还是随**总行数**走。这是驱动客户端侧的行为，
//!           与服务端跑在模拟层上无关，是 ADR-0001「同步阻塞 IO 够用」的真正前提：
//!           若随行数线性增长，说明驱动内部缓冲了全量结果，流式读是假的。
//!   不能答 —— 吞吐的**绝对数字**。服务端在 Rosetta 模拟层上，秒数是废数据。
//!             本程序仍打印耗时，只为看**相对趋势**（如 fetch_array_size 调大是否变快），
//!             绝对值一律不得写进结论。
//!
//! 一次进程只测一个配置 —— `/proc/self/status` 的 `VmHWM` 是进程存续期的峰值，
//! 在同一进程里连测多个配置，后面的配置会被前面的峰值污染。配置矩阵由 runner 脚本循环。
//!
//! **3b（客户端每行处理开销）**：内存那一半已经做完，剩下的问题是「同步阻塞 IO 的**实现**
//! 会不会成为瓶颈」。客户库快不快不是我们能决定的（那是服务端吞吐，已转 #2），
//! 能决定的是我们这一侧每搬一行烧多少 CPU。`getrusage(RUSAGE_SELF)` 的 `ru_utime + ru_stime`
//! 只计本进程真正占用 CPU 的时间，**等服务端的时间不计入**，因此**不受模拟层影响**；
//! 墙钟减掉它就是等服务端。四个 `cpu*` 模式把取数路径切成累进的四层，相邻两层相减即成本分解：
//!   cpu0 迭代行、不取值        —— 驱动的行推进与网络往返
//!   cpu1 取原生类型            —— ODPI-C 取值（i64 / f64 / String / Timestamp）
//!   cpu2 取规范形式文本、即弃  —— 数值与日期 → 文本
//!   cpu3 组 Vec<String> + 批次 —— 我们自己的组装与批次缓冲（= 完整搬运路径）
//!
//! 用法：
//!   spike-bulk <mode> <rows> <batch> <fetch_array_size> <prefetch_rows> [table] [ncols]
//!     mode  = baseline | stream | collect | cpu0 | cpu1 | cpu2 | cpu3
//!     ncols = cpu* 模式下取前几列（1..=4），用来量**每单元格**成本，好往生产的 70 列外推
//!     rows  = 读多少行（WHERE row_id <= N）
//!     batch = stream 模式下攒多少行算一批（模拟当前设计的批次落库），到批即清空
//!     table = 默认 t_bulk_probe；传 t_bulk_probe@fa 即走 dblink

use oracle::sql_type::Timestamp;
use oracle::Connection;
use std::time::Instant;

/// 一行被搬运成的形态 —— 与 ADR-0003 一致：能走字符串就走字符串。
type Cells = Vec<String>;

fn main() {
    if let Err(e) = run() {
        eprintln!("!! 探针失败: {e}");
        std::process::exit(1);
    }
}

fn run() -> oracle::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("stream");
    let rows_wanted: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let batch: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let fas: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(100);
    let prefetch: u32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(2);
    let table = args.get(6).map(String::as_str).unwrap_or("t_bulk_probe");
    let ncols: usize = args.get(7).and_then(|s| s.parse().ok()).unwrap_or(4).clamp(1, 4);

    let user = env_or("ORACLE_USER", "spike");
    let pass = env_or("ORACLE_PASSWORD", "spike123");
    let dsn = env_or("ORACLE_DSN", "//oracle:1521/XE");

    let rss_start = rss_kb("VmRSS");
    let conn = Connection::connect(&user, &pass, &dsn)?;
    let rss_connected = rss_kb("VmRSS");

    let t0 = Instant::now();
    let cpu_before = cpu_used();
    let (n_read, checksum, rss_peak_sampled) = match mode {
        // 只连库不查询 —— 量出「驱动 + Instant Client 常驻」的地板，
        // 后面每个配置的增量都要减掉它才有意义。
        "baseline" => (0u64, 0u64, rss_connected),
        "stream" => fetch_stream(&conn, table, rows_wanted, batch, fas, prefetch)?,
        "collect" => fetch_collect(&conn, table, rows_wanted, fas, prefetch)?,
        // 3b 的四层：同一条 SQL、同一批参数，只有「每行做多少事」不同。
        "cpu0" => fetch_layer(&conn, table, rows_wanted, batch, fas, prefetch, ncols, Layer::Iter)?,
        "cpu1" => fetch_layer(&conn, table, rows_wanted, batch, fas, prefetch, ncols, Layer::Native)?,
        "cpu2" => fetch_layer(&conn, table, rows_wanted, batch, fas, prefetch, ncols, Layer::Text)?,
        "cpu3" => fetch_layer(&conn, table, rows_wanted, batch, fas, prefetch, ncols, Layer::Full)?,
        other => {
            eprintln!("!! 未知 mode: {other}");
            std::process::exit(2);
        }
    };
    let elapsed_us = t0.elapsed().as_micros() as u64;
    let elapsed_ms = elapsed_us / 1000;
    let cpu = cpu_used().since(cpu_before);

    let hwm = rss_kb("VmHWM");
    println!(
        "== #5 内存形状探针 == mode={mode} rows={rows_wanted} batch={batch} \
         fetch_array_size={fas} prefetch_rows={prefetch} table={table}"
    );
    println!("读到 {n_read} 行，校验和 {checksum}");
    println!(
        "RSS: 起始 {rss_start} kB → 连上 {rss_connected} kB → 采样峰值 {rss_peak_sampled} kB；\
         VmHWM {hwm} kB"
    );
    println!("耗时 {elapsed_ms} ms（墙钟；模拟层上的绝对值作废，只看趋势）");
    println!(
        "客户端 CPU: user {} us + sys {} us = {} us；等服务端（墙钟 - CPU）{} us",
        cpu.user_us,
        cpu.sys_us,
        cpu.total(),
        elapsed_us.saturating_sub(cpu.total())
    );
    // 机器可读的一行，给 runner 汇总用。
    println!(
        "RESULT mode={mode} rows={rows_wanted} batch={batch} fas={fas} prefetch={prefetch} \
         table={table} n_read={n_read} rss_start_kb={rss_start} rss_conn_kb={rss_connected} \
         rss_peak_kb={rss_peak_sampled} vmhwm_kb={hwm} elapsed_ms={elapsed_ms} \
         ncols={ncols} cpu_user_us={} cpu_sys_us={} cpu_total_us={} wall_us={elapsed_us} \
         wait_us={}",
        cpu.user_us,
        cpu.sys_us,
        cpu.total(),
        elapsed_us.saturating_sub(cpu.total())
    );
    Ok(())
}

/// 流式：攒够 `batch` 行就「落库」（这里只做校验和后清空），内存应当只与 `batch` 有关。
fn fetch_stream(
    conn: &Connection,
    table: &str,
    rows_wanted: i64,
    batch: usize,
    fas: u32,
    prefetch: u32,
) -> oracle::Result<(u64, u64, u64)> {
    let sql = select_sql(table);
    let mut stmt = conn
        .statement(&sql)
        .fetch_array_size(fas)
        .prefetch_rows(prefetch)
        .build()?;
    let rows = stmt.query(&[&rows_wanted])?;

    let mut buf: Vec<Cells> = Vec::with_capacity(batch);
    let mut n = 0u64;
    let mut checksum = 0u64;
    let mut peak = rss_kb("VmRSS");
    for row in rows {
        let row = row?;
        buf.push(cells_of(&row)?);
        n += 1;
        if buf.len() >= batch {
            checksum = checksum.wrapping_add(drain_batch(&mut buf));
        }
        if n % 5000 == 0 {
            peak = peak.max(rss_kb("VmRSS"));
        }
    }
    checksum = checksum.wrapping_add(drain_batch(&mut buf));
    peak = peak.max(rss_kb("VmRSS"));
    Ok((n, checksum, peak))
}

/// 一次性全量：把所有行留在内存里，量「不流式」的代价。
fn fetch_collect(
    conn: &Connection,
    table: &str,
    rows_wanted: i64,
    fas: u32,
    prefetch: u32,
) -> oracle::Result<(u64, u64, u64)> {
    let sql = select_sql(table);
    let mut stmt = conn
        .statement(&sql)
        .fetch_array_size(fas)
        .prefetch_rows(prefetch)
        .build()?;
    let rows = stmt.query(&[&rows_wanted])?;

    let mut all: Vec<Cells> = Vec::new();
    let mut peak = rss_kb("VmRSS");
    for row in rows {
        let row = row?;
        all.push(cells_of(&row)?);
        if all.len() % 5000 == 0 {
            peak = peak.max(rss_kb("VmRSS"));
        }
    }
    peak = peak.max(rss_kb("VmRSS"));
    let n = all.len() as u64;
    let checksum = drain_batch(&mut all);
    Ok((n, checksum, peak))
}

fn select_sql(table: &str) -> String {
    // 绑定变量 —— #6 已实测能穿过 dblink，这里顺带保持同一条链路的写法。
    format!("SELECT row_id, n_amount, v_text, d_biz FROM {table} WHERE row_id <= :1 ORDER BY row_id")
}

/// 取值路径与 #3 一致：数字与文本走字符串，日期走驱动结构化类型再自己格式化。
fn cells_of(row: &oracle::Row) -> oracle::Result<Cells> {
    let row_id: String = row.get(0)?;
    let n_amount: Option<String> = row.get(1)?;
    let v_text: Option<String> = row.get(2)?;
    let d_biz: Option<Timestamp> = row.get(3)?;
    Ok(vec![
        row_id,
        n_amount.unwrap_or_default(),
        v_text.unwrap_or_default(),
        d_biz.map(fmt_ts).unwrap_or_default(),
    ])
}

fn fmt_ts(ts: Timestamp) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
}

/// 「落库」的替身：算个校验和再清空，保证编译器不能把整批数据优化掉。
fn drain_batch(buf: &mut Vec<Cells>) -> u64 {
    let mut sum = 0u64;
    for cells in buf.iter() {
        for c in cells {
            sum = sum.wrapping_add(c.len() as u64);
        }
    }
    buf.clear();
    sum
}

/// 从 `/proc/self/status` 读一个 kB 字段（`VmRSS` 当前 / `VmHWM` 进程存续期峰值）。
fn rss_kb(field: &str) -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ===== 3b：客户端每行 CPU 开销 =====

/// 取数路径的四个累进层级。相邻两层相减，就是那一层新增的每行成本。
#[derive(Clone, Copy, PartialEq)]
enum Layer {
    /// 只把行推过去，一个字段都不取。
    Iter,
    /// 取驱动原生类型：`NUMBER` → i64 / f64，`DATE` → `Timestamp`。
    Native,
    /// 取 ADR-0003 的规范形式文本，算完校验和立刻丢弃（不组装、不缓冲）。
    Text,
    /// 完整搬运路径：组 `Vec<String>`，攒够一批再清空 —— 与 `stream` 模式同构。
    Full,
}

/// 与 `select_sql` 同源，但只取前 `ncols` 列 —— 用来量每单元格成本。
fn select_sql_n(table: &str, ncols: usize) -> String {
    const COLS: [&str; 4] = ["row_id", "n_amount", "v_text", "d_biz"];
    let cols = COLS[..ncols].join(", ");
    format!("SELECT {cols} FROM {table} WHERE row_id <= :1 ORDER BY row_id")
}

#[allow(clippy::too_many_arguments)]
fn fetch_layer(
    conn: &Connection,
    table: &str,
    rows_wanted: i64,
    batch: usize,
    fas: u32,
    prefetch: u32,
    ncols: usize,
    layer: Layer,
) -> oracle::Result<(u64, u64, u64)> {
    let sql = select_sql_n(table, ncols);
    let mut stmt = conn
        .statement(&sql)
        .fetch_array_size(fas)
        .prefetch_rows(prefetch)
        .build()?;
    let rows = stmt.query(&[&rows_wanted])?;

    let batch = batch.max(1);
    let mut buf: Vec<Cells> = Vec::with_capacity(if layer == Layer::Full { batch } else { 0 });
    let mut n = 0u64;
    let mut checksum = 0u64;
    // RSS 采样在这里是副产品；本模式的判据是 CPU，采样频率保持与 stream 一致。
    let mut peak = rss_kb("VmRSS");
    for row in rows {
        let row = row?;
        match layer {
            Layer::Iter => {}
            Layer::Native => checksum = checksum.wrapping_add(native_cost(&row, ncols)?),
            Layer::Text => checksum = checksum.wrapping_add(text_cost(&row, ncols)?),
            Layer::Full => {
                buf.push(cells_n(&row, ncols)?);
                if buf.len() >= batch {
                    checksum = checksum.wrapping_add(drain_batch(&mut buf));
                }
            }
        }
        n += 1;
        if n % 5000 == 0 {
            peak = peak.max(rss_kb("VmRSS"));
        }
    }
    checksum = checksum.wrapping_add(drain_batch(&mut buf));
    peak = peak.max(rss_kb("VmRSS"));
    Ok((n, checksum, peak))
}

/// 取原生类型，不做任何文本化。校验和只为挡住优化。
fn native_cost(row: &oracle::Row, ncols: usize) -> oracle::Result<u64> {
    let mut sum = 0u64;
    if ncols >= 1 {
        let row_id: i64 = row.get(0)?;
        sum = sum.wrapping_add(row_id as u64);
    }
    if ncols >= 2 {
        let n_amount: Option<f64> = row.get(1)?;
        sum = sum.wrapping_add(n_amount.unwrap_or(0.0) as u64);
    }
    if ncols >= 3 {
        let v_text: Option<String> = row.get(2)?;
        sum = sum.wrapping_add(v_text.map(|s| s.len() as u64).unwrap_or(0));
    }
    if ncols >= 4 {
        let d_biz: Option<Timestamp> = row.get(3)?;
        sum = sum.wrapping_add(d_biz.map(|t| t.year() as u64).unwrap_or(0));
    }
    Ok(sum)
}

/// 取规范形式文本（与 `cells_of` 同一条取值路径），但算完就丢，不组装、不缓冲。
fn text_cost(row: &oracle::Row, ncols: usize) -> oracle::Result<u64> {
    let mut sum = 0u64;
    if ncols >= 1 {
        let row_id: String = row.get(0)?;
        sum = sum.wrapping_add(row_id.len() as u64);
    }
    if ncols >= 2 {
        let n_amount: Option<String> = row.get(1)?;
        sum = sum.wrapping_add(n_amount.map(|s| s.len() as u64).unwrap_or(0));
    }
    if ncols >= 3 {
        let v_text: Option<String> = row.get(2)?;
        sum = sum.wrapping_add(v_text.map(|s| s.len() as u64).unwrap_or(0));
    }
    if ncols >= 4 {
        let d_biz: Option<Timestamp> = row.get(3)?;
        sum = sum.wrapping_add(d_biz.map(|t| fmt_ts(t).len() as u64).unwrap_or(0));
    }
    Ok(sum)
}

/// `cells_of` 的前 `ncols` 列版本。
fn cells_n(row: &oracle::Row, ncols: usize) -> oracle::Result<Cells> {
    let mut cells = Vec::with_capacity(ncols);
    if ncols >= 1 {
        cells.push(row.get::<usize, String>(0)?);
    }
    if ncols >= 2 {
        cells.push(row.get::<usize, Option<String>>(1)?.unwrap_or_default());
    }
    if ncols >= 3 {
        cells.push(row.get::<usize, Option<String>>(2)?.unwrap_or_default());
    }
    if ncols >= 4 {
        cells.push(
            row.get::<usize, Option<Timestamp>>(3)?
                .map(fmt_ts)
                .unwrap_or_default(),
        );
    }
    Ok(cells)
}

/// `getrusage(RUSAGE_SELF)` 读到的进程 CPU 时间。**不含等 IO 的时间**，
/// 所以服务端跑在模拟层上不影响这个数 —— 这正是 3b 能在台架上做的原因。
#[derive(Clone, Copy)]
struct Cpu {
    user_us: u64,
    sys_us: u64,
}

impl Cpu {
    fn total(&self) -> u64 {
        self.user_us + self.sys_us
    }
    fn since(&self, before: Cpu) -> Cpu {
        Cpu {
            user_us: self.user_us.saturating_sub(before.user_us),
            sys_us: self.sys_us.saturating_sub(before.sys_us),
        }
    }
}

fn cpu_used() -> Cpu {
    // SAFETY: getrusage 只写入我们提供的这块 rusage 内存。
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
            return Cpu { user_us: 0, sys_us: 0 };
        }
        let us = |tv: libc::timeval| tv.tv_sec as u64 * 1_000_000 + tv.tv_usec as u64;
        Cpu {
            user_us: us(ru.ru_utime),
            sys_us: us(ru.ru_stime),
        }
    }
}
