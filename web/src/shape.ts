// 六条 SQL 形状规则的中文措辞，唯一来源。
//
// source 侧回的 `check.message` 是英文（`crates/source/src/lib.rs` 的 `SHAPE_RULES`，
// 属于 API 语义不动它）。新建任务对话框和运行详情的形状预检卡都要给中文，
// 各抄一份迟早会漂，所以两边都从这里取。

import type { ShapeCheck } from "./api";

const SHAPE_RULE_LABELS: Readonly<Record<string, string>> = {
  business_date_range: "业务日期半开区间",
  no_additional_predicates: "WHERE 无额外谓词",
  named_projection: "每列显式命名",
  determinate_projection: "列精度可确定",
  no_relative_time_functions: "无相对时间函数",
  matching_date_columns: "源/目标日期列同名",
};

const SHAPE_RULE_DESCRIPTIONS: Readonly<Record<string, string>> = {
  business_date_range:
    "WHERE 必须按 source_date_col 写业务日期的半开区间：>= :biz_date 且 < :biz_date + 1。",
  no_additional_predicates: "WHERE 里除业务日期区间外不能再有别的谓词。",
  named_projection: "每一列都要显式命名，不能留没有列名的表达式。",
  determinate_projection: "每一列的精度必须静态可定，不能跑起来才知道。",
  no_relative_time_functions:
    "不能用 SYSDATE 这类相对时间函数，否则同一业务日期重跑结果会变。",
  matching_date_columns: "target_date_col 必须与 source_date_col 同名（忽略大小写）。",
};

export function shapeRuleLabel(rule: string): string {
  return SHAPE_RULE_LABELS[rule] ?? rule;
}

/** 规则的中文说明；遇到没登记的规则名就退回 source 给的原文，不吞信息。 */
export function shapeRuleDescription(rule: string, fallback: string): string {
  return SHAPE_RULE_DESCRIPTIONS[rule] ?? fallback;
}

/**
 * 未通过的形状规则条数。
 *
 * 结论条（`run.ts`）与运行详情的形状预检卡副标题（`RunScreen.tsx`）都要报这个数。
 * 各数一遍迟早会漂成两个数字摆在同一屏上，所以只留这一处。
 */
export function failedShapeRuleCount(checks: ReadonlyArray<ShapeCheck>): number {
  return checks.filter((check) => !check.passed).length;
}
