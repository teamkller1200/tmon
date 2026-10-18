/// 0..=100 の割合を `width` 文字のバー（█ と ░）にする。100 を超える値は 100 に丸める。
pub fn bar_string(percent: u32, width: usize) -> String {
    let pct = percent.min(100) as usize;
    let filled = (pct * width).saturating_div(100);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// バイト数を 1024 進で `1.5 KB` の形にする。
pub fn bytes(v: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = v as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{} B", v) } else { format!("{:.1} {}", value, UNITS[unit]) }
}

/// 1 秒あたりのバイト数を `1.5 KB/s` の形にする。
pub fn rate(bps: u64) -> String {
    format!("{}/s", bytes(bps))
}

/// 0..=100 の割合を小数点 1 桁で表す。
pub fn percent(v: f32) -> String {
    format!("{:.1}%", v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_string_scales_correctly() {
        assert_eq!(bar_string(0, 10), "░".repeat(10));
        assert_eq!(bar_string(50, 10), "█████░░░░░");
        assert_eq!(bar_string(100, 10), "█".repeat(10));
    }

    #[test]
    fn bytes_formats_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn rate_appends_per_second() {
        assert_eq!(rate(0), "0 B/s");
        assert_eq!(rate(1024), "1.0 KB/s");
    }

    #[test]
    fn percent_has_one_decimal() {
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(42.06), "42.1%");
        assert_eq!(percent(100.0), "100.0%");
    }
}
