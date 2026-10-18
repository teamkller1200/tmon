use crate::app::App;
use crate::format::bar_string;
use crate::gpu::{gpu_util_string, power_percent, vram_string, GpuStats};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
};

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

            let metrics = [
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
            let unavailable = Paragraph::new("GPU unavailable
NVIDIA-SMI missing or unsupported GPU")
                .block(Block::default().title("GPU").borders(Borders::ALL));
            f.render_widget(unavailable, header[1]);
        }
    }
}

pub fn draw(f: &mut ratatui::Frame<'_>, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(7)])
        .split(area);

    render_dashboard(f, chunks[1], app.gpu.clone(), app.frame_count as usize);

    let percent = app.gpu.as_ref().map(|s| s.gpu_util.min(100)).unwrap_or(0);
    let gauge = Gauge::default()
        .block(Block::default().title("GPU Load").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Green))
        .percent(percent as u16);
    f.render_widget(gauge, chunks[2]);
}

/// TestBackend に 1 フレーム描画し、各行末尾の空白を trim した文字列を返す。行数は必ず `height`。
/// 現状はテスト専用なので `cfg(test)`（本体から使うようになったら属性を外す）。
#[cfg(test)]
pub fn snapshot(app: &App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let mut lines = Vec::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            let cell = &buffer[(x, y)];
            row.push(cell.symbol().chars().next().unwrap_or(' '));
        }
        lines.push(row.trim_end_matches(' ').to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_gpu() -> GpuStats {
        GpuStats {
            gpu_util: 45,
            memory_used_mb: 4000,
            memory_total_mb: 8000,
            temperature_c: 64,
            power_w: 150.0,
            power_limit_w: 300.0,
            fan_speed_pct: 60,
        }
    }

    #[test]
    fn snapshot_has_exact_height() {
        let app = App::new();
        let shots = snapshot(&app, 40, 12);
        assert_eq!(shots.lines().count(), 12);
    }

    #[test]
    fn dashboard_respects_terminal_size() {
        let app = App { gpu: Some(sample_gpu()), ..Default::default() };

        let wide = snapshot(&app, 120, 30);
        let narrow = snapshot(&app, 80, 20);

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
}
