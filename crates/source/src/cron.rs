//! 五字段 cron 表达式的**解析与推算**——手写，不引入依赖。
//!
//! 打包链是离线交叉编译，多一个 crate 就多一份 vendor 负担及其整棵传递树；而这个产品
//! 真正用到的表达式形态只有 `*`、`a`、`a-b`、`*/n`、`a-b/n` 五种，`L`、`W`、`#`、秒字段
//! 一个都用不上。一个能被表格化用例完全钉死的纯函数，比一个九成功能永远不会被执行的
//! 库更容易说清楚「凌晨两点」到底是哪个两点。
//!
//! **它是纯函数**：[`CronSchedule::parse`] 只吃一段文本，[`CronSchedule::next_after`]
//! 吃一个时刻、吐下一个触发时刻。时区不在这一层——本模块从头到尾只认
//! [`chrono::NaiveDateTime`] 这种没有时区的挂钟时间。谁调用它、按哪个时区把挂钟时间
//! 换算成真实时刻，是调用方的事（今天是服务器本地时区，见 `http::handle_schedule_preview`）。
//! 把时区揉进来会让「表达式 + 时刻 → 时刻」这条唯一的判定式多出一个说不清的输入。
//!
//! 语义按 Vixie cron 的老规矩，其中**只有一条会让人意外**，因此写在这里：**「日」和
//! 「周」两个字段同时被限定时，它们是「或」而不是「且」**。`0 0 1 * 1` 是「每月 1 号
//! 以及每个周一」，不是「每月 1 号且恰好是周一」。这是 cron 三十年的既定含义，改掉它
//! 等于让同一段文本在这里和在系统 crontab 里是两个意思。界面上的「下次触发」读数就是
//! 这条规则最好的解释器——它把语义摆出来给人看，而不是让人去猜。

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// 往前找多远还找不到就认输：约八年。
///
/// `0 0 29 2 *`（只在 2 月 29 号）在本世纪最长要等八年，是所有合法表达式里跨度最大的
/// 一种。找不到不是错误，是**这个表达式永远不会触发**——`30 2`（2 月 30 号）就是。
/// 那种情况返回 `None`，由调用方去说人话，而不是在这里假装找到了一个时刻。
const MAX_LOOKAHEAD_DAYS: u32 = 366 * 9;

/// 一个字段的取值集合。
///
/// 用位掩码存**取值本身**（「日」用第 1..=31 位，「月」用第 1..=12 位），而不是从 0 起
/// 的下标——省掉每次读写的 ±1，那个 ±1 是这类代码里最经典的一处错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    bits: u64,
    /// 这个字段有没有被限定过。只有「日」和「周」用得上它，见模块头那条「或」规则。
    restricted: bool,
}

impl Field {
    const fn has(self, value: u32) -> bool {
        self.bits & (1u64 << value) != 0
    }
}

/// 一个字段的名字与合法区间。错误消息里那句「分钟字段」就是从这里来的。
struct Bounds {
    name: &'static str,
    low: u32,
    high: u32,
}

const MINUTE: Bounds = Bounds { name: "分钟", low: 0, high: 59 };
const HOUR: Bounds = Bounds { name: "小时", low: 0, high: 23 };
const DAY_OF_MONTH: Bounds = Bounds { name: "日", low: 1, high: 31 };
const MONTH: Bounds = Bounds { name: "月", low: 1, high: 12 };
/// 「周」收 0..=7，7 与 0 都是周日——这是 cron 的通用写法，两种都收才不会让人被拒得莫名。
const DAY_OF_WEEK: Bounds = Bounds { name: "星期", low: 0, high: 7 };

/// 一段解析好的 cron 表达式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: Field,
    hours: Field,
    days_of_month: Field,
    months: Field,
    days_of_week: Field,
}

impl CronSchedule {
    /// 解析五字段表达式。失败时的 `Err` 是**直接给人看的一句话**，不是错误码：
    /// 它会原样出现在保存被拒时的提示里。
    pub fn parse(expression: &str) -> Result<Self, String> {
        let trimmed = expression.trim();
        if trimmed.is_empty() {
            return Err("cron 表达式不能为空".to_owned());
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "cron 表达式要五个字段（分 时 日 月 周），这里有 {} 个：{trimmed}",
                fields.len()
            ));
        }
        Ok(Self {
            minutes: parse_field(fields[0], &MINUTE)?,
            hours: parse_field(fields[1], &HOUR)?,
            days_of_month: parse_field(fields[2], &DAY_OF_MONTH)?,
            months: parse_field(fields[3], &MONTH)?,
            days_of_week: normalize_week(parse_field(fields[4], &DAY_OF_WEEK)?),
        })
    }

    /// **严格晚于** `after` 的下一个触发时刻，秒与纳秒一律归零。
    ///
    /// 「严格晚于」是刻意的：调度器拿上一次的触发时刻来问下一次，取等号会让它在同一分钟
    /// 里原地打转。返回 `None` 只有一个含义——这个表达式在可预见的将来永不触发。
    pub fn next_after(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let start = after
            .with_second(0)?
            .with_nanosecond(0)?
            .checked_add_signed(chrono::Duration::minutes(1))?;
        let mut date = start.date();
        let start_minute_of_day = start.time().hour() * 60 + start.time().minute();
        for offset in 0..MAX_LOOKAHEAD_DAYS {
            if offset > 0 {
                date = date.succ_opt()?;
            }
            if !self.date_matches(date) {
                continue;
            }
            let from = if offset == 0 { start_minute_of_day } else { 0 };
            for minute_of_day in from..24 * 60 {
                let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
                if self.hours.has(hour) && self.minutes.has(minute) {
                    return date.and_hms_opt(hour, minute, 0);
                }
            }
        }
        None
    }

    /// 接下来的 `count` 个触发时刻。界面上的「下次触发」读数就吃这个。
    pub fn upcoming(&self, after: NaiveDateTime, count: usize) -> Vec<NaiveDateTime> {
        let mut cursor = after;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            match self.next_after(cursor) {
                Some(next) => {
                    out.push(next);
                    cursor = next;
                }
                None => break,
            }
        }
        out
    }

    /// 「月」必须中，「日」与「周」按模块头那条规则：两个都被限定时取**并集**。
    fn date_matches(&self, date: NaiveDate) -> bool {
        if !self.months.has(date.month()) {
            return false;
        }
        let by_day = self.days_of_month.has(date.day());
        let by_week = self.days_of_week.has(date.weekday().num_days_from_sunday());
        match (self.days_of_month.restricted, self.days_of_week.restricted) {
            (true, true) => by_day || by_week,
            (true, false) => by_day,
            (false, true) => by_week,
            (false, false) => true,
        }
    }
}

/// 7 折回 0：两个写法指的是同一个周日，位掩码里只留一个。
fn normalize_week(mut field: Field) -> Field {
    if field.has(7) {
        field.bits &= !(1u64 << 7);
        field.bits |= 1;
    }
    field
}

fn parse_field(field: &str, bounds: &Bounds) -> Result<Field, String> {
    // 「有没有被限定」只看字面：光秃秃一个 `*` 才叫没限定。`*/1` 与它等价，但把它也算成
    // 「没限定」需要先算出集合再比对，那会让这条判据依赖另一条判据。
    let restricted = field != "*";
    let mut bits = 0u64;
    for item in field.split(',') {
        let (low, high, step) = parse_item(item, bounds)?;
        let mut value = low;
        while value <= high {
            bits |= 1u64 << value;
            value += step;
        }
    }
    Ok(Field { bits, restricted })
}

/// 一个逗号项 → `(起, 止, 步长)`。这是整个语法唯一被承认的形状表。
fn parse_item(item: &str, bounds: &Bounds) -> Result<(u32, u32, u32), String> {
    let (base, step) = match item.split_once('/') {
        Some((base, step_text)) => {
            let step: u32 = step_text.parse().map_err(|_| unknown(item, bounds))?;
            if step == 0 {
                return Err(format!("{}字段的步长要大于 0：{item}", bounds.name));
            }
            // 步长只跟在 `*` 或 `a-b` 后面。`5/10` 在有些实现里是「从 5 起每 10 个」，
            // 但它读起来像除法，而这个产品用不到；与其收下一个会被读错的写法，不如拒了。
            if base != "*" && !base.contains('-') {
                return Err(format!(
                    "{}字段的步长只能跟在 * 或 a-b 后面：{item}",
                    bounds.name
                ));
            }
            (base, step)
        }
        None => (item, 1),
    };
    if base == "*" {
        return Ok((bounds.low, bounds.high, step));
    }
    let (low, high) = match base.split_once('-') {
        Some((low_text, high_text)) => (
            parse_value(low_text, item, bounds)?,
            parse_value(high_text, item, bounds)?,
        ),
        None => {
            let value = parse_value(base, item, bounds)?;
            (value, value)
        }
    };
    if low > high {
        return Err(format!(
            "{}字段的区间起点比终点大：{item}",
            bounds.name
        ));
    }
    Ok((low, high, step))
}

fn parse_value(text: &str, item: &str, bounds: &Bounds) -> Result<u32, String> {
    let value: u32 = text.parse().map_err(|_| unknown(item, bounds))?;
    if value < bounds.low || value > bounds.high {
        return Err(format!(
            "{}字段的 {value} 超出取值范围 {}-{}",
            bounds.name, bounds.low, bounds.high
        ));
    }
    Ok(value)
}

fn unknown(item: &str, bounds: &Bounds) -> String {
    format!(
        "{}字段看不懂这一项：{item}（只支持 *、a、a-b、*/n、a-b/n，以及用逗号并列）",
        bounds.name
    )
}
