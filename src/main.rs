mod app;
mod format;
mod gpu;
mod metrics;
mod ui;

use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    event::{self, Event},
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
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        Ok(Self { terminal: Terminal::new(CrosstermBackend::new(stdout))? })
    }

    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
    }
}

/// キー入力を最大 `budget` の間さばく。kill 要求が出たらその PID を返す（無ければ `None`）。
/// 終了要求は `App::should_quit` に出るので呼び出し側のループ条件が拾う。
fn pump_keys(app: &mut app::App, budget: Duration) -> io::Result<Option<u32>> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if app.should_quit {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        if event::poll(remaining.min(Duration::from_millis(50)))?
            && let Event::Key(key) = event::read()?
            && let app::Action::Kill(pid) = app.on_key(key)
        {
            return Ok(Some(pid));
        }
    }
}

fn main() -> io::Result<()> {
    let mut guard = TerminalGuard::new()?;
    // Sampler::new() は CPU 使用率の基準を取るため約 200ms ブロックする（ループ開始前に 1 度だけ）。
    // ponytail: 計測は UI スレッドで 1Hz 実行。計測が 100ms を超えて体感に響くようなら
    // 専用スレッド + mpsc::Receiver<Metrics> に移す（Sampler は Send なので移設のみで済む）
    let mut sampler = metrics::Sampler::new();
    let mut app = app::App::new();

    while !app.should_quit {
        // nvidia-smi の起動も 1 tick につき 1 回だけ。
        app.on_tick(sampler.sample());

        guard.terminal().draw(|f| ui::draw(f, &app))?;

        if let Some(pid) = pump_keys(&mut app, Duration::from_secs(1))? {
            app.set_kill_result(pid, sampler.kill(pid));
        }
    }

    Ok(())
}
