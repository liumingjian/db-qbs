//! ADR-0003 规范形式。
//!
//! 这里只做「驱动给出的原始字符串 → 规范形式」这一步转换，**不做任何数值解析**：
//! 一旦经过 `f64` 或 `rust_decimal`，38 位精度就已经丢了，再断言也没意义。
//! 所以本模块全程 `&str` 进、`String` 出，只做字符搬运。

/// `NUMBER` 的规范形式：十进制字符串；去除前导零和小数尾零；整数不带小数点；
/// 负号前置；零一律为 `0`。
///
/// 小数点前的 `0` **保留**（`0.5` 而非 `.5`）—— 与台架 `t_canon_expected` 一致。
/// ADR-0003 对这一条未写明，见 #10；若 ADR 定成另一边，改这里 + 那张表。
pub fn canon_number(raw: &str) -> String {
    let raw = raw.trim();
    let (neg, digits) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };

    let (int_part, frac_part) = match digits.split_once('.') {
        Some((i, f)) => (i, f),
        None => (digits, ""),
    };

    let int_trimmed = int_part.trim_start_matches('0');
    let frac_trimmed = frac_part.trim_end_matches('0');

    // 零：正零负零一律归一为 "0"
    if int_trimmed.is_empty() && frac_trimmed.is_empty() {
        return "0".to_string();
    }

    let int_out = if int_trimmed.is_empty() { "0" } else { int_trimmed };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(int_out);
    if !frac_trimmed.is_empty() {
        out.push('.');
        out.push_str(frac_trimmed);
    }
    out
}

/// 二进制转十六进制大写 —— `RAW` / `BLOB` / `LONG RAW`。
/// **ADR-0003 未定义这几类的规范形式**，这里只是给出一个可比对的表示，
/// 用来回答「驱动能否无损取到字节」，不是在替 ADR 做决定。
pub fn hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_forms_collapse() {
        assert_eq!(canon_number("0"), "0");
        assert_eq!(canon_number("0.000"), "0");
        assert_eq!(canon_number("-0"), "0");
        assert_eq!(canon_number("-0.00"), "0");
        assert_eq!(canon_number(".0"), "0");
    }

    #[test]
    fn trailing_and_leading_zeros_stripped() {
        assert_eq!(canon_number("1.2300000000"), "1.23");
        assert_eq!(canon_number("100.00"), "100");
        assert_eq!(canon_number("007"), "7");
        assert_eq!(canon_number("-0.0100"), "-0.01");
    }

    #[test]
    fn leading_zero_before_point_is_kept() {
        // #10 未决时的现行口径：保留。
        assert_eq!(canon_number("0.5"), "0.5");
        assert_eq!(canon_number(".5"), "0.5");
        assert_eq!(canon_number("-.01"), "-0.01");
    }

    #[test]
    fn full_38_digits_survive() {
        let p = "12345678901234567890123456789012345678";
        assert_eq!(canon_number(p), p);
        assert_eq!(canon_number(&format!("-{p}")), format!("-{p}"));
        assert_eq!(
            canon_number("1234567890123456789012345678.0123456789"),
            "1234567890123456789012345678.0123456789"
        );
    }

    #[test]
    fn hex_is_upper_and_padded() {
        assert_eq!(hex_upper(&[0xde, 0xad, 0xbe, 0xef, 0x00]), "DEADBEEF00");
        assert_eq!(hex_upper(&[0x00, 0x01, 0x02, 0x03, 0x04, 0xff]), "0001020304FF");
    }
}
