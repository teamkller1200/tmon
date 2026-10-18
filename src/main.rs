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

/// キー入力を最大 `budget` の間さばく。`should_quit` が立ったら `false` を返す。
fn pump_keys(app: &mut app::App, budget: Duration) -> io::Result<bool> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if app.should_quit {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(true);
        }
        if event::poll(remaining.min(Duration::from_millis(50)))?
            && let Event::Key(key) = event::read()?
        {
            app.on_key(key);
        }
    }
}

fn main() -> io::Result<()> {
    let mut guard = TerminalGuard::new()?;
    let mut app = app::App::new();

    while !app.should_quit {
        // nvidia-smi は 1 tick につき 1 回だけ起動する。
        let gpu = gpu::read_gpu_stats();
        app.on_tick(gpu);

        guard.terminal().draw(|f| ui::draw(f, &app))?;

        if !pump_keys(&mut app, Duration::from_secs(1))? {
            break;
        }
    }

    Ok(())
}
