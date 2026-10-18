mod app;
mod format;
mod gpu;
mod metrics;
mod ui;

use std::{
    io::{self, Stdout},
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEventKind,
    },
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{backend::CrosstermBackend, Terminal};

/// raw mode と代替スクリーンを確実に戻すための RAII ガード。
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        // raw mode に入った後の失敗は Drop が走らないので、ここで復元してから返す。
        Self::setup().inspect_err(|_| Self::restore())
    }

    fn setup() -> io::Result<Self> {
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        // マウスホイールとタブのクリックを受け取る。副作用としてターミナル自身の
        // テキスト選択は Shift 併用になる。復元は restore() が必ず行う。
        stdout.execute(EnableMouseCapture)?;
        Ok(Self { terminal: Terminal::new(CrosstermBackend::new(stdout))? })
    }

    fn restore() {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(DisableMouseCapture);
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }

    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        Self::restore();
    }
}

/// 最大 `budget` の間キー入力を待ち、1 つだけ処理する。kill 要求が出たら
/// (PID, 開始時刻) を返す。戻るたびに呼び出し側が再描画するので、x の直後に y を
/// 押しても確認プロンプトが描かれないまま kill されることはない。
/// 終了要求は `App::should_quit` に出るので呼び出し側のループ条件が拾う。
fn pump_keys(app: &mut app::App, budget: Duration) -> io::Result<Option<(u32, u64)>> {
    let deadline = Instant::now() + budget;
    loop {
        let now = Instant::now();
        if now >= deadline || !event::poll(deadline - now)? {
            return Ok(None);
        }
        let event = event::read()?;
        // EnableMouseCapture はマウス移動も報告する（1003h）。状態を変えないイベントで
        // 毎回描き直さないよう読み捨て、期限まで次の入力を待つ。
        let actionable = match &event {
            Event::Key(_) => true,
            Event::Mouse(mouse) => matches!(
                mouse.kind,
                MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::Down(MouseButton::Left)
            ),
            _ => false,
        };
        if !actionable {
            continue;
        }
        let action = match event {
            Event::Key(key) => app.on_key(key),
            Event::Mouse(mouse) => app.on_mouse(mouse),
            _ => app::Action::None,
        };
        return match action {
            app::Action::Kill(pid, start) => Ok(Some((pid, start))),
            _ => Ok(None),
        };
    }
}

fn main() -> io::Result<()> {
    let mut guard = TerminalGuard::new()?;
    // Sampler::new() は CPU 使用率の基準を取るため約 200ms ブロックする（ループ開始前に 1 度だけ）。
    // ponytail: 計測は UI スレッドで 1Hz 実行。計測が 100ms を超えて体感に響くようなら
    // 専用スレッド + mpsc::Receiver<Metrics> に移す（Sampler は Send なので移設のみで済む）
    let mut sampler = metrics::Sampler::new();
    let mut app = app::App::new();
    let tick = Duration::from_secs(1);
    let mut next_sample = Instant::now();

    while !app.should_quit {
        let now = Instant::now();
        if now >= next_sample {
            // nvidia-smi の起動も 1 tick につき 1 回だけ。
            app.on_tick(sampler.sample());
            next_sample = now + tick;
        }

        guard.terminal().draw(|f| ui::draw(f, &app))?;

        // 次の tick まで入力待ち。1 キー処理ごとにループ先頭へ戻って必ず再描画する。
        let wait = next_sample.saturating_duration_since(now).max(Duration::from_millis(10));
        if let Some((pid, start)) = pump_keys(&mut app, wait)? {
            app.set_kill_result(pid, sampler.kill(pid, start));
        }
    }

    Ok(())
}
