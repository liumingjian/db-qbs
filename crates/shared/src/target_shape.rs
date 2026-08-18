//! 九行形态（ADR-0030 §1）的**推导**：源形态 → 目标形状、白名单成员、业务日期列可用性。
//!
//! 三句界线，改这个文件之前先读：
//!
//! 1. **共用的是推导，不是判定。** ADR-0030 §1 把「推导形状是什么」与「失效模式是哪一类」
//!    定为**类型的性质**；「怎么比」——`NUMBER` 的下界式、小数秒严格相等、目标字符集必须
//!    `utf8mb4`、目标列必须可空——是**判定式**，按 ADR-0010 §3.1 集中在 sink，
//!    **一行都不在本模块**。
//! 2. **判定式仍两端各一份**，兜底是 `crates/sink/tests/target_ddl_drift.rs` 那条
//!    「source 生成的表喂回 sink 预检必过」的回路（ADR-0027 §1 / A9 记的那笔账）。
//! 3. **ADR-0027 §1 第 2 条拒的是搬移职责**（把建表 SQL 的生成搬到 sink 去），
//!    **不是禁止共用推导**——见该 ADR 2026-08-18 增补。A5 括号里那句只管判定式。
//!    谁再提这条，先读这三句。
//!
//! 裸 `NUMBER` / 数值表达式列的 `(p,s)` 配置**不在本模块**：那是任务定义（source 侧）的输入，
//! [`classify_column`] 只看 describe，吐 [`ColumnShape::NeedsPrecision`]，
//! 配置的读取与校验留在 `crates/source/src/target_ddl.rs`。

use std::fmt;

use crate::{ColumnSupport, SourceColumn};

/// 推导出的目标形状。渲染成 MySQL 类型文字由 [`fmt::Display`] 负责——
/// 两端的文字（source 的建表 SQL、sink 的建议）都从同一个值渲染。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetShape {
    Decimal { precision: i64, scale: i64 },
    Varchar { length: u64 },
    Datetime { fsp: u32 },
}

impl fmt::Display for TargetShape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decimal { precision, scale } => write!(formatter, "DECIMAL({precision},{scale})"),
            Self::Varchar { length } => write!(formatter, "VARCHAR({length})"),
            Self::Datetime { fsp } => write!(formatter, "DATETIME({fsp})"),
        }
    }
}

/// 推不出形状的原因。**闭集**：加一种原因，两端的映射都编译不过，漏一处的路被堵死。
/// 措辞不在这里——两端各自映射成自己的文字（source 英文进建表卡报错、
/// sink 中文进预检报告），本模块只给类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeRejection {
    /// `NUMBER` 的 precision / scale 只有一半可判定。
    NumberPrecisionIncomplete,
    /// 字符族列 describe 给不出 length（字符表达式列，ADR-0030 §2）。
    CharacterLengthMissing,
    /// `TIMESTAMP` 列没带 fsp。
    TimestampFspMissing,
    /// `TIMESTAMP(n)`，`n > 6`（ADR-0030 §2）。
    TimestampFspTooPrecise { fsp: u32 },
    /// 推导形状超出 MySQL `DECIMAL(65,30)` 的表达能力（ADR-0030 §6 / ADR-0027 A5：
    /// 判的是**推导形状**，不是源 `(p,s)`）。
    DecimalShapeUnrepresentable { precision: i64, scale: i64 },
    /// 类型不在九行白名单内。
    TypeNotWhitelisted,
}

/// 分类的三条出路。它与 [`ColumnSupport`] 的三档一一对应（见 [`column_support`]）——
/// 那三档是**展示提示、不是预检裁决**（ADR-0010 2026-08-16 增补一），
/// 共用推导不改变这一点：sink 照样自己判，不读那个标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnShape {
    /// 已定形。
    Resolved(TargetShape),
    /// 裸 `NUMBER` / 数值表达式列：形状要等任务定义配 `(p,s)`（ADR-0030 §4）。
    NeedsPrecision,
    /// 推不出形状。
    Rejected(ShapeRejection),
}

/// 只看 describe 元数据分类一列。
pub fn classify_column(column: &SourceColumn) -> ColumnShape {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale)) => {
                let (precision, scale) = derive_number_shape(precision, scale);
                if is_supported_decimal_shape(precision, scale) {
                    ColumnShape::Resolved(TargetShape::Decimal { precision, scale })
                } else {
                    ColumnShape::Rejected(ShapeRejection::DecimalShapeUnrepresentable {
                        precision,
                        scale,
                    })
                }
            }
            (None, None) => ColumnShape::NeedsPrecision,
            _ => ColumnShape::Rejected(ShapeRejection::NumberPrecisionIncomplete),
        },
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => match column.length {
            Some(length) => ColumnShape::Resolved(TargetShape::Varchar { length }),
            None => ColumnShape::Rejected(ShapeRejection::CharacterLengthMissing),
        },
        "DATE" => ColumnShape::Resolved(TargetShape::Datetime { fsp: 0 }),
        "TIMESTAMP" => match column.fsp {
            Some(fsp) if fsp <= 6 => ColumnShape::Resolved(TargetShape::Datetime { fsp: 6 }),
            Some(fsp) => ColumnShape::Rejected(ShapeRejection::TimestampFspTooPrecise { fsp }),
            None => ColumnShape::Rejected(ShapeRejection::TimestampFspMissing),
        },
        _ => ColumnShape::Rejected(ShapeRejection::TypeNotWhitelisted),
    }
}

/// 三条出路 → 取列卡的三档标记（ADR-0027 2026-08-16 增补二 §1）。
pub fn column_support(shape: ColumnShape) -> ColumnSupport {
    match shape {
        ColumnShape::Resolved(_) => ColumnSupport::Ok,
        ColumnShape::NeedsPrecision => ColumnSupport::NeedsPrecision,
        ColumnShape::Rejected(_) => ColumnSupport::Unsupported,
    }
}

/// `NUMBER(p,s)` 三种形态的推导（ADR-0030 §1 形态 1/2/3）：
/// 常规 `0 ≤ s ≤ p` 原样、纯小数 `s > p` 取 `(s,s)`、负标度 `s < 0` 取 `(p+|s|,0)`。
///
/// **必须用饱和运算，不是防御性洁癖。** source 侧的 `(p,s)` 来自 Oracle 驱动 describe，
/// 受 precision ≤ 38、scale ∈ −84..127 约束，推导结果最大 `38 + 84 = 122`；
/// **但 sink 侧的 `(p,s)` 是从网线上收来的报文字段**（`POST /runs`），是对端给的任意 `i64`，
/// `i64::MAX` 配 `i64::MIN` 会同时撞上 `abs()` 与加法两处溢出。
/// 饱和到 `i64::MAX` 之后 [`is_supported_decimal_shape`] 照样判它装不进 `DECIMAL(65,30)`，
/// 落成一条普通的预检拒绝——见 `crates/sink/tests/sink_skeleton.rs` 的
/// `precheck_rejects_overflowing_number_metadata_without_panicking`。
pub fn derive_number_shape(precision: i64, scale: i64) -> (i64, i64) {
    if scale < 0 {
        (precision.saturating_add(scale.saturating_abs()), 0)
    } else if scale > precision {
        (scale, scale)
    } else {
        (precision, scale)
    }
}

/// 推导形状装不装得进 MySQL `DECIMAL(65,30)`（ADR-0030 §6：判**推导形状**，不是源 `(p,s)`）。
pub fn is_supported_decimal_shape(precision: i64, scale: i64) -> bool {
    (1..=65).contains(&precision) && (0..=30).contains(&scale) && scale <= precision
}

/// 能不能当业务日期列：`DATE` 或 `TIMESTAMP(0..=6)`，字符族与 `NUMBER` 族一律拒
/// （ADR-0027 A6）。
pub fn is_business_date_column(column: &SourceColumn) -> bool {
    match column.data_type.to_uppercase().as_str() {
        "DATE" => true,
        "TIMESTAMP" => column.fsp.is_some_and(|fsp| fsp <= 6),
        _ => false,
    }
}
