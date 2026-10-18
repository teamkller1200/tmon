# Windows タスクマネージャー相当機能 設計書

- 日付: 2026-10-18
- 対象: C:\Users\syu12\programming\tui-gpumonitor (Rust/TUI, ratatui + crossterm)
- 実装計画: docs/superpowers/plans/2026-10-18-task-manager-parity.md
- 状態: ユーザーレビュー待ち（実装前）

## 1. 目的

現在の `src/main.rs` は NVIDIA GPU の 1 画面ダッシュボードのみを表示する。本設計は Windows のタスクマネージャーと同等の監視・操作機能（パフォーマンス全体グラフ＋プロセス一覧＋プロセス終了）を、同じ TUI に追加するための仕様を定める。

## 2. スコープ

### 2.1 含む（ユーザー確定済み）

1. パフォーマンスタブ: CPU 全体使用率、メモリ、ディスク（全体 I/O レート）、ネットワーク（全体 送受信レート）、GPU（既存機能）を、それぞれ直近 60 秒の推移（スパークライン）付きで表示する。CPU はコア別の使用率も棒グラフで表示する。
2. プロセスタブ: プロセス一覧（名前、PID、CPU%、メモリ、ディスク読み書きレート）をソート・検索（フィルタ）できるテーブルで表示する。
3. プロセス終了: 選択プロセスを確認プロンプト経由で kill できる。

### 2.2 含まない（YAGNI / 今回のスコープ外）

- サービス / スタートアップ / ユーザー / 詳細タブ（ユーザーが選択しなかった）。
- プロセス単位のネットワーク I/O 列。**重要**: Windows タスクマネージャーにはプロセス毎の Network 列があるが、`sysinfo` はこれを提供せず、実装には ETW 購読が必要（Windows 専用・管理者権限・別スレッド）。今回は対象外とし、代わりにパフォーマンスタブのネットワーク全体値を使う。列に `-` を出すような飾りは入れない。
- サンプリングのバックグラウンドスレッド化、設定ファイル、テーマ、履歴の永続化、プロセスツリー表示、CPU アフィニティ/優先度変更。

## 3. 前提環境と制約

- rustc / cargo 1.99.1、edition 2024。`sysinfo` 0.39.6 の MSRV は 1.95 なので問題ない。
- 追加する依存は `sysinfo = "0.39"` のみ（default features: component, disk, network, system, user）。ratatui 0.29 / crossterm 0.28 は据え置き。他のクレートを追加しない。
- `unsafe` を書かない。テストは標準の `#[test]` のみ（外部テストクレートなし）、`cargo test` で実行する。
- 既存 UI の文言は英語（`GPU Monitor`, `GPU Load`, `VRAM`, `TEMP`, `PWR`, `FAN` など）なので、追加分も英語で統一する。
- PID 4 = System は Windows 固有。PID 0 と併せて保護対象にする（他 OS では単に存在しないだけ）。
- 初回 `cargo add sysinfo` / `cargo build` はレジストリ取得のため DSH サンドボックス外（承認エスカレーション）で実行する必要がある。取得後はローカルキャッシュによりオフラインビルド可能。

## 4. アーキテクチャ

### 4.1 モジュール構成

現在の単一ファイル `src/main.rs`（293 行）を責務で分割する。

| ファイル | 責務 | 主要な公開 API |
|---|---|---|
| `src/main.rs` | 端末初期化と復元の RAII ガード、イベントループ、1 秒 tick、キー入力の配送 | `TerminalGuard`, `fn main()` |
| `src/gpu.rs` | GPU 計測（既存コードの移設）。nvidia-smi 実行 | `GpuStats`, `read_gpu_stats() -> Option<GpuStats>`, `power_percent() -> u32`, `gpu_util_string() -> String`, `vram_string(u32, u32) -> String` |
| `src/metrics.rs` | sysinfo の所有とサンプリング。全体/プロセスの計測値 | `Sampler`, `Metrics`, `ProcessRow`, `KillOutcome`, `rate_per_sec()`, `is_loopback_name()`, `kill_blocked_reason()` |
| `src/app.rs` | UI 状態、履歴リングバッファ、ソート/フィルタ、キー→アクション変換 | `App`, `Tab`, `SortKey`, `Mode`, `Action`, `History`, `HISTORY_LEN`, `filter_matches()`, `sort_rows()` |
| `src/format.rs` | 表示用フォーマット（純関数） | `bar_string()`, `bytes()`, `rate()`, `percent()` |
| `src/ui.rs` | 描画のみ。状態を持たない | `draw(&mut Frame, &App)`, `snapshot(&App, u16, u16) -> String` |

`app.rs` は sysinfo に依存しない（純粋な UI 状態＋入力なのでテストが容易）。`metrics.rs` は ratatui に依存しない。

### 4.2 データフロー（1 秒ごとの tick）

```
loop {
    if last_tick.elapsed() >= 1s {
        let m = sampler.sample();          // sysinfo + nvidia-smi を 1 回だけ
        app.on_tick(m);                    // metrics 差し替え + 履歴 push + 選択位置クランプ
        last_tick = Instant::now();
    }
    terminal.draw(|f| ui::draw(f, &app))?;
    // 50ms ポーリングでキー処理 → Action::None | Quit | Kill(pid)
}
```

計測は UI スレッドで 1 秒に 1 回だけ行う。`nvidia-smi` の 2 重呼び出し（現在は 1 フレームに 2 回、`src/main.rs:259` と `src/main.rs:266`）はこの経路に統合して解消する。
```rust
// ponytail: 計測は UI スレッドで 1Hz 実行。計測が 100ms を超えて体感に響くようなら
// 専用スレッド + mpsc::Receiver<Metrics> に移す（Sampler は Send なので移設のみで済む）
```

### 4.3 端末復元（既存バグの修正）

現在は `q` 押下時にしか `disable_raw_mode()` / `LeaveAlternateScreen` を呼ばないため、エラーや panic で端末が壊れたまま残る。RAII ガードに置き換える。

```rust
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
    }
}
```

## 5. データモデル

### 5.1 metrics.rs

```rust
pub struct Metrics {
    pub cpu_total: f32,          // 0.0..=100.0 (global_cpu_usage)
    pub per_core: Vec<f32>,      // 各 0.0..=100.0
    pub mem_used: u64,           // bytes (total_memory - available_memory)
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disk_read_bps: u64,      // 全ディスク合計の read bytes/sec
    pub disk_write_bps: u64,
    pub net_rx_bps: u64,         // loopback を除く全インタフェース合計 bytes/sec
    pub net_tx_bps: u64,
    pub gpu: Option<GpuStats>,
    pub processes: Vec<ProcessRow>,
}

pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,                // 0.0..=100.0（全コア合計を CPU 数で正規化）
    pub memory: u64,             // bytes
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
}

pub enum KillOutcome { Killed, NotFound, Failed }
```

### 5.2 Sampling（SysinfoSampler の内部方針）

```rust
pub struct Sampler {
    sys: System,
    disks: Disks,
    networks: Networks,
    prev_disk: (u64, u64),   // (total_read_bytes, total_written_bytes)
    prev_net: (u64, u64),    // (total_received, total_transmitted)
    prev_at: Instant,
}
impl Sampler {
    pub fn new() -> Self;
    pub fn sample(&mut self) -> Metrics;
    pub fn kill(&mut self, pid: u32) -> KillOutcome;
}
```

- `Sampler::new()`: `System::new_all()` / `Disks::new_with_refreshed_list()` / `Networks::new_with_refreshed_list()`。CPU% とプロセス CPU% は 2 回の refresh の差分で算出されるため、`MINIMUM_CPU_UPDATE_INTERVAL` だけ `sleep` してから `refresh_all()` で基点を作る（起動時に 1 回だけ、約 200ms）。
- `sample()`:
  - `sys.refresh_cpu_all()`（コア別含む）、`sys.refresh_memory()`、`sys.refresh_processes(ProcessesToUpdate::All, true)`、`disks.refresh(true)`、`networks.refresh(true)`。
  - 経過秒は `prev_at.elapsed().as_secs_f64()`。
  - ディスク/ネットワークは累積値の差（`saturating_sub`）を経過秒で割る。
  - プロセスのディスクは `DiskUsage::read_bytes` / `written_bytes`（前回 refresh からの差分）を経過秒で割る。
  - `gpu::read_gpu_stats()` を 1 回だけ呼ぶ。
- CPU%: `cpu.cpu_usage()` はマルチコアで 100 を超えるため、`sys.cpus().len().max(1)` で割って 0..100 に正規化する（`sysinfo` のドキュメント通り）。
- プロセス名は `p.name().to_string_lossy().into_owned()`。

### 5.3 純関数（テスト対象）

```rust
// metrics.rs
pub fn rate_per_sec(delta_bytes: u64, elapsed_secs: f64) -> u64;
    // elapsed_secs <= 0.0 なら 0 を返す（0 除算/Inf の防止）
pub fn is_loopback_name(name: &str) -> bool;
    // ASCII 小文字化して "loopback" を含む、または "lo" と等しい
pub fn kill_blocked_reason(pid: u32, own_pid: Option<u32>) -> Option<&'static str>;
    // pid == 0 / pid == 4 / own_pid == Some(pid) を拒否（理由文字列を返す）

// app.rs
pub fn filter_matches(name: &str, pid: u32, filter: &str) -> bool;
    // filter を trim + 小文字化。名前の部分一致、または PID の前方一致。空なら常に true
pub fn sort_rows(rows: &mut [ProcessRow], key: SortKey, desc: bool);
    // Name は小文字比較、Cpu は f32::total_cmp。同値は安定ソートで元順を維持

// format.rs
pub fn bar_string(percent: u32, width: usize) -> String;   // 既存実装を移設
pub fn bytes(n: u64) -> String;    // "1.5 GB" など（1024 進、B/KB/MB/GB/TB）
pub fn rate(bps: u64) -> String;   // "1.5 MB/s"
pub fn percent(v: f32) -> String;  // "42.1%"
```

## 6. 画面設計

### 6.0 モックアップと配色（100x30、枠はすべて `Borders::ALL` の実線）

Performance タブ（起動直後・既定。パネルごとのアクセント色は下の配色表を参照）:

```text
 Performance │ Processes                                                   [q] quit  [Tab] switch
┌─ CPU ────────────────────────────────────────────────────────────────────────────────────────────┐
│ 42.1%   ████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░                               │
│  ▄▄▄▄▅▅▅▅▅▅▅▅▅▄▄▄▄▄▄▄▄▄▄▅▅▅▅▅▅▅▅▅▄▄▃▃▃▂▂▁▁▁▁▁▁▁▁▁▂▂▂▂▂▃▃▃▃▃▃▃▃▃▃▄▄▄▄▄▅▆▆▆▇▇▇▇▇▇▇▆▆▆▅▅▅▄▄▄▃▃▃▃▃▄▄▄│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ Memory ─────────────────────────────────────────────────────────────────────────────────────────┐
│ 12.0 GB / 31.9 GB (38%)   Swap 1.0 / 8.0 GB   ███████████░░░░░░░░░░░░░░░░░░░                     │
│  ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ Disk ───────────────────────────────────────────────────────────────────────────────────────────┐
│ R 1.2 MB/s   W 300 KB/s                                                                          │
│  ██▂▂▂▂▂▄▂▂▂▂▂▄▄▂▂▂██▄▂▂▂▂▂▄▄▂▂▂▂▄▄▂▂█▂▂▄▂▂▂▂▂▄▄▂▂▂▂▂▄██▂▂▂▄▄▂▂▂▂▄▄▂▂▂▂▂█▂▂▂▂▂▄▄▂▂▂▂▂▄▂▂▂██▄▄▂▂▂▂│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ Network ────────────────────────────────────────────────────────────────────────────────────────┐
│ v 1.2 MB/s   ^ 300 KB/s                                                                          │
│  ██▃▃▃▃▃▃▄▄▄▄█▃▃▃▃▃▄▄▄▄▄█▃▃▃▃▃▄▄▄▂▂██▃▃▃▃▄▄▄▄▂██▃▃▃▄▄▄▄▄▂██▃▃▃▄▄▄▂▂▃▃█▃▃▃▄▄▄▄▂▃▃█▃▃▄▄▄▄▄▂▃▃██▃▄▄▄│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ GPU ────────────────────────────────────────────────────────────────────────────────────────────┐
│ 45%   █████████████░░░░░░░░░░░░░░░   TEMP 61C   PWR 120 / 320W   FAN 40%                         │
│  ▃▃▄▄▄▅▅▅▆▆▆▆▆▇▇▇▇▇▇▇▇▇▇▇▆▆▆▆▆▅▅▅▄▄▄▄▃▄▄▄▅▅▅▅▆▆▆▆▇▇▇▇▇▇▇▇▇▇▇▇▆▆▆▆▅▅▅▅▄▄▄▃▄▄▄▄▅▅▅▆▆▆▆▆▇▇▇▇▇▇▇▇▇▇▇▆│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ Cores (16) ─────────────────────────────────────────────────────────────────────────────────────┐
│ cpu00 12% ██░░░░░░░                                                                              │
│ cpu01  5% █░░░░░░░░                                                                              │
│ cpu02 78% ███████░░                                                                              │
│ cpu03 43% ████░░░░░                                                                              │
│ cpu04 91% █████████                                                                              │
│ cpu05 22% ██░░░░░░░                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Processes タブ（`s`=CPU 降順が既定。3 行目が選択行で、実際は背景 `Color::DarkGray`。ヘッダ行は `Modifier::BOLD`、テーブル自体に枠は付けない）:

```text
 Performance │ Processes                                                   [q] quit  [Tab] switch
 Name                                               PID   CPU%       MEM     DISK R     DISK W
 chrome.exe                                        1234   42.1    1.5 GB  12.4 MB/s   1.2 MB/s
 Code.exe                                          8877   28.6    980 MB   3.1 MB/s   820 KB/s
 cargo.exe                                         2210   19.2    210 MB    44 KB/s  12.0 MB/s
 WindowsTerminal.exe                               6650    8.4    180 MB      0 B/s      0 B/s
 explorer.exe                                      3312    3.1    120 MB      0 B/s     8 KB/s
 nvim.exe                                          9104    1.8     88 MB      0 B/s      0 B/s
 System                                               4    0.9     24 MB      0 B/s     4 KB/s
 MemCompression                                    1236    0.3    512 MB      0 B/s      0 B/s


















 Filter: (none)  (/)   x kill  s sort  r reverse  Tab switch  q quit
```

kill 確認中（`x` 押下後。最下行が黄色 `Color::Yellow`）:

```text
 Kill PID 1234 (chrome.exe)? [y/N]
```

フィルタ入力中（`/` 押下後。最下行の入力中テキストは `Color::Cyan`）:

```text
 Filter: chrom_
```

配色（これ以外の色は使わない）:

| 対象 | 色 |
|---|---|
| CPU パネル / Cores パネル（枠・タイトル・数値・グラフ） | `Color::LightBlue` |
| Memory パネル（同上） | `Color::LightMagenta` |
| Disk パネル（同上） | `Color::LightGreen` |
| Network パネル（同上） | `Color::LightCyan` |
| GPU パネル（同上） | `Color::LightYellow` |
| 上記以外の文字（タブヘッダ、[`q`] quit など） | 既定（白系） |
| タブの選択中 | `Modifier::REVERSED` |
| プロセス一覧の選択行 | 背景 `Color::DarkGray` |
| プロセステーブルのヘッダ行 | `Modifier::BOLD` |
| kill 確認プロンプト | `Color::Yellow` |
| kill 成功 / 失敗 / 見つからない | `Color::Green` / `Color::Red` / `Color::Yellow` |
| フィルタ入力中テキスト | `Color::Cyan` |
### 6.1 共通

- 1 行目: タブ `Performance | Processes`（`Tabs` ウィジェット、選択中は反転）。右端に `[q] quit  [Tab] switch`。
- 最下行: ステータス行（キーガイド、フィルタ入力、kill 結果メッセージ）。
- ターミナルが小さい場合でも panic しない（`Constraint::Min(0)` と ratatui のクランプに任せる）。

### 6.2 Performance タブ

縦積み。各パネルは枠付き 2 行で、1 行目が数値、2 行目がグラフ（グラフは常にパネルの最下行）。タイトルはコンポーネント名のみとし、数値と重複させない。

| パネル | 1 行目（数値） | 2 行目（スパークライン） |
|---|---|---|
| `CPU` | `42.1%` + `bar_string(42, 58)` | 全体使用率%（max 100） |
| `Memory` | `12.0 GB / 31.9 GB (38%)   Swap 1.0 / 8.0 GB` + `bar_string(38, 30)` | 使用率%（max 100） |
| `Disk` | `R 1.2 MB/s   W 300 KB/s` | read+write 合計（max 自動） |
| `Network` | `v 1.2 MB/s   ^ 300 KB/s` | rx+tx 合計（max 自動） |
| `GPU` | `45%` + `bar_string(45, 28)` + `   TEMP 61C   PWR 120 / 320W   FAN 40%` | 使用率%（max 100） |
| `Cores (16)` | コアごとに 1 行 `cpu00 12% ██░░░░░░░`（`bar_string(pct, 9)`） | なし（高さが足りない分は表示しない。スクロールなし） |

- GPU 不在（nvidia-smi なし）の場合は `GPU unavailable` を表示する（既存挙動を踏襲）。
- スパークラインは `Sparkline::default().data(history.as_vec()).max(m)`。`data` は `Vec<u64>` を渡す（`&[u64]` は `Into<SparklineBar>` を満たさないため不可）。

### 6.3 Processes タブ

- 列: `Name` (Min 20) / `PID` (7) / `CPU%` (7) / `MEM` (10) / `DISK R` (11) / `DISK W` (11)。
- 行の選択は `TableState` + `Table::row_highlight_style(Style::default().bg(Color::DarkGray))`。`highlight_symbol` は使わない（列がずれるため）。`TableState` は描画時に `app.selected` から作る（ratatui が選択行が見えるようスクロール量を再計算する）。
- フィルタ行: 通常時 `Filter: (none)  (/)`、入力中 `Filter: abc_`。
- フッタ: `x kill  s sort  r reverse  / filter  Tab switch  q quit` とステータスメッセージ。

## 7. キーバインド

### 7.1 Normal モード

| キー | 動作 |
|---|---|
| `q` | 終了 |
| `Tab` / `h` / `l` / `Left` / `Right` | タブ切替 |
| `j` / `Down` | 選択を 1 つ下へ |
| `k` / `Up` | 選択を 1 つ上へ |
| `PageDown` / `PageUp` | 選択を 10 移動 |
| `s` | ソート列を巡回（Name -> Pid -> Cpu -> Memory -> Disk -> Name）。切替時は降順にリセット |
| `r` | 昇順/降順を反転 |
| `/` | フィルタ入力モードへ |
| `x` | 選択プロセスの kill 確認へ（保護対象なら理由をステータスに表示） |

初期値: ソート `SortKey::Cpu` 降順（Windows タスクマネージャーと同じ）、選択 0、タブ `Performance`。

### 7.2 Filter モード

| キー | 動作 |
|---|---|
| 文字キー | フィルタ文字列に追加 |
| `Backspace` | 末尾を削除 |
| `Enter` / `Esc` | Normal に戻る（フィルタ文字列は保持） |

### 7.3 ConfirmKill モード

| キー | 動作 |
|---|---|
| `y` / `Y` | kill を実行（`Action::Kill(pid)`）、結果をステータスに表示 |
| その他（`n` / `Esc` 含む） | 中止して Normal に戻る |

確認プロンプトの文言: `Kill PID 1234 (chrome.exe)? [y/N]`。

## 8. kill の安全設計

1. `x` 押下時、まず `kill_blocked_reason(pid, get_current_pid().ok().map(Pid::as_u32))` で拒否判定。拒否なら確認を出さずステータスに理由を表示する（PID 0 / PID 4 / 自分自身）。
2. 確認プロンプトで `y` を押した場合のみ `Sampler::kill(pid)` を呼ぶ。
3. `KillOutcome::{Killed, NotFound, Failed}` をステータスに表示する（`Failed` は権限不足などを含む）。
4. テストから実プロセスを kill しない。判定（1）と結果変換（3）は純関数と `KillOutcome` の値で検証し、実際の kill は手動確認に回す。

## 9. エラー処理

| 事象 | 挙動 |
|---|---|
| panic / `?` による早期 return | `TerminalGuard::drop` が raw mode と代替画面を復元する |
| nvidia-smi 不在・失敗 | `gpu: None` → `GPU unavailable` 表示。アプリは継続 |
| 経過秒 0 / カウンタ巻き戻り | `rate_per_sec` が 0 を返す。`saturating_sub` で underflow させない |
| プロセスが消えた | `sample()` の `ProcessesToUpdate::All, true` で除去。`selected` は `visible().len()-1` にクランプ |
| kill 対象が既に存在しない | `KillOutcome::NotFound` をステータス表示 |
| プロセス一覧が空 | テーブルはヘッダのみ。選択は `None` 扱い |

## 10. テスト方針

- 各モジュールに `#[test]` を置く（インライン `#[cfg(test)] mod tests`）。
- 純関数（`rate_per_sec`, `is_loopback_name`, `kill_blocked_reason`, `filter_matches`, `sort_rows`, `bar_string`, `bytes`, `rate`）は境界値を含めて単体テストする。
- `App` の履歴リングは `HISTORY_LEN` で上限になることを検証する。
- UI は `ui::snapshot(&app, width, height)`（`TestBackend`）で文字列スナップショットを作り、固定データに対する期待部分文字列を検証する。`snapshot` は `height` 行を必ず返す（末尾空白のみ trim。空行も保持し、行位置を検査できるようにする）。
- `Sampler` は実環境の 1 回のスモークテスト（`Sampler::new()` 後に 2 回サンプルし、`cpu_total` が 0..=100、`per_core` が空でない、`processes` が空でない）だけを行う。

### 10.1 Review Focus（仕様が暗黙に前提し、バグが出やすい入力）

1. **極小ターミナル**（例 20x5）: `Layout` が 0 高の矩形を返しても panic せず描画が終わること。
2. **プロセス数の増減**: 一覧が縮んだ後に `selected` が範囲外にならず、フッタとテーブルの整合が保たれること。
3. **フィルタが 0 件一致**: 空テーブルでも選択が `None` になり panic しないこと。
4. **非 ASCII プロセス名**（日本語・絵文字）: 文字境界で slice せず、切り詰めは `chars()` 単位であること（`&s[..n]` 禁止）。
5. **レート計算の異常入力**: 経過 0 秒・カウンタ巻き戻りで 0 除算や underflow を起こさないこと。

## 11. 既知の制限

- プロセス単位のネットワーク I/O は非対応（5.1 / 2.2 参照）。
- ディスクはボリューム単位の合計であり、物理ディスク別・プロセス別の内訳は出さない。
- サンプリング周期は 1 秒固定。Windows タスクマネージャーより粗いため、短時間のスパイクは取り逃す。
- `nvidia-smi` を 1 秒ごとに起動するため、プロセス起動コスト（数 ms〜数十 ms）が常時発生する。
- CPU% の正規化は「全コア合計 / コア数」であり、タスクマネージャーの「CPU 時間 / 経過時間」とは計算式が異なるため厳密一致はしない。
