use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::metrics::Metrics;

/// スパークラインに残すサンプル数（1 秒 tick なので 60 秒分）。
pub const HISTORY_LEN: usize = 60;

/// 末尾 HISTORY_LEN 個だけ保持する数値履歴。
#[derive(Debug, Default)]
pub struct History(VecDeque<u64>);

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, v: u64) {
        if self.0.len() == HISTORY_LEN {
            self.0.pop_front();
        }
        self.0.push_back(v);
    }

    pub fn as_vec(&self) -> Vec<u64> {
        self.0.iter().copied().collect()
    }

    pub fn latest(&self) -> u64 {
        self.0.back().copied().unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Performance,
    Processes,
}

#[derive(Default)]
pub struct App {
    pub tab: Tab,
    pub metrics: Metrics,
    pub hist_cpu: History,
    pub hist_mem: History,
    pub hist_disk: History,
    pub hist_net: History,
    pub hist_gpu: History,
    pub frame_count: u64,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// 1 秒 tick ごとに呼ぶ。計測値を保持し、グラフ用の履歴を 1 つ進める。
    pub fn on_tick(&mut self, metrics: Metrics) {
        self.hist_cpu.push(metrics.cpu_total.round() as u64);
        let mem_pct = if metrics.mem_total > 0 {
            (metrics.mem_used as f64 / metrics.mem_total as f64 * 100.0).round() as u64
        } else {
            0
        };
        self.hist_mem.push(mem_pct);
        self.hist_disk.push((metrics.disk_read_bps + metrics.disk_write_bps) / 1024);
        self.hist_net.push((metrics.net_rx_bps + metrics.net_tx_bps) / 1024);
        self.hist_gpu.push(metrics.gpu.as_ref().map_or(0, |g| g.gpu_util) as u64);
        self.metrics = metrics;
        self.frame_count += 1;
    }

    /// 押下キーを状態に反映する。KeyEventKind::Press 以外は無視する。
    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;

    #[test]
    fn history_caps_at_history_len() {
        let mut h = History::new();
        for i in 0..(HISTORY_LEN + 5) {
            h.push(i as u64);
        }
        assert_eq!(h.as_vec().len(), HISTORY_LEN);
        assert_eq!(h.latest(), (HISTORY_LEN + 4) as u64); // 最後に push した値
    }

    #[test]
    fn on_tick_stores_metrics_and_pushes_history() {
        let mut app = App::new();
        let m = Metrics { cpu_total: 12.5, mem_total: 100, mem_used: 50, ..Default::default() };
        app.on_tick(m);
        assert_eq!(app.metrics.cpu_total, 12.5);
        assert_eq!(app.hist_cpu.latest(), 13); // 12.5 -> round して u64
        assert_eq!(app.hist_mem.latest(), 50); // 使用率%
        assert_eq!(app.frame_count, 1);
    }
}
