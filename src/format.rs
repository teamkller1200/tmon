/// 0..=100 の割合を `width` 文字のバー（█ と ░）にする。100 を超える値は 100 に丸める。
pub fn bar_string(percent: u32, width: usize) -> String {
    let pct = percent.min(100) as usize;
    let filled = (pct * width).saturating_div(100);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
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
}
