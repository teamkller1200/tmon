use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::metrics::{KillOutcome, Metrics, ProcessRow, kill_blocked_reason, own_pid};

/// スパークラインに残すサンプル数（1 秒 tick なので 60 秒分）。
pub const HISTORY_LEN: usize = 60;


/// 末尾 HISTORY_LEN 個だけ保持する数値履歴。
#[derive(Clone, Debug, Default)]
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

    /// テスト専用（本体は `as_vec()` でグラフに渡すだけ）。
    #[cfg(test)]
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

/// ステータス行の配色（spec §6.0 の kill 結果の色分け）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusKind {
    #[default]
    Info,
    Ok,
    Warn,
    Err,
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

impl Tab {
    /// タブの並び（ui はこの順に描き、ヘッダのクリック位置もこの順で引く）。
    const ALL: [Tab; 2] = [Tab::Performance, Tab::Processes];

    /// ui が描くタブ見出し。
    pub fn titles() -> [&'static str; 2] {
        ["Performance", "Processes"]
    }

    /// ヘッダ行の桁位置からタブを引く。ratatui の Tabs は「空白 1 + 見出し + 空白 1」を
    /// 並べ、タブの間を区切り `│` 1 桁で挟む。区切りは左のタブに含める。
    pub fn hit(column: u16) -> Option<Tab> {
        let titles = Self::titles();
        let mut start = 0u16;
        for (i, title) in titles.iter().enumerate() {
            let last = i + 1 == titles.len();
            let end = start + title.len() as u16 + 2 + if last { 0 } else { 1 };
            if column < end {
                return Some(Self::ALL[i]);
            }
            start = end;
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct App {
    pub tab: Tab,
    pub metrics: Metrics,
    pub hist_cpu: History,
    pub hist_mem: History,
    pub hist_disk: History,
    pub hist_net: History,
    pub hist_gpu: History,
    pub should_quit: bool,
    pub sort: SortKey,
    pub desc: bool,
    pub selected: usize,
    pub filter: String,
    pub mode: Mode,
    pub status: Option<String>,
    pub status_kind: StatusKind,
    pub pending_kill: Option<u32>,
    pub pending_name: Option<String>,
    pub pending_start: Option<u64>,
    /// プロセス表を描いた矩形と、そのとき Table が使ったスクロール量。ui が毎フレーム書き、
    /// マウスクリックをどの行に当てるかの判定に使う。
    pub rows_area: Rect,
    pub rows_offset: usize,
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
            should_quit: false,
            // 起動時はタスクマネージャーと同じ CPU 降順。
            sort: SortKey::Cpu,
            desc: true,
            selected: 0,
            filter: String::new(),
            mode: Mode::Normal,
            status: None,
            status_kind: StatusKind::Info,
            pending_kill: None,
            pending_name: None,
            pending_start: None,
            rows_area: Rect::ZERO,
            rows_offset: 0,
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
        // プロセスが消えると選択行が範囲外になるので寄せる。
        self.clamp_selection();
    }

    /// 押下キーを状態に反映する。`y` で確定した kill は (PID, 確認時の開始時刻) を返す。
    /// KeyEventKind::Press 以外は無視する。
    pub fn on_key(&mut self, key: KeyEvent) -> Option<(u32, u64)> {
        if key.kind != KeyEventKind::Press {
            return None;
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
                // 絞り込みで行が減っても、選択行が表示とずれたまま残らないようその場で寄せる。
                self.clamp_selection();
                None
            }
            Mode::ConfirmKill => {
                // y 以外（q を含む）はすべてキャンセル扱い。
                let pid = self.pending_kill.take();
                let start = self.pending_start.take();
                self.leave_modal();
                match (key.code, pid, start) {
                    (KeyCode::Char('y') | KeyCode::Char('Y'), Some(pid), Some(start)) => {
                        Some((pid, start))
                    }
                    _ => None,
                }
            }
            Mode::Normal => match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    None
                }
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                    self.cycle_tab();
                    None
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.cycle_tab();
                    None
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_selection(1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_selection(-1);
                    None
                }
                KeyCode::PageDown => {
                    self.move_selection(10);
                    None
                }
                KeyCode::PageUp => {
                    self.move_selection(-10);
                    None
                }
                KeyCode::Char('s') => {
                    self.sort = self.sort.next();
                    self.desc = true;
                    None
                }
                KeyCode::Char('r') => {
                    self.desc = !self.desc;
                    None
                }
                KeyCode::Char('/') => {
                    self.mode = Mode::Filter;
                    None
                }
                KeyCode::Char('x') => {
                    self.request_kill(own_pid());
                    None
                }
                _ => None,
            },
        }
    }

    /// マウス 1 イベントを状態に反映する。ヘッダ行の見出しクリックでタブを切り替え、
    /// ホイールでプロセス選択を動かす。選択ごと動かすのは、見えていない行に `x` が効くのを
    /// 防ぐため（Table は選択行が見える位置まで自分でスクロールする）。
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if mouse.row == 0 => {
                // 確認プロンプトやフィルタ入力は Processes タブのフッタにしか出ないので、
                // タブを移る前に閉じる（確認が見えないまま y が効くのを防ぐ）。
                self.leave_modal();
                if let Some(tab) = Tab::hit(mouse.column) {
                    self.tab = tab;
                }
            }
            // 表の行のクリックでその行を選ぶ（枠と見出しの 2 行は `row_at` が除く）。
            MouseEventKind::Down(MouseButton::Left) if self.tab == Tab::Processes => {
                if let Some(index) = self.row_at(mouse.column, mouse.row) {
                    self.leave_modal();
                    self.selected = index;
                }
            }
            // ホイール 1 ノッチ = 1 行。
            MouseEventKind::ScrollDown if self.tab == Tab::Processes => {
                self.move_selection(1);
            }
            MouseEventKind::ScrollUp if self.tab == Tab::Processes => {
                self.move_selection(-1);
            }
            _ => {}
        }
    }

    /// 画面座標からプロセス表の行番号を引く。`rows_area` / `rows_offset` は ui が描画時に書く。
    /// 表の外・枠・見出し・データより下は None。
    fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.rows_area;
        let top = area.y + 2; // 枠 1 行 + 見出し 1 行
        if area.is_empty()
            || row < top
            || row >= area.bottom()
            || column < area.x
            || column >= area.right()
        {
            return None;
        }
        let index = self.rows_offset + (row - top) as usize;
        (index < self.visible().len()).then_some(index)
    }

    /// モーダル（kill の確認・フィルタ入力）を閉じる。
    fn leave_modal(&mut self) {
        self.pending_kill = None;
        self.pending_name = None;
        self.pending_start = None;
        self.mode = Mode::Normal;
    }

    /// 選択行が現在の一覧に収まるよう寄せる。
    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    /// 選択行の kill を要求する。Processes タブ以外と保護対象では確認を出さない
    /// （Performance タブにはフッタが無く、確認プロンプトが見えないまま kill できてしまう）。
    pub fn request_kill(&mut self, own_pid: Option<u32>) {
        if self.tab != Tab::Processes {
            return;
        }
        let Some(row) = self.visible().get(self.selected).cloned() else {
            return;
        };
        if let Some(reason) = kill_blocked_reason(row.pid, own_pid) {
            self.status = Some(reason.to_string());
            self.status_kind = StatusKind::Warn;
            return;
        }
        self.pending_kill = Some(row.pid);
        self.pending_name = Some(row.name);
        self.pending_start = Some(row.start_secs);
        self.mode = Mode::ConfirmKill;
    }

    /// kill の結果をステータス行に反映する。
    pub fn set_kill_result(&mut self, pid: u32, outcome: KillOutcome) {
        let (text, kind) = match outcome {
            KillOutcome::Killed => (format!("killed PID {pid}"), StatusKind::Ok),
            KillOutcome::Replaced => {
                (format!("PID {pid} is now another process; kill canceled"), StatusKind::Warn)
            }
            KillOutcome::NotFound => (format!("PID {pid} not found"), StatusKind::Warn),
            KillOutcome::Failed => {
                (format!("failed to kill PID {pid} (permission?)"), StatusKind::Err)
            }
        };
        self.status = Some(text);
        self.status_kind = kind;
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
    use crate::metrics::{KillOutcome, Metrics, ProcessRow};
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
    }

    fn row(pid: u32, name: &str, cpu: f32) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.to_string(),
            cpu,
            memory: 0,
            start_secs: 0,
            disk_read_bps: 0,
            disk_write_bps: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
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
        assert_eq!(app.visible().first().map(|r| r.pid), Some(1));
    }

    #[test]
    fn filter_zero_matches_yields_no_selection() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        app.filter = "zzz".to_string();
        assert!(app.visible().is_empty());
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
        assert_eq!(app.on_key(key(KeyCode::Char('/'))), None);
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
    fn kill_request_on_protected_pid_shows_status_without_prompt() {
        let mut app = app_with_processes(&[("System", 4, 0.0)]);
        app.tab = Tab::Processes;
        app.request_kill(Some(999));
        assert_eq!(app.pending_kill, None);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status.is_some());
    }

    #[test]
    fn kill_request_then_confirm_returns_kill_action() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        app.tab = Tab::Processes;
        app.request_kill(Some(999));
        assert_eq!(app.pending_kill, Some(1234));
        assert_eq!(app.mode, Mode::ConfirmKill);
        assert_eq!(app.on_key(key(KeyCode::Char('n'))), None);
        assert_eq!(app.pending_kill, None);
        assert_eq!(app.mode, Mode::Normal);
        app.request_kill(Some(999));
        assert_eq!(app.on_key(key(KeyCode::Char('y'))), Some((1234, 0)));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.pending_kill, None);
    }

    #[test]
    fn quit_key_is_ignored_while_confirming() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        app.tab = Tab::Processes;
        app.request_kill(Some(999));
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), None);
        assert!(!app.should_quit);
    }

    #[test]
    fn kill_result_updates_status() {
        let mut app = App::new();
        app.set_kill_result(1234, KillOutcome::Killed);
        assert_eq!(app.status.as_deref(), Some("killed PID 1234"));
        app.set_kill_result(1234, KillOutcome::NotFound);
        assert_eq!(app.status.as_deref(), Some("PID 1234 not found"));
        app.set_kill_result(1234, KillOutcome::Failed);
        assert_eq!(app.status.as_deref(), Some("failed to kill PID 1234 (permission?)"));
        assert_eq!(app.status_kind, StatusKind::Err);
        app.set_kill_result(1234, KillOutcome::Replaced);
        assert_eq!(app.status.as_deref(), Some("PID 1234 is now another process; kill canceled"));
        assert_eq!(app.status_kind, StatusKind::Warn);
    }

    #[test]
    fn filtering_clamps_selection_immediately() {
        let mut app = app_with_processes(&[("a", 1, 1.0), ("b", 2, 2.0), ("c", 3, 3.0)]);
        app.selected = 2;
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('a')));
        assert!(
            app.selected < app.visible().len(),
            "selected {} is out of {} rows",
            app.selected,
            app.visible().len()
        );
    }

    #[test]
    fn kill_request_is_ignored_outside_processes_tab() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        assert_eq!(app.tab, Tab::Performance);
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.pending_kill, None);
        assert_eq!(app.pending_start, None);
    }

    #[test]
    fn navigation_and_quit_actions() {
        let mut app = app_with_processes(&[("a", 1, 1.0), ("b", 2, 2.0)]);
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1); // 末端で止まる
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), None);
        assert!(app.should_quit);
    }

    #[test]
    fn tab_hit_test_matches_the_rendered_titles() {
        assert_eq!(Tab::hit(0), Some(Tab::Performance));
        assert_eq!(Tab::hit(12), Some(Tab::Performance));
        assert_eq!(Tab::hit(13), Some(Tab::Performance)); // 見出しの間の区切りは左のタブ
        assert_eq!(Tab::hit(14), Some(Tab::Processes));
        assert_eq!(Tab::hit(24), Some(Tab::Processes));
        assert_eq!(Tab::hit(25), None); // 見出しの右側は無効
    }

    #[test]
    fn click_on_a_tab_header_switches_tabs() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 0));
        assert_eq!(app.tab, Tab::Processes);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 0));
        assert_eq!(app.tab, Tab::Performance);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 3));
        assert_eq!(app.tab, Tab::Performance, "ヘッダ行以外のクリックは無視する");
    }

    #[test]
    fn wheel_scrolls_the_process_selection() {
        let mut app = app_with_processes(&[
            ("a", 1, 5.0),
            ("b", 2, 4.0),
            ("c", 3, 3.0),
            ("d", 4, 2.0),
            ("e", 5, 1.0),
        ]);
        app.tab = Tab::Processes;
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        assert_eq!(app.selected, 1, "1 ノッチ = 1 行");
        for _ in 0..10 {
            app.on_mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        }
        assert_eq!(app.selected, 4, "末尾で止まる");
        app.on_mouse(mouse(MouseEventKind::ScrollUp, 0, 5));
        assert_eq!(app.selected, 3);
    }

    /// ホイールでハイライトを送った行が、そのまま x（kill）の対象になる。
    #[test]
    fn kill_targets_the_row_highlighted_by_scrolling() {
        // PID 0/4 は保護対象なので、テストでは使わない。
        let mut app = app_with_processes(&[
            ("a", 10, 5.0),
            ("b", 11, 4.0),
            ("c", 12, 3.0),
            ("d", 13, 2.0),
            ("e", 14, 1.0),
        ]);
        app.tab = Tab::Processes;
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        let highlighted = app.visible()[app.selected].pid;
        assert_eq!(app.selected, 1);

        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.mode, Mode::ConfirmKill, "確認プロンプトが出ていない");
        assert_eq!(app.pending_kill, Some(highlighted), "ハイライト行以外を kill しようとした");
    }

    /// 表の行のクリックが、その行（`rows_offset` を足した行）を選ぶ。
    #[test]
    fn click_on_a_table_row_selects_that_row() {
        let mut app = app_with_processes(&[
            ("a", 1, 5.0),
            ("b", 2, 4.0),
            ("c", 3, 3.0),
            ("d", 4, 2.0),
        ]);
        app.tab = Tab::Processes;
        app.rows_area = Rect::new(0, 1, 80, 12);
        // 枠 1 行 + 見出し 1 行の下（y = 3）が最初のデータ行。
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
        assert_eq!(app.selected, 1);

        // 見出し行・表の外は選択を変えない。
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 2));
        assert_eq!(app.selected, 1, "見出し行で選択が動いた");
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 40));
        assert_eq!(app.selected, 1, "表の外で選択が動いた");

        // スクロール中は、そのとき見えている行を選ぶ。
        app.rows_offset = 2;
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
        assert_eq!(app.selected, 2);
    }

    /// クリックはモーダルを閉じる（見えない対象への確認が残らないように）。
    #[test]
    fn click_on_a_row_cancels_a_pending_kill() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        app.tab = Tab::Processes;
        app.rows_area = Rect::new(0, 1, 80, 12);
        app.request_kill(Some(999));
        assert_eq!(app.mode, Mode::ConfirmKill);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 3));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.pending_kill, None);
    }

    #[test]
    fn wheel_is_ignored_on_the_performance_tab() {
        let mut app = app_with_processes(&[("a", 1, 5.0), ("b", 2, 4.0)]);
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn clicking_a_tab_cancels_a_pending_kill() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        app.tab = Tab::Processes;
        app.request_kill(Some(999));
        assert_eq!(app.mode, Mode::ConfirmKill);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 0));
        assert_eq!(app.tab, Tab::Performance);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.pending_kill, None);
    }
}
