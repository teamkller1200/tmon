# Windows タスクマネージャー相当機能 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 既存の GPU モニタ TUI に、Windows タスクマネージャー相当のパフォーマンスタブ（CPU/メモリ/ディスク/ネットワーク/GPU の推移グラフ）とプロセスタブ（ソート・検索・kill）を追加する。

**Architecture:** `src/main.rs` の単一ファイルを `main` / `app`（UI 状態・入力）/ `metrics`（sysinfo 計測）/ `gpu`（nvidia-smi）/ `format`（表示整形）/ `ui`（描画のみ）に分割し、1 秒 tick で `Sampler::sample()` の結果を `App` に渡して描画する。`app` と `metrics` の主要ロジックは純関数として切り出し、`ui` は `TestBackend` の文字列スナップショットで検証する。

**Tech Stack:** Rust 2024 / ratatui 0.29 / crossterm 0.28 / sysinfo 0.39（新規・唯一の追加依存）

**Spec:** `docs/superpowers/specs/2026-10-18-task-manager-parity-design.md`

## Global Constraints

- rustc / cargo 1.99.1、edition 2024。`sysinfo` 0.39.6 の MSRV は 1.95 なので満たす。
- 追加依存は `sysinfo = "0.39"` の 1 つだけ。default features（component, disk, network, system, user）を使い、features を絞らない。他のクレートを追加しない。
- `unsafe` を書かない。テストは標準の `#[test]` のみ。外部テストクレート・fixture クレートを使わない。検証コマンドは `cargo test`。
- UI 文言は英語（既存の `GPU Monitor` / `GPU Load` / `VRAM` / `TEMP` / `PWR` / `FAN` に合わせる）。コメントは日本語可。
- 計測は UI スレッドで 1 秒に 1 回のみ（`nvidia-smi` も 1 tick 1 回）。
- 保護 PID: 0 と 4、および自分自身は kill 対象にしない。
- プロセス単位のネットワーク I/O は実装しない（spec 2.2）。
- 文字列の切り詰めに `&s[..n]` を使わない（非 ASCII で panic する）。`chars()` 単位で扱う。
- `cargo add sysinfo` / 初回 `cargo build` はレジストリ取得のため DSH サンドボックス外（承認エスカレーション）で実行する。

## Review Focus

spec の §10.1 より。いずれもテストが無いと壊れる入力なので、各行にテストを割り当てる。

1. 極小ターミナル（20x5 等）: `Layout` が 0 高の矩形を返しても panic せず描画が完了すること → Task 3 `snapshot_at_tiny_size_does_not_panic` / Task 4 `processes_snapshot_at_tiny_size_does_not_panic`
2. プロセス数の増減: 一覧が縮んだ後に `selected` が範囲外にならないこと → Task 4 `selection_clamped_when_processes_shrink`
3. フィルタ 0 件一致: 空テーブルで選択が `None` になり panic しないこと → Task 4 `filter_zero_matches_yields_no_selection`
4. 非 ASCII プロセス名（日本語・絵文字）: 文字境界で panic しないこと → Task 4 `non_ascii_process_name_does_not_panic`
5. レート計算の異常入力: 経過 0 秒・カウンタ巻き戻りで 0 除算や underflow を起こさないこと → Task 2 `rate_per_sec_handles_zero_elapsed`

---

### Task 1: モジュール分割・端末復元 RAII・nvidia-smi 1 回化（挙動不変）

**Files:**
- Create: `src/format.rs`, `src/gpu.rs`, `src/app.rs`, `src/ui.rs`, `.gitignore`
- Modify: `src/main.rs`（293 行をモジュール宣言とループだけに縮める）
- Test: 各モジュールの `#[cfg(test)] mod tests`（既存テスト 3 件を移設）

**Interfaces:**
- Consumes: なし（既存コードの移設のみ）
- Produces:
  - `gpu::GpuStats { gpu_util: u32, memory_used_mb: u32, memory_total_mb: u32, temperature_c: u32, power_w: f64, power_limit_w: f64, fan_speed_pct: u32 }`（`Clone, Debug`）
  - `gpu::read_gpu_stats() -> Option<GpuStats>`, `gpu::power_percent(&GpuStats) -> u32`, `gpu::gpu_util_string(u32) -> String`, `gpu::vram_string(u32, u32) -> String`
  - `format::bar_string(percent: u32, width: usize) -> String`
  - `app::App { pub gpu: Option<GpuStats>, pub frame_count: u64, pub should_quit: bool }`, `App::new() -> Self`, `App::on_key(KeyEvent) -> ()`
  - `ui::draw(f: &mut ratatui::Frame<'_>, app: &App)`, `ui::snapshot(app: &App, width: u16, height: u16) -> String`
  - `main::TerminalGuard`（`Drop` で `disable_raw_mode()` と `LeaveAlternateScreen`）

- [ ] **Step 1: git リポジトリを初期化する**

`git log` は `fatal: not a git repository` を返す（このプロジェクトは未管理）。タスク単位のコミットを取るため、まず初期化する。

```bash
cd C:/Users/syu12/programming/tui-gpumonitor
printf '/target\n' > .gitignore
git init
git add -A
git commit -m "chore: initial import of tui-gpumonitor"
```

- [ ] **Step 2: `src/format.rs` を作り `bar_string` を移設する**

`src/main.rs:74-79` の `bar_string` をそのまま移す。`src/main.rs` に `mod format;` を追加し、呼び出しを `format::bar_string` に置換する。既存テスト `bar_string_scales_correctly`（`src/main.rs:225-231`）も `format.rs` の `#[cfg(test)] mod tests` へ移す。

Run: `cargo test bar_string_scales_correctly -v`
Expected: PASS（`format::tests::bar_string_scales_correctly`）

- [ ] **Step 3: `src/gpu.rs` を作り GPU 計測を移設する**

`GpuStats` / `read_gpu_stats` / `power_percent` / `gpu_util_string` / `vram_string`（`src/main.rs:20-89`）とテスト `power_percent_is_normalized_to_limit`（`src/main.rs:232-246`）を移す。ロジックは変更しない。

Run: `cargo test power_percent_is_normalized_to_limit -v`
Expected: PASS（`gpu::tests::power_percent_is_normalized_to_limit`）

- [ ] **Step 4: `src/app.rs` を作る（この時点では GPU 表示と `q` だけ）**

```rust
use crate::gpu::GpuStats;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

pub struct App {
    pub gpu: Option<GpuStats>,
    pub frame_count: u64,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self { gpu: None, frame_count: 0, should_quit: false }
    }

    /// 1 秒 tick ごとに呼ぶ。この時点では GPU 値の差し替えとフレーム数の加算のみ。
    pub fn on_tick(&mut self, gpu: Option<GpuStats>) {
        self.gpu = gpu;
        self.frame_count += 1;
    }

    /// 押下キーを状態に反映する。`KeyEventKind::Press` 以外は無視する。
    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press { return; }
        if key.code == KeyCode::Char('q') { self.should_quit = true; }
    }
}
```

- [ ] **Step 5: `src/ui.rs` を作り描画を移設する**

`render_dashboard`（`src/main.rs:91-156`）を `ui.rs` の private 関数として移す。`frame_count: usize` 引数は `app.frame_count` から取るため削除する。`ui::draw(f, app)` は次の構成にする。

```rust
pub fn draw(f: &mut ratatui::Frame<'_>, app: &App) {
    let area = f.area();
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(3),
            ratatui::layout::Constraint::Min(0),
            ratatui::layout::Constraint::Length(7),
        ])
        .split(area);
    render_dashboard(f, chunks[1], app.gpu.clone(), app.frame_count as usize);
    // 既存の GPU Load ゲージ（Constraint::Length(7) のチャンク）を chunks[2] に描く
}
```

`render_snapshot`（`src/main.rs:158-192`）を次の公開関数に置き換える。**仕様変更**: 空行を捨てず、常に `height` 行を返す（行位置を検査できるようにする）。

```rust
/// TestBackend で 1 フレーム描画し、各行末尾の空白を trim した文字列を返す。行数は必ず height。
pub fn snapshot(app: &App, width: u16, height: u16) -> String;
```

既存テスト `dashboard_respects_terminal_size`（`src/main.rs:194-223`）を `ui.rs` に移し、`snapshot(&app, w, h)` を呼ぶ形に書き換える（`GpuStats` のリテラルはそのまま、`render_dashboard` に渡していた `Some(stats.clone())` は `App { gpu: Some(stats.clone()), ..Default::default() }` 相当にする）。

```rust
#[test]
fn snapshot_has_exact_height() {
    let app = App::new();
    let shots = snapshot(&app, 40, 12);
    assert_eq!(shots.lines().count(), 12);
}
```

- [ ] **Step 6: `src/main.rs` を `TerminalGuard` とイベントループだけにする**

```rust
mod app; mod format; mod gpu; mod ui;

use std::{io::{self, Stdout}, time::{Duration, Instant}};
use crossterm::{
    event::{self, Event},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{backend::CrosstermBackend, Terminal};

pub struct TerminalGuard { terminal: Terminal<CrosstermBackend<Stdout>> }

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        Ok(Self { terminal: Terminal::new(CrosstermBackend::new(stdout))? })
    }
    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> { &mut self.terminal }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
    }
}
```

ループは次にする。`read_gpu_stats()` は draw クロージャの外で **1 tick に 1 回だけ**呼ぶ（現在の 2 重呼び出し `src/main.rs:259` と `src/main.rs:266` を解消する）。`last.elapsed() >= 1s` のたびに `app.on_tick(gpu)` を呼ぶ。

```rust
let mut guard = TerminalGuard::new()?;
let mut app = app::App::new();
let mut last = Instant::now();
while !app.should_quit {
    if last.elapsed() >= Duration::from_secs(1) {
        app.on_tick(gpu::read_gpu_stats());
        last = Instant::now();
    }
    guard.terminal().draw(|f| ui::draw(f, &app))?;
    if event::poll(Duration::from_millis(50))? {
        if let Event::Key(key) = event::read()? { app.on_key(key); }
    }
}
```

- [ ] **Step 7: テストとビルドを通す**

Run: `cargo test`
Expected: コンパイル成功。`format::tests::bar_string_scales_correctly`, `gpu::tests::power_percent_is_normalized_to_limit`, `ui::tests::dashboard_respects_terminal_size`, `ui::tests::snapshot_has_exact_height` の 4 件が PASS。

Run: `cargo clippy -- -D warnings`（未使用 import の検出）
Expected: warning なし。root に残った未使用 `use` を削除する。

- [ ] **Step 8: 手動確認**

Run: `cargo run`
Expected: 分割前と同じ GPU ダッシュボードが表示され、`q` で終了する。終了後に `echo test` が正常表示される（raw mode が残っていない）。`Ctrl+C` で中断しても同様に端末が壊れない（`TerminalGuard::drop` が走る）。

- [ ] **Step 9: コミット**

```bash
git add -A && git commit -m "refactor: split main.rs into modules and guard terminal restore"
```

---

### Task 2: sysinfo 導入と計測層（`metrics.rs`）

**Files:**
- Modify: `Cargo.toml`（`sysinfo = "0.39"` を追加）, `src/main.rs`（`mod metrics;`）
- Create: `src/metrics.rs`
- Test: `src/metrics.rs` の `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `gpu::read_gpu_stats()`, `gpu::GpuStats`
- Produces:
  - `metrics::Metrics { cpu_total: f32, per_core: Vec<f32>, mem_used: u64, mem_total: u64, swap_used: u64, swap_total: u64, disk_read_bps: u64, disk_write_bps: u64, net_rx_bps: u64, net_tx_bps: u64, gpu: Option<GpuStats>, processes: Vec<ProcessRow> }`（`Clone, Debug, Default`）
  - `metrics::ProcessRow { pid: u32, name: String, cpu: f32, memory: u64, disk_read_bps: u64, disk_write_bps: u64 }`（`Clone, Debug`）
  - `metrics::KillOutcome { Killed, NotFound, Failed }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `metrics::Sampler::new() -> Self`, `Sampler::sample(&mut self) -> Metrics`, `Sampler::kill(&mut self, pid: u32) -> KillOutcome`
  - `metrics::rate_per_sec(delta_bytes: u64, elapsed_secs: f64) -> u64`
  - `metrics::is_loopback_name(name: &str) -> bool`

- [ ] **Step 1: sysinfo を追加してビルドする**

```bash
cd C:/Users/syu12/programming/tui-gpumonitor
cargo add sysinfo@0.39
cargo build
```

`cargo add` はネットワーク必須（サンドボックス内では `SEC_E_NO_CREDENTIALS` で失敗するため、承認付きでサンドボックス外実行する）。完了後 `Cargo.toml` に `sysinfo = "0.39"` があることを確認し、`Cargo.lock` をコミット対象に含める。

Run: `cargo doc -p sysinfo --no-deps` は不要。`cargo build` が成功すればよい。

- [ ] **Step 2: 失敗するテストを書く（純関数）**

`src/metrics.rs` に `#[cfg(test)] mod tests` を書き、`mod metrics;` を `src/main.rs` に追加する。この時点ではコンパイルエラー（未定義）で FAIL する。

```rust
#[test]
fn rate_per_sec_handles_zero_elapsed() {
    assert_eq!(rate_per_sec(2000, 2.0), 1000);
    assert_eq!(rate_per_sec(100, 0.0), 0);
    assert_eq!(rate_per_sec(100, -1.0), 0);
    assert_eq!(rate_per_sec(3, 2.0), 1);   // 切り捨て
}

#[test]
fn is_loopback_name_matches_case_insensitively() {
    assert!(is_loopback_name("Loopback Pseudo-Interface 1"));
    assert!(is_loopback_name("loopback"));
    assert!(is_loopback_name("lo"));
    assert!(!is_loopback_name("Ethernet"));
    assert!(!is_loopback_name("vEthernet (WSL)"));
}
```

Run: `cargo test metrics:: -v`
Expected: FAIL（`cannot find function rate_per_sec` など）

- [ ] **Step 3: 純関数と `Sampler` を実装する**

```rust
pub fn rate_per_sec(delta_bytes: u64, elapsed_secs: f64) -> u64 {
    if elapsed_secs <= 0.0 { return 0; }
    (delta_bytes as f64 / elapsed_secs) as u64
}

pub fn is_loopback_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("loopback") || lower == "lo"
}
```

`Sampler::new()` は `System::new_all()` / `Disks::new_with_refreshed_list()` / `Networks::new_with_refreshed_list()` の後、`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` だけ `std::thread::sleep` してから `refresh_all()` を呼ぶ（CPU% は 2 回の refresh の差分なので、これが無いと最初の値が 0 になる）。

`Sampler::sample()` の手順:

1. `let elapsed = self.prev_at.elapsed().as_secs_f64().max(0.0);`
2. `self.sys.refresh_cpu_all(); self.sys.refresh_memory(); self.sys.refresh_processes(ProcessesToUpdate::All, true);`
3. `self.disks.refresh(true); self.networks.refresh(true);`
4. ディスク合計 `self.disks.list().iter().map(|d| d.usage().total_read_bytes).sum::<u64>()`（書き込みも同様）を取り、`rate_per_sec(total.saturating_sub(prev), elapsed)`。次回のため `prev` を更新する。
5. ネットワークは `self.networks.list().iter().filter(|(name, _)| !is_loopback_name(name))` で `total_received()` / `total_transmitted()` を合計し、同じくレート化する。
6. CPU: `sys.global_cpu_usage()` を `cpu_total`、`sys.cpus().iter().map(|c| c.cpu_usage()).collect()` を `per_core` に入れる（`sysinfo` のドキュメント原文は "Returns \"global\" CPUs usage (aka the addition of all the CPUs)." で、これはマルチコアでは 100 を超える加算値なので `sys.cpus().len()` で割って 0..100 に正規化する）。
7. メモリ: `sys.used_memory()`, `sys.total_memory()`, `sys.used_swap()`, `sys.total_swap()`。
8. プロセス: 各 `(pid, p)` について `ProcessRow { pid: pid.as_u32(), name: p.name().to_string_lossy().into_owned(), cpu: p.cpu_usage() / cpu_count, memory: p.memory(), disk_read_bps: rate_per_sec(p.disk_usage().read_bytes, elapsed), disk_write_bps: rate_per_sec(p.disk_usage().written_bytes, elapsed) }`（`cpu_count = sys.cpus().len().max(1) as f32`）。
9. `gpu: crate::gpu::read_gpu_stats()` を 1 回だけ呼ぶ。
10. `self.prev_at = Instant::now();`

`Sampler::kill(pid)` は `Pid::from_u32(pid)` で `sys.process()` を引き、見つからなければ `refresh_processes(ProcessesToUpdate::Some(&[pid]), true)` で 1 回だけ再取得してから判定する。`Some(p)` かつ `p.kill()` が `true` なら `Killed`、`false` なら `Failed`、`None` なら `NotFound`。

- [ ] **Step 4: スモークテストを追加する**

```rust
#[test]
fn sampler_smoke() {
    let mut s = Sampler::new();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    let m = s.sample();
    assert!((0.0..=100.0).contains(&m.cpu_total), "cpu_total out of range: {}", m.cpu_total);
    assert!(!m.per_core.is_empty());
    assert!(m.per_core.iter().all(|c| *c >= 0.0));
    assert!(m.mem_total > 0);
    assert!(!m.processes.is_empty());
    assert!(m.processes.iter().all(|p| p.cpu >= 0.0));
}
```

Run: `cargo test metrics:: -v`
Expected: `rate_per_sec_handles_zero_elapsed`, `is_loopback_name_matches_case_insensitively`, `sampler_smoke` が PASS。

- [ ] **Step 5: コミット**

```bash
git add -A && git commit -m "feat: add sysinfo-based sampler for cpu, memory, disk and network"
```

---

### Task 3: パフォーマンスタブ（履歴＋スパークライン）

**Files:**
- Modify: `src/app.rs`, `src/ui.rs`, `src/format.rs`, `src/main.rs`
- Test: 各モジュールの `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `metrics::{Sampler, Metrics}`, `format::bar_string`
- Produces:
  - `app::HISTORY_LEN: usize = 60`
  - `app::History { .. }`: `History::new() -> Self`, `History::push(&mut self, v: u64)`, `History::as_vec(&self) -> Vec<u64>`, `History::latest(&self) -> u64`
  - `app::Tab { Performance, Processes }`（`Clone, Copy, PartialEq, Eq, Debug`）
  - `app::App { pub tab: Tab, pub metrics: Metrics, pub hist_cpu: History, pub hist_mem: History, pub hist_disk: History, pub hist_net: History, pub hist_gpu: History, .. }`
  - `App::on_tick(&mut self, metrics: Metrics)`（Task 1 の `on_tick(gpu)` を置き換える）
  - `format::bytes(u64) -> String`, `format::rate(u64) -> String`, `format::percent(f32) -> String`
  - `ui::draw` 内の `performance_tab(f, area, app)`

- [ ] **Step 1: 失敗するテストを書く（`format` の追加関数）**

```rust
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
```

Run: `cargo test format:: -v`
Expected: FAIL（`cannot find function bytes`）

- [ ] **Step 2: `format` の 3 関数を実装する**

`bytes` は 1024 進で `B/KB/MB/GB/TB` を選び、`B` 以外は小数 1 桁（`format!("{:.1} {}", v, unit)`）。`rate` は `format!("{}/s", bytes(bps))`。`percent` は `format!("{:.1}%", v)`。

- [ ] **Step 3: 失敗するテストを書く（`app` の履歴と tick）**

```rust
#[test]
fn history_caps_at_history_len() {
    let mut h = History::new();
    for i in 0..(HISTORY_LEN + 5) { h.push(i as u64); }
    assert_eq!(h.as_vec().len(), HISTORY_LEN);
    assert_eq!(h.latest(), (HISTORY_LEN + 4) as u64);   // 最後に push した値
}

#[test]
fn on_tick_stores_metrics_and_pushes_history() {
    let mut app = App::new();
    let m = Metrics { cpu_total: 12.5, mem_total: 100, mem_used: 50, ..Default::default() };
    app.on_tick(m);
    assert_eq!(app.metrics.cpu_total, 12.5);
    assert_eq!(app.hist_cpu.latest(), 13);            // 12.5 -> round して u64
    assert_eq!(app.hist_mem.latest(), 50);            // 使用率%
    assert_eq!(app.frame_count, 1);
}
```

Run: `cargo test app:: -v`
Expected: FAIL（`no method named on_tick` / `cannot find type History`）

- [ ] **Step 4: `app` に履歴と tick を実装する**

`History` は `VecDeque<u64>` を持ち、`push` で `HISTORY_LEN` を超えたら `pop_front()` する。`on_tick` は `hist_cpu.push(metrics.cpu_total.round() as u64)`、`hist_mem.push(mem_percent)`（`mem_total == 0` なら 0）、`hist_disk.push((disk_read_bps + disk_write_bps) / 1024)`、`hist_net.push((net_rx_bps + net_tx_bps) / 1024)`、`hist_gpu.push(gpu.as_ref().map(|g| g.gpu_util).unwrap_or(0) as u64)` を行い、`self.metrics = metrics; self.frame_count += 1;` する。

- [ ] **Step 5: `ui.rs` にタブヘッダとパフォーマンスタブを実装する**

`draw` の先頭で `Tabs::new(["Performance", "Processes"]).select(app.tab as usize).highlight_style(Style::default().add_modifier(Modifier::REVERSED))` を 1 行高の領域に描く。残り領域は `match app.tab { Tab::Performance => performance_tab(f, area, app), Tab::Processes => performance_tab(f, area, app) }` とする（`Tab::Processes` の腕は Task 4 で `processes_tab(f, area, app)` に差し替える）。

`performance_tab` は `Layout::vertical([Constraint::Length(4), Constraint::Length(4), Constraint::Length(4), Constraint::Length(4), Constraint::Length(4), Constraint::Min(0)])` で、上から CPU / Memory / Disk / Network / GPU を `Constraint::Length(4)`（枠 2 行 + 内容 2 行）、最後の `Cores` を `Constraint::Min(0)` にする。色は spec §6.0 の配色表のアクセント色を使い、各パネルの**枠・タイトル・数値・スパークラインを同じ色**に揃える（CPU/Cores = `Color::LightBlue`、Memory = `Color::LightMagenta`、Disk = `Color::LightGreen`、Network = `Color::LightCyan`、GPU = `Color::LightYellow`）。各パネルは `Block::default().title(コンポーネント名のみ).borders(Borders::ALL)` にし、内側を `Layout::vertical([Constraint::Length(1), Constraint::Length(1)])` で 1 行目（数値）と 2 行目（グラフ）に分ける。スパークラインは必ず `Sparkline::default().data(history.as_vec()).max(m)` の形で、`Vec<u64>` を所有で渡す（`&[u64]` は `Into<SparklineBar>` を満たさずコンパイルできない）。`Cores` は `Paragraph` に改行区切りで入れる。新 UI では使わなくなる `gpu::power_percent` とそのテスト `power_percent_is_normalized_to_limit` はこのステップで削除する（`gpu_util_string` / `vram_string` は継続使用）。

| パネル（タイトル） | 1 行目（数値） | 2 行目（スパークライン） |
|---|---|---|
| `CPU` | `42.1%` + `bar_string(pct, 58)` | 全体使用率% / max 100 |
| `Memory` | `12.0 GB / 31.9 GB (38%)   Swap 1.0 / 8.0 GB` + `bar_string(pct, 30)` | 使用率% / max 100 |
| `Disk` | `R 1.2 MB/s   W 300 KB/s`（`format::rate`） | read+write 合計 / `.max()` を渡さず自動 |
| `Network` | `v 1.2 MB/s   ^ 300 KB/s` | rx+tx 合計 / 自動 |
| `GPU` | `45%` + `bar_string(pct, 28)` + `   VRAM 4.0 / 8.0 GB   TEMP 64C   PWR 150 / 320W   FAN 60%` | 使用率% / max 100。GPU 不在時はこのパネルだけ `GPU unavailable` を表示 |
| `Cores (16)` | コアごとに 1 行 `cpu00 12% ██░░░░░░░`（`bar_string(pct, 9)`） | なし（`Paragraph` の複数行。高さが足りない分は描かない） |

数値は `format::bytes` / `format::rate` / `format::percent` で作る。

同じ 5 パネル分の枠生成をコピーしないよう、`ui.rs` にモジュール内ヘルパー `fn panel_block(title: &str, color: Color) -> Block<'_>`（枠 + タイトルを同色）、`fn spark(history: &History, max: Option<u64>) -> Sparkline<'_>`、`fn metric_panel(f, area, title, color, text: Line<'_>, sparkline: Sparkline<'_>)` を置く。`metric_panel` は `panel_block(..).inner(area)` を `Layout::vertical([Constraint::Length(1), Constraint::Length(1)])` で 2 行に割る。公開は不要（`pub` を付けない）。

GPU 不在時の表示は既存文言 `GPU unavailable` / `NVIDIA-SMI missing or unsupported GPU` をそのまま使う。

- [ ] **Step 6: スナップショットテストを追加する**

```rust
fn app_with_metrics() -> App {
    let mut app = App::new();
    app.on_tick(Metrics {
        cpu_total: 42.0,
        per_core: vec![10.0, 20.0, 30.0],
        mem_used: 12 * 1024 * 1024 * 1024,
        mem_total: 32 * 1024 * 1024 * 1024,
        disk_read_bps: 1024 * 1024,
        disk_write_bps: 300 * 1024,
        net_rx_bps: 512 * 1024,
        net_tx_bps: 2048,
        gpu: Some(sample_gpu()),
        ..Default::default()
    });
    app
}

#[test]
fn performance_tab_shows_every_panel() {
    let wide = snapshot(&app_with_metrics(), 120, 40);
    for label in ["Performance", "Processes", "CPU", "Memory", "Disk", "Network", "GPU", "Cores", "42.0%"] {
        assert!(wide.contains(label), "snapshot missing {label}:" + &wide);
    }
}

#[test]
fn snapshot_at_tiny_size_does_not_panic() {
    let _ = snapshot(&app_with_metrics(), 20, 5);
}

#[test]
fn gpu_panel_falls_back_when_unavailable() {
    let mut app = app_with_metrics();
    app.metrics.gpu = None;
    assert!(snapshot(&app, 120, 40).contains("GPU unavailable"));
}
```

Run: `cargo test ui:: -v`
Expected: 4 件（既存 2 + 新規 3 のうち該当分）が PASS。

- [ ] **Step 7: `main.rs` を計測付きループにする**

`let mut sampler = metrics::Sampler::new();` を作り、1 秒 tick で `app.on_tick(sampler.sample())` を呼ぶ。`Sampler::new()` は約 200ms ブロックするため、ループ開始前に 1 度だけ呼ぶ。

```rust
// ponytail: 計測は UI スレッドで 1Hz 実行。計測が 100ms を超えて体感に響くようなら
// 専用スレッド + mpsc::Receiver<Metrics> に移す（Sampler は Send なので移設のみで済む）
```

- [ ] **Step 8: 検証してコミット**

Run: `cargo test`
Expected: 全 PASS。
Run: `cargo run`
Expected: 1 秒ごとに CPU / Memory / Disk / Network / GPU / Cores の数値とスパークラインが更新され、`q` で終了。CPU 使用率が実際の負荷（例: 別ウィンドウで `cargo build`）に反応する。

```bash
git add -A && git commit -m "feat: add performance tab with per-metric history sparklines"
```

---

### Task 4: プロセスタブ（一覧・ソート・フィルタ）

**Files:**
- Modify: `src/app.rs`, `src/ui.rs`, `src/main.rs`
- Test: `src/app.rs`, `src/ui.rs` の `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `app::{App, History}`, `metrics::ProcessRow`
- Produces:
  - `app::SortKey { Name, Pid, Cpu, Memory, Disk }`（`Clone, Copy, PartialEq, Eq, Debug`）
  - `app::Mode { Normal, Filter, ConfirmKill }`（`Clone, Copy, PartialEq, Eq, Debug`）
  - `app::Action { None, Quit, Kill(u32) }`（`Debug, PartialEq`）
  - `App` 追加フィールド: `pub sort: SortKey`, `pub desc: bool`, `pub selected: usize`, `pub filter: String`, `pub mode: Mode`, `pub status: Option<String>`
  - `App::visible(&self) -> Vec<ProcessRow>`（フィルタ＋ソート済み）, `App::selected_pid(&self) -> Option<u32>`
  - `App::on_key(&mut self, KeyEvent) -> Action`（Task 1 の `()` 戻りを置き換える）
  - `app::filter_matches(name: &str, pid: u32, filter: &str) -> bool`
  - `app::sort_rows(rows: &mut [ProcessRow], key: SortKey, desc: bool)`
  - `ui::draw` 内の `processes_tab(f, area, app)`

- [ ] **Step 1: 失敗するテストを書く（純関数）**

```rust
fn row(pid: u32, name: &str, cpu: f32) -> ProcessRow {
    ProcessRow { pid, name: name.to_string(), cpu, memory: 0, disk_read_bps: 0, disk_write_bps: 0 }
}

#[test]
fn filter_matches_name_case_insensitively_and_pid_prefix() {
    assert!(filter_matches("chrome.exe", 1234, "CHROME"));
    assert!(filter_matches("chrome.exe", 1234, "12"));
    assert!(filter_matches("chrome.exe", 1234, "  "));      // 空白のみは全件一致
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
```

Run: `cargo test app:: -v`
Expected: FAIL（`cannot find function filter_matches`）

- [ ] **Step 2: 純関数と `App` のプロセス状態を実装する**

`filter_matches` は `let f = filter.trim().to_ascii_lowercase();` が空なら `true`、そうでなければ `name.to_ascii_lowercase().contains(&f) || pid.to_string().starts_with(&f)`。

`sort_rows` は `rows.sort_by(|a, b| { ... })` で、`Cpu` は `a.cpu.total_cmp(&b.cpu)`、`Memory` / `Disk`（`disk_read_bps + disk_write_bps`）/ `Pid` は数値比較、`Name` は `to_ascii_lowercase()` の比較。`desc` なら `ord.reverse()`。`sort_by` は安定なので同値の元順が保たれる。

`visible()` は `self.metrics.processes.iter().filter(|p| filter_matches(&p.name, p.pid, &self.filter)).cloned().collect::<Vec<_>>()` を作り `sort_rows` する。

```rust
// ponytail: 毎フレーム全行を clone している（数百行 x 1Hz なので実測上問題なし）。
// 数千プロセスで気になったら &[ProcessRow] + インデックス配列に変える。
```

- [ ] **Step 3: 失敗するテストを書く（選択とキー操作）**

```rust
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
    assert!(!app.should_quit);            // Filter モード中の q は終了しない
}

#[test]
fn navigation_and_quit_actions() {
    let mut app = app_with_processes(&[("a", 1, 1.0), ("b", 2, 2.0)]);
    assert_eq!(app.selected, 0);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected, 1);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected, 1);            // 末端で止まる
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::Quit);
    assert!(app.should_quit);
}
```

Run: `cargo test app:: -v`
Expected: FAIL（`no variant named Filter` など）

- [ ] **Step 4: `App::on_key` とモードを実装する**

`on_key` は `KeyEventKind::Press` 以外を無視し、`Mode` ごとに分岐する。

| モード | キー | 動作 |
|---|---|---|
| Normal | `q` | `should_quit = true` にして `Action::Quit` |
| Normal | `Tab` / `h` / `l` / `Left` / `Right` | `tab` を切り替え（`Tab` は次のタブへ巡回） |
| Normal | `j` / `Down` | `selected` を +1（`visible().len().saturating_sub(1)` でクランプ） |
| Normal | `k` / `Up` | `selected` を -1（0 で止まる） |
| Normal | `PageDown` / `PageUp` | `selected` を ±10（同じクランプ） |
| Normal | `s` | `sort` を `Name -> Pid -> Cpu -> Memory -> Disk -> Name` で巡回し、`desc` を `true` に戻す |
| Normal | `r` | `desc` を反転 |
| Normal | `/` | `mode = Filter` |
| Normal | `x` | Task 5 で実装（この時点では `status = Some("kill は未実装".into())` でもよいが、テストから呼ばない） |
| Filter | 文字 | `filter.push(c)` |
| Filter | `Backspace` | `filter.pop()` |
| Filter | `Enter` / `Esc` | `mode = Normal` |

`on_tick` の末尾で `self.selected = self.selected.min(self.visible().len().saturating_sub(1));` を行う（空一覧なら `visible().len()` が 0 になり `selected` は 0 になるが、`selected_pid()` は `visible().get(self.selected)` 経由なので `None` を返す）。

- [ ] **Step 5: `ui.rs` にプロセスタブを実装する**

列は `[Constraint::Min(20), Constraint::Length(7), Constraint::Length(7), Constraint::Length(10), Constraint::Length(11), Constraint::Length(11)]`、ヘッダは `Name / PID / CPU% / MEM / DISK R / DISK W`。セルは `format::percent(p.cpu)`、`format::bytes(p.memory)`、`format::rate(p.disk_read_bps)`、`format::rate(p.disk_write_bps)`。

```rust
let rows = app.visible();
let mut state = ratatui::widgets::TableState::default();
if !rows.is_empty() { state.select(Some(app.selected.min(rows.len() - 1))); }
let table = ratatui::widgets::Table::new(rows.iter().map(|p| ratatui::widgets::Row::new(vec![..])), widths)
    .header(ratatui::widgets::Row::new(vec!["Name", "PID", "CPU%", "MEM", "DISK R", "DISK W"]).style(Style::default().add_modifier(Modifier::BOLD)))
    .block(Block::default().title("Processes").borders(Borders::ALL))
    .row_highlight_style(Style::default().bg(Color::DarkGray));
f.render_stateful_widget(table, area, &mut state);
```

`highlight_symbol` は使わない（列がずれるため）。`TableState` は毎フレーム `app.selected` から作り直す（ratatui が選択行が見えるようオフセットを再計算する）。

フッタ行: 通常時 `Filter: (none)  (/)   x kill  s sort  r reverse  Tab switch  q quit`、`Mode::Filter` なら `Filter: {filter}_`、`status` があればそれを右側に表示する。

- [ ] **Step 6: スナップショットテストを追加する**

```rust
#[test]
fn processes_tab_lists_sorted_rows_with_headers() {
    let mut app = app_with_processes(&[("alpha.exe", 111, 5.0), ("beta.exe", 222, 50.0)]);
    app.tab = Tab::Processes;
    let shot = snapshot(&app, 120, 30);
    for label in ["Name", "PID", "CPU%", "MEM", "DISK R", "DISK W", "alpha.exe", "beta.exe"] {
        assert!(shot.contains(label), "snapshot missing {label}:" + &shot);
    }
    let (b, a) = (shot.find("beta.exe").unwrap(), shot.find("alpha.exe").unwrap());
    assert!(b < a, "CPU 降順なので beta.exe が先に来るべき");
}

#[test]
fn non_ascii_process_name_does_not_panic() {
    let mut app = app_with_processes(&[("日本語プロセス名.exe", 333, 1.0), ("絵文字🎮.exe", 444, 2.0)]);
    app.tab = Tab::Processes;
    let _ = snapshot(&app, 40, 20);
}

#[test]
fn processes_snapshot_at_tiny_size_does_not_panic() {
    let mut app = app_with_processes(&[("a", 1, 1.0)]);
    app.tab = Tab::Processes;
    let _ = snapshot(&app, 20, 5);
}
```

Run: `cargo test ui:: -v`
Expected: 全 PASS。

- [ ] **Step 7: `main.rs` で `Action` を処理する**

`if let Some(key) = ... { match app.on_key(key) { Action::None => {}, Action::Quit => {}, Action::Kill(_) => {} /* Task 5 で実装 */ } }` の形にする（`should_quit` は `App` が持つので `Quit` は何もしなくてよい）。

- [ ] **Step 8: 検証してコミット**

Run: `cargo test`
Expected: 全 PASS。
Run: `cargo run`
Expected: `Tab` でプロセスタブへ移動。`s` でソート列が変わりヘッダ順に並ぶ。`r` で逆順。`/` の後に入力した文字で絞り込まれ、`Esc` で確定。`j`/`k` で選択行が移動し、選択行が画面外に出るとスクロールが追従する。

```bash
git add -A && git commit -m "feat: add sortable and filterable process list"
```

---

### Task 5: プロセス終了（kill フロー）

**Files:**
- Modify: `src/metrics.rs`, `src/app.rs`, `src/ui.rs`, `src/main.rs`
- Test: `src/metrics.rs`, `src/app.rs`, `src/ui.rs` の `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `metrics::Sampler`, `app::{App, Mode, Action}`, `App::selected_pid()`
- Produces:
  - `metrics::kill_blocked_reason(pid: u32, own_pid: Option<u32>) -> Option<&'static str>`
  - `metrics::Sampler::kill(&mut self, pid: u32) -> KillOutcome`（Task 2 で定義済み）
  - `App` 追加フィールド: `pub pending_kill: Option<u32>`、`pub pending_name: Option<String>`（確認プロンプトに出すプロセス名）
  - `App::set_kill_result(&mut self, pid: u32, outcome: KillOutcome)`（`status` に結果を反映する）
  - `metrics::own_pid() -> Option<u32>`（`sysinfo::get_current_pid().ok().map(|p| p.as_u32())`）
  - `App::request_kill(&mut self, own_pid: Option<u32>)`（`x` から呼ぶ。保護対象なら `status` に理由を入れて確認を出さない）

- [ ] **Step 1: 失敗するテストを書く（保護判定）**

```rust
#[test]
fn kill_blocked_reason_protects_system_and_self() {
    assert!(kill_blocked_reason(0, Some(999)).is_some());
    assert!(kill_blocked_reason(4, Some(999)).is_some());
    assert!(kill_blocked_reason(999, Some(999)).is_some());
    assert!(kill_blocked_reason(1234, Some(999)).is_none());
    assert!(kill_blocked_reason(1234, None).is_none());
}
```

Run: `cargo test metrics::kill_blocked_reason -v`
Expected: FAIL（`cannot find function kill_blocked_reason`）

- [ ] **Step 2: 実装する**

```rust
pub fn kill_blocked_reason(pid: u32, own_pid: Option<u32>) -> Option<&'static str> {
    match pid {
        0 => Some("PID 0 (System Idle Process) cannot be killed"),
        4 => Some("PID 4 (System) cannot be killed"),
        _ if own_pid == Some(pid) => Some("this monitor process cannot kill itself"),
        _ => None,
    }
}
```

- [ ] **Step 3: 失敗するテストを書く（確認フロー）**

```rust
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
    assert_eq!(app.on_key(key(KeyCode::Char('n'))), Action::None);
    assert_eq!(app.pending_kill, None);
    assert_eq!(app.mode, Mode::Normal);
    app.request_kill(Some(999));
    assert_eq!(app.on_key(key(KeyCode::Char('y'))), Action::Kill(1234));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.pending_kill, None);
}

#[test]
fn quit_key_is_ignored_while_confirming() {
    let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
    app.tab = Tab::Processes;
    app.request_kill(Some(999));
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::None);
    assert!(!app.should_quit);
}
```

Run: `cargo test app::kill -v`
Expected: FAIL（`no method named request_kill`）

- [ ] **Step 4: `App` に kill フローを実装する**

`request_kill(own_pid)` は次の順で書く。

```rust
let Some(pid) = self.selected_pid() else { return; };
let name = self.visible().get(self.selected).map(|r| r.name.clone()).unwrap_or_default();
if let Some(reason) = kill_blocked_reason(pid, own_pid) {
    self.status = Some(reason.to_string());
    return;
}
self.pending_kill = Some(pid);
self.pending_name = Some(name);
self.mode = Mode::ConfirmKill;
```

`on_key` の `Mode::ConfirmKill` 分岐は `y` / `Y` で `let pid = self.pending_kill.take();` とモード復帰を行い `pid.map(Action::Kill).unwrap_or(Action::None)` を返す。それ以外のキーは `pending_kill = None` と `pending_name = None` にして `Mode::Normal` へ戻し `Action::None`（`q` も含む）。

`Normal` モードの `x` は `self.request_kill(crate::metrics::own_pid())` を呼ぶ。`metrics` に `pub fn own_pid() -> Option<u32>` を追加し、`sysinfo::get_current_pid().ok().map(|p| p.as_u32())` を返す（`get_current_pid` は cargo feature `system` で利用可能）。

- [ ] **Step 5: `ui.rs` に確認プロンプトを表示する**

フッタ行の描画で `Mode::ConfirmKill` のとき `format!("Kill PID {} ({})? [y/N]", pid, name)` を黄色（`Color::Yellow`）で表示し、他のキーガイドは隠す。

```rust
#[test]
fn confirm_prompt_is_shown() {
    let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
    app.tab = Tab::Processes;
    app.request_kill(Some(999));
    let shot = snapshot(&app, 120, 30);
    assert!(shot.contains("Kill PID 1234"), "prompt missing:" + &shot);
    assert!(shot.contains("[y/N]"), "prompt missing:" + &shot);
}
```

Run: `cargo test ui:: -v`
Expected: PASS。

- [ ] **Step 6: `main.rs` で `Action::Kill` を実行する**

```rust
match app.on_key(key) {
    app::Action::Kill(pid) => {
        let outcome = sampler.kill(pid);
        app.set_kill_result(pid, outcome);
    }
    _ => {}
}
```

`App::set_kill_result(&mut self, pid: u32, outcome: KillOutcome) -> ()` は `Sampler::kill` の戻り値を `status` 用の文言に変換する（`Killed` / `NotFound` / `Failed` の 3 通り。`Failed` には権限不足が含まれる）。文言は下のテストの `assert_eq!` の文字列に一致させる。

```rust
#[test]
fn kill_result_updates_status() {
    let mut app = App::new();
    app.set_kill_result(1234, KillOutcome::Killed);
    assert_eq!(app.status.as_deref(), Some("killed PID 1234"));
    app.set_kill_result(1234, KillOutcome::NotFound);
    assert_eq!(app.status.as_deref(), Some("PID 1234 not found"));
    app.set_kill_result(1234, KillOutcome::Failed);
    assert_eq!(app.status.as_deref(), Some("failed to kill PID 1234 (permission?)"));
}
```

- [ ] **Step 7: 手動確認（テストからは実プロセスを kill しない）**

Run: `cargo run` の後、`Tab` でプロセスタブへ移動し、`notepad.exe` を起動しておいて選択して `x` → `y`。
Expected: `killed PID <n>` がフッタに表示され、次の tick で一覧から消える。`System` (PID 4) で `x` を押した場合は確認が出ず `PID 4 (System) cannot be killed` が表示される。管理者権限の無いプロセスで `Failed` が出ることも確認する（権限不足の正常系）。

- [ ] **Step 8: コミット**

```bash
git add -A && git commit -m "feat: add guarded process kill with confirmation"
```

---

## 完了条件

1. `cargo test` が全て PASS する。
2. `cargo clippy -- -D warnings` が警告なしで通る。
3. `cargo run` でパフォーマンスタブとプロセスタブが動作し、`q` で終了後に端末が正常状態に戻る。
4. spec の §2.1 の 3 項目（パフォーマンスグラフ / プロセス一覧 / kill）が手動確認できる。
5. 実装しないもの（spec §2.2）を追加していない。
