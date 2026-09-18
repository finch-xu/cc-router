//! 数字 / 时间 / 定宽文本的格式化。全部是纯函数。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `1284` → `1,284`
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `3_200_000` → `3.2M`; 小于 1000 原样。
pub fn compact(n: i64) -> String {
    let abs = n.unsigned_abs() as f64;
    let (div, unit) = match abs {
        a if a >= 1e9 => (1e9, "B"),
        a if a >= 1e6 => (1e6, "M"),
        a if a >= 1e3 => (1e3, "K"),
        _ => return n.to_string(),
    };
    format!("{}{:.1}{unit}", if n < 0 { "-" } else { "" }, abs / div)
}

/// `98.64` → `98.6%`
pub fn percent(p: f64) -> String {
    format!("{p:.1}%")
}

/// 剩余毫秒 → `mm:ss`; 超过 99 分钟显示 `99:59`, 负数显示 `00:00`。
pub fn mmss(remaining_ms: i64) -> String {
    let secs = (remaining_ms.max(0) + 999) / 1000; // 向上取整: 还剩 0.2s 时显示 00:01 而不是 00:00
    let secs = secs.min(99 * 60 + 59);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// 按**显示宽度**截断或右补空格到恰好 `width` 列。CJK 字符占两列, 不能用 `{:<16}`。
/// 截断时以 `…` 结尾; 宽字符放不下时用空格补齐那一列。
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return format!("{text}{}", " ".repeat(width - text.width()));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
        used += 1;
    }
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups_by_three() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1284), "1,284");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(thousands(-12345), "-12,345");
    }

    #[test]
    fn compact_picks_the_unit() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(3_200_000), "3.2M");
        assert_eq!(compact(15_300), "15.3K");
        assert_eq!(compact(2_000_000_000), "2.0B");
    }

    #[test]
    fn mmss_rounds_up_and_clamps() {
        assert_eq!(mmss(42_000), "00:42");
        assert_eq!(mmss(200), "00:01");
        assert_eq!(mmss(-5), "00:00");
        assert_eq!(mmss(61_000), "01:01");
        assert_eq!(mmss(10 * 3600 * 1000), "99:59");
    }

    #[test]
    fn fit_counts_display_width_not_chars() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("智谱主号", 10), "智谱主号  "); // 8 列 + 2 空格
        assert_eq!(fit("智谱主号备用", 8).width(), 8);
        assert_eq!(fit("智谱主号备用", 8), "智谱主… "); // 第 4 个字放不下, 省略号后补一格
        assert_eq!(fit("abcdefgh", 5), "abcd…");
        assert_eq!(fit("abc", 0), "");
    }
}
