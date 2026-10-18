use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::metrics::{Metrics, ProcessRow};

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

/// プロセス一覧のソート列。`s` でこの順に巡回する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Pid,
    #[default]
    Cpu,
    Memory,
    Disk,
}

impl SortKey {
    fn next(self) -> Self {
        match self {
            SortKey::Name => SortKey::Pid,
            SortKey::Pid => SortKey::Cpu,
            SortKey::Cpu => SortKey::Memory,
            SortKey::Memory => SortKey::Disk,
            SortKey::Disk => SortKey::Name,
        }
    }
}

/// 入力の解釈モード。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Filter,
    ConfirmKill,
}

/// `on_key` が返す、UI 層に任せる操作。
#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    Quit,
    Kill(u32),
}

/// プロセス名の部分一致、または PID の前方一致（空フィルタは全件一致）。
pub fn filter_matches(name: &str, pid: u32, filter: &str) -> bool {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return true;
    }
    name.to_ascii_lowercase().contains(&needle) || pid.to_string().starts_with(&needle)
}

/// `key` 列で安定ソートする（同値なら元の順を保つ）。
pub fn sort_rows(rows: &mut [ProcessRow], key: SortKey, desc: bool) {
    rows.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::Cpu => a.cpu.total_cmp(&b.cpu),
            SortKey::Memory => a.memory.cmp(&b.memory),
            SortKey::Disk => {
                (a.disk_read_bps + a.disk_write_bps).cmp(&(b.disk_read_bps + b.disk_write_bps))
            }
        };
        if desc { ord.reverse() } else { ord }
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Performance,
    Processes,
}

#[derive(Debug)]
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
    pub sort: SortKey,
    pub desc: bool,
    pub selected: usize,
    pub filter: String,
    pub mode: Mode,
    pub status: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            metrics: Metrics::default(),
            hist_cpu: History::new(),
            hist_mem: History::new(),
            hist_disk: History::new(),
            hist_net: History::new(),
            hist_gpu: History::new(),
            frame_count: 0,
            should_quit: false,
            // 起動時はタスクマネージャーと同じ CPU 降順。
            sort: SortKey::Cpu,
            desc: true,
            selected: 0,
            filter: String::new(),
            mode: Mode::Normal,
            status: None,
        }
    }
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
        // プロセスが消えると選択行が範囲外になるので寄せる。
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    /// 押下キーを状態に反映し、UI 層に渡す操作を返す。KeyEventKind::Press 以外は無視する。
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        match self.mode {
            Mode::Filter => {
                match key.code {
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char(c) => self.filter.push(c),
                    KeyCode::Enter | KeyCode::Esc => self.mode = Mode::Normal,
                    _ => {}
                }
                Action::None
            }
            // Task 5 で y/n の確認応答を実装する。
            Mode::ConfirmKill => Action::None,
            Mode::Normal => match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    Action::Quit
                }
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                    self.cycle_tab();
                    Action::None
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.cycle_tab();
                    Action::None
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_selection(1);
                    Action::None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_selection(-1);
                    Action::None
                }
                KeyCode::PageDown => {
                    self.move_selection(10);
                    Action::None
                }
                KeyCode::PageUp => {
                    self.move_selection(-10);
                    Action::None
                }
                KeyCode::Char('s') => {
                    self.sort = self.sort.next();
                    self.desc = true;
                    Action::None
                }
                KeyCode::Char('r') => {
                    self.desc = !self.desc;
                    Action::None
                }
                KeyCode::Char('/') => {
                    self.mode = Mode::Filter;
                    Action::None
                }
                // Task 5 で kill 確認に差し替える。
                KeyCode::Char('x') => {
                    self.status = Some("kill: not implemented yet".to_string());
                    Action::None
                }
                _ => Action::None,
            },
        }
    }

    /// フィルタとソートを適用した、表示順のプロセス行。
    pub fn visible(&self) -> Vec<ProcessRow> {
        let mut rows: Vec<ProcessRow> = self
            .metrics
            .processes
            .iter()
            .filter(|p| filter_matches(&p.name, p.pid, &self.filter))
            .cloned()
            .collect();
        sort_rows(&mut rows, self.sort, self.desc);
        rows
    }

    /// 選択中の行の PID。表示中の行が無い（または範囲外）なら None。
    pub fn selected_pid(&self) -> Option<u32> {
        self.visible().get(self.selected).map(|p| p.pid)
    }

    /// タブを巡回させる（2 枚しかないので next と prev は同じ）。
    fn cycle_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Performance => Tab::Processes,
            Tab::Processes => Tab::Performance,
        };
    }

    fn move_selection(&mut self, delta: i32) {
        let max = self.visible().len().saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, max) as usize;
    }
}

// ponytail: visible() は毎回全行を clone + sort する（数百行 x 1Hz なので実測上問題なし）。
// 数千プロセスで重くなったら &[ProcessRow] + インデックス配列に変える。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{Metrics, ProcessRow};
    use crossterm::event::KeyModifiers;

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

    fn row(pid: u32, name: &str, cpu: f32) -> ProcessRow {
        ProcessRow { pid, name: name.to_string(), cpu, memory: 0, disk_read_bps: 0, disk_write_bps: 0 }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with_processes(names: &[(&str, u32, f32)]) -> App {
        let mut app = App::new();
        app.on_tick(Metrics {
            processes: names.iter().map(|(n, pid, cpu)| row(*pid, n, *cpu)).collect(),
            ..Default::default()
        });
        app
    }

    #[test]
    fn filter_matches_name_case_insensitively_and_pid_prefix() {
        assert!(filter_matches("chrome.exe", 1234, "CHROME"));
        assert!(filter_matches("chrome.exe", 1234, "12"));
        assert!(filter_matches("chrome.exe", 1234, "  ")); // 空白のみは全件一致
        assert!(!filter_matches("chrome.exe", 1234, "firefox"));
        assert!(!filter_matches("chrome.exe", 1234, "999"));
    }

    #[test]
    fn sort_rows_by_cpu_desc_keeps_ties_stable() {
        let mut rows = vec![row(1, "a", 5.0), row(2, "b", 50.0), row(3, "c", 5.0)];
        sort_rows(&mut rows, SortKey::Cpu, true);
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![2, 1, 3]);
    }

    #[test]
    fn sort_rows_by_name_ascending() {
        let mut rows = vec![row(1, "Zebra", 0.0), row(2, "alpha", 0.0)];
        sort_rows(&mut rows, SortKey::Name, false);
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn selection_clamped_when_processes_shrink() {
        let mut app = app_with_processes(&[("a", 1, 1.0), ("b", 2, 2.0), ("c", 3, 3.0)]);
        app.selected = 2;
        app.on_tick(Metrics { processes: vec![row(1, "a", 1.0)], ..Default::default() });
        assert_eq!(app.selected, 0);
        assert_eq!(app.selected_pid(), Some(1));
    }

    #[test]
    fn filter_zero_matches_yields_no_selection() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        app.filter = "zzz".to_string();
        assert!(app.visible().is_empty());
        assert_eq!(app.selected_pid(), None);
    }

    #[test]
    fn sort_key_cycles_and_resets_to_descending() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        assert_eq!((app.sort, app.desc), (SortKey::Cpu, true));
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!((app.sort, app.desc), (SortKey::Memory, true));
        app.on_key(key(KeyCode::Char('r')));
        assert!(!app.desc);
    }

    #[test]
    fn filter_mode_edits_filter_and_esc_returns_to_normal() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        assert_eq!(app.on_key(key(KeyCode::Char('/'))), Action::None);
        assert_eq!(app.mode, Mode::Filter);
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.filter, "a");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.filter, "");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.should_quit); // Filter モード中の入力はコマンドとして解釈しない
    }

    #[test]
    fn navigation_and_quit_actions() {
        let mut app = app_with_processes(&[("a", 1, 1.0), ("b", 2, 2.0)]);
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1); // 末端で止まる
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::Quit);
        assert!(app.should_quit);
    }
}
