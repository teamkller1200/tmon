use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Terminal,
};

#[derive(Clone, Debug)]
struct GpuStats {
    gpu_util: u32,
    memory_used_mb: u32,
    memory_total_mb: u32,
    temperature_c: u32,
    power_w: f64,
    power_limit_w: f64,
    fan_speed_pct: u32,
}

fn read_gpu_stats() -> Option<GpuStats> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit,fan.speed",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut parts = stdout.split(',').map(str::trim);

    let gpu_util = parts.next()?.parse::<u32>().ok()?;
    let memory_used_mb = parts.next()?.parse::<u32>().ok()?;
    let memory_total_mb = parts.next()?.parse::<u32>().ok()?;
    let temperature_c = parts.next()?.parse::<u32>().ok()?;
    let power_w = parts.next()?.parse::<f64>().ok()?;
    let power_limit_w = parts.next()?.parse::<f64>().ok()?;
    let fan_speed_pct = parts.next()?.parse::<u32>().ok()?;

    Some(GpuStats {
        gpu_util,
        memory_used_mb,
        memory_total_mb,
        temperature_c,
        power_w,
        power_limit_w,
        fan_speed_pct,
    })
}

fn power_percent(stats: &GpuStats) -> u32 {
    if stats.power_limit_w <= 0.0 {
        0
    } else {
        ((stats.power_w / stats.power_limit_w) * 100.0).round() as u32
    }
}

fn bar_string(percent: u32, width: usize) -> String {
    let pct = percent.min(100) as usize;
    let filled = (pct * width).saturating_div(100);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn gpu_util_string(util: u32) -> String {
    format!("{}%", util)
}

fn vram_string(used_mb: u32, total_mb: u32) -> String {
    let used_gb = (used_mb as f64 / 1024.0).max(1.0).round() as u32;
    let total_gb = (total_mb as f64 / 1024.0).max(1.0).round() as u32;
    format!("{}/{}GB", used_gb, total_gb)
}

fn render_dashboard(
    f: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    stats: Option<GpuStats>,
    frame_count: usize,
) {
    let header = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("●", Style::default().fg(Color::Green)),
        Span::raw(" GPU Monitor"),
        Span::styled("  [q]", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).style(Style::default().fg(Color::White)));
    f.render_widget(title, header[0]);

    match stats {
        Some(stats) => {
            let memory_ratio = if stats.memory_total_mb > 0 {
                (stats.memory_used_mb as f64 / stats.memory_total_mb as f64 * 100.0).round() as u32
            } else {
                0
            };
            let power_pct = power_percent(&stats);

            let content_and_footer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(1)])
                .split(header[1]);
            let content = content_and_footer[0];
            let footer = content_and_footer[1];

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![Constraint::Length(1); 5])
                .split(content);

            let metrics = vec![
                ("GPU", gpu_util_string(stats.gpu_util), stats.gpu_util),
                ("VRAM", vram_string(stats.memory_used_mb, stats.memory_total_mb), memory_ratio),
                ("TEMP", format!("{}°C", stats.temperature_c), stats.temperature_c),
                ("PWR", format!("{}W", stats.power_w.round() as u32), power_pct),
                ("FAN", format!("{}%", stats.fan_speed_pct), stats.fan_speed_pct),
            ];

            for (idx, (label, value, percent)) in metrics.iter().enumerate() {
                let bar_width = (rows[idx].width as usize)
                    .saturating_sub(label.len() + value.len() + 8)
                    .max(6);
                let text = format!("{:<4} {:<5} {}", label, value, bar_string(*percent, bar_width));
                f.render_widget(Paragraph::new(text), rows[idx]);
            }
            let tick = Paragraph::new(format!("tick {:>4}", frame_count))
                .block(Block::default().title("Tick").borders(Borders::ALL));
            f.render_widget(tick, footer);
        }
        None => {
            let unavailable = Paragraph::new("GPU unavailable\nNVIDIA-SMI missing or unsupported GPU")
                .block(Block::default().title("GPU").borders(Borders::ALL));
            f.render_widget(unavailable, header[1]);
        }
    }
}

fn render_snapshot(width: u16, height: u16, stats: GpuStats) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(7)])
                .split(area);

            render_dashboard(f, chunks[1], Some(stats.clone()), 0);
            let gauge = Gauge::default()
                .block(Block::default().title("GPU Load").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Green))
                .percent(stats.gpu_util.min(100) as u16);
            f.render_widget(gauge, chunks[2]);
        })
        .unwrap();

    let buffer = terminal.backend().buffer().clone();
    let mut lines = Vec::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            let cell = &buffer[(x, y)];
            row.push(cell.symbol().chars().next().unwrap_or(' '));
        }
        let trimmed = row.trim_end_matches(' ').to_string();
        if !trimmed.is_empty() || y + 1 == height {
            lines.push(trimmed);
        }
    }
    lines.join("\n")
}

#[test]
fn dashboard_respects_terminal_size() {
    let stats = GpuStats {
        gpu_util: 45,
        memory_used_mb: 4000,
        memory_total_mb: 8000,
        temperature_c: 64,
        power_w: 150.0,
        power_limit_w: 300.0,
        fan_speed_pct: 60,
    };

    let wide = render_snapshot(120, 30, stats.clone());
    let narrow = render_snapshot(80, 20, stats);

    for expected in [
        ("GPU", "45%"),
        ("VRAM", "4/8GB"),
        ("TEMP", "64°C"),
        ("PWR", "150W"),
        ("FAN", "60%"),
    ] {
        let (label, value) = expected;
        assert!(wide.contains(label) && wide.contains(value), "wide snapshot missing {label} value {value}:\n{wide}");
        assert!(narrow.contains(label) && narrow.contains(value), "narrow snapshot missing {label} value {value}:\n{narrow}");
    }

    assert!(wide.contains("█") && narrow.contains("█"), "rendered bars should be present in wide and narrow snapshots");
    assert!(wide.lines().count() >= narrow.lines().count());
}

#[test]
fn bar_string_scales_correctly() {
    assert_eq!(bar_string(0, 10), "░".repeat(10));
    assert_eq!(bar_string(50, 10), "█████░░░░░");
    assert_eq!(bar_string(100, 10), "█".repeat(10));
}

#[test]
fn power_percent_is_normalized_to_limit() {
    let s = GpuStats {
        gpu_util: 50,
        memory_used_mb: 1000,
        memory_total_mb: 2000,
        temperature_c: 60,
        power_w: 150.0,
        power_limit_w: 300.0,
        fan_speed_pct: 50,
    };
    assert_eq!(power_percent(&s), 50);
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut frame_count = 0usize;
    let mut last = Instant::now();

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let stats = read_gpu_stats();
            render_dashboard(f, area, stats, frame_count);

            let gauge = Gauge::default()
                .block(Block::default().title("GPU Load").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Green));

            let percent = read_gpu_stats().map(|s| s.gpu_util.min(100)).unwrap_or(0);
            let gpu = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(7)])
                .split(area);
            f.render_widget(gauge.percent(percent as u16), gpu[1]);
        })?;

        frame_count += 1;

        loop {
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                        disable_raw_mode()?;
                        terminal.backend_mut().execute(LeaveAlternateScreen)?;
                        return Ok(());
                    }
                }
            }

            if last.elapsed() >= Duration::from_secs(1) {
                last = Instant::now();
                break;
            }
        }
    }
}
