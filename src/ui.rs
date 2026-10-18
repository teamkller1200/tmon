use crate::app::{App, History, Mode, SortKey, Tab};
use crate::format::{bar_string, bytes, percent, rate};
use crate::gpu::{gpu_util_string, vram_string};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, TableState, Tabs},
};

/// パネルの枠とタイトルをアクセント色で作る。
fn panel_block<'a>(title: &'a str, color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title, Style::default().fg(color)))
}

/// スパークライン用の履歴ウィジェット。`max` を渡すと縦軸を固定する。
fn spark(history: &History, max: Option<u64>) -> Sparkline<'_> {
    let sparkline = Sparkline::default().data(history.as_vec());
    match max {
        Some(m) => sparkline.max(m),
        None => sparkline,
    }
}

/// 数値 1 行 + グラフ 1 行のパネルを描く（枠 2 行 + 内容 2 行 = 高さ 4）。
fn metric_panel(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    color: Color,
    text: Line<'_>,
    sparkline: Sparkline<'_>,
) {
    f.render_widget(panel_block(title, color), area);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
        .split(panel_block(title, color).inner(area));
    f.render_widget(Paragraph::new(text).style(Style::default().fg(color)), rows[0]);
    f.render_widget(sparkline.style(Style::default().fg(color)), rows[1]);
}

fn performance_tab(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Min(0),
    ])
    .split(area);

    let m = &app.metrics;

    let cpu_pct = m.cpu_total.round().clamp(0.0, 100.0) as u32;
    metric_panel(
        f,
        rows[0],
        "CPU",
        Color::LightBlue,
        Line::from(format!("{}   {}", percent(m.cpu_total), bar_string(cpu_pct, 58))),
        spark(&app.hist_cpu, Some(100)),
    );

    let mem_pct = if m.mem_total > 0 {
        (m.mem_used as f64 / m.mem_total as f64 * 100.0).round() as u32
    } else {
        0
    };
    metric_panel(
        f,
        rows[1],
        "Memory",
        Color::LightMagenta,
        Line::from(format!(
            "{} / {} ({}%)   Swap {} / {}   {}",
            bytes(m.mem_used),
            bytes(m.mem_total),
            mem_pct,
            bytes(m.swap_used),
            bytes(m.swap_total),
            bar_string(mem_pct, 30),
        )),
        spark(&app.hist_mem, Some(100)),
    );

    metric_panel(
        f,
        rows[2],
        "Disk",
        Color::LightGreen,
        Line::from(format!("R {}   W {}", rate(m.disk_read_bps), rate(m.disk_write_bps))),
        spark(&app.hist_disk, None),
    );

    metric_panel(
        f,
        rows[3],
        "Network",
        Color::LightCyan,
        Line::from(format!("v {}   ^ {}", rate(m.net_rx_bps), rate(m.net_tx_bps))),
        spark(&app.hist_net, None),
    );

    match &m.gpu {
        Some(g) => metric_panel(
            f,
            rows[4],
            "GPU",
            Color::LightYellow,
            Line::from(format!(
                "{}   {}   VRAM {}   TEMP {}C   PWR {} / {}W   FAN {}%",
                gpu_util_string(g.gpu_util),
                bar_string(g.gpu_util, 28),
                vram_string(g.memory_used_mb, g.memory_total_mb),
                g.temperature_c,
                g.power_w.round() as u32,
                g.power_limit_w.round() as u32,
                g.fan_speed_pct,
            )),
            spark(&app.hist_gpu, Some(100)),
        ),
        None => {
            let text = "GPU unavailable\nNVIDIA-SMI missing or unsupported GPU";
            f.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::LightYellow))
                    .block(panel_block("GPU", Color::LightYellow)),
                rows[4],
            );
        }
    }

    let cores = m
        .per_core
        .iter()
        .enumerate()
        .map(|(i, usage)| {
            let pct = usage.round().clamp(0.0, 100.0) as u32;
            Line::from(format!("cpu{i:02} {pct:>3}% {}", bar_string(pct, 9)))
        })
        .collect::<Vec<_>>();
    let title = format!("Cores ({})", m.per_core.len());
    f.render_widget(
        Paragraph::new(cores)
            .style(Style::default().fg(Color::LightBlue))
            .block(panel_block(&title, Color::LightBlue)),
        rows[5],
    );
}

fn sort_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "name",
        SortKey::Pid => "pid",
        SortKey::Cpu => "cpu",
        SortKey::Memory => "mem",
        SortKey::Disk => "disk",
    }
}

/// フッタ行の文言。パフォーマンスタブでは何も出さない（操作はヘッダに書いてある）。
fn footer_text(app: &App) -> String {
    if app.tab != Tab::Processes {
        return String::new();
    }
    let base = match app.mode {
        Mode::Filter => format!("Filter: {}_   (Enter/Esc: close)", app.filter),
        Mode::ConfirmKill => "kill? (y/n)".to_string(), // Task 5 で実装
        Mode::Normal => {
            let filter = if app.filter.is_empty() { "(none)".to_string() } else { app.filter.clone() };
            let order = if app.desc { "desc" } else { "asc" };
            format!(
                "Filter: {filter}   (/)    x kill    s sort {}{order}    r reverse    Tab switch    q quit",
                sort_label(app.sort)
            )
        }
    };
    match &app.status {
        Some(status) => format!("{base}    {status}"),
        None => base,
    }
}

/// プロセス一覧。選択行は `app.selected` から毎フレーム作り直す（範囲外に出ないよう追従する）。
fn processes_tab(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = app.visible();
    let widths = [
        Constraint::Min(20),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(11),
    ];
    let body = rows.iter().map(|p| {
        Row::new(vec![
            Cell::from(p.name.clone()),
            Cell::from(p.pid.to_string()),
            Cell::from(percent(p.cpu)),
            Cell::from(bytes(p.memory)),
            Cell::from(rate(p.disk_read_bps)),
            Cell::from(rate(p.disk_write_bps)),
        ])
    });
    let header = Row::new(vec!["Name", "PID", "CPU%", "MEM", "DISK R", "DISK W"])
        .style(Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD));
    let table = Table::new(body, widths)
        .header(header)
        .column_spacing(1)
        .block(panel_block("Processes", Color::LightBlue))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

pub fn draw(f: &mut ratatui::Frame<'_>, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(f.area());

    let header = Layout::horizontal([Constraint::Min(0), Constraint::Length(23)]).split(chunks[0]);
    f.render_widget(
        Tabs::new(["Performance", "Processes"])
            .select(app.tab as usize)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        header[0],
    );
    f.render_widget(
        Paragraph::new("[q] quit  [Tab] switch")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right),
        header[1],
    );

    match app.tab {
        Tab::Performance => performance_tab(f, chunks[1], app),
        Tab::Processes => processes_tab(f, chunks[1], app),
    }

    let style = if app.status.is_some() {
        Style::default().fg(Color::LightYellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(footer_text(app)).style(style), chunks[2]);
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
    use crate::app::App;
    use crate::metrics::{Metrics, ProcessRow};
    use ratatui::backend::TestBackend;

    fn sample_gpu() -> crate::gpu::GpuStats {
        crate::gpu::GpuStats {
            gpu_util: 45,
            memory_used_mb: 4000,
            memory_total_mb: 8000,
            temperature_c: 64,
            power_w: 150.0,
            power_limit_w: 300.0,
            fan_speed_pct: 60,
        }
    }

    fn app_with_metrics() -> App {
        let mut app = App::new();
        app.on_tick(Metrics {
            cpu_total: 42.0,
            per_core: vec![10.0, 20.0, 30.0],
            mem_used: 12 * 1024 * 1024 * 1024,
            mem_total: 32 * 1024 * 1024 * 1024,
            swap_used: 1024 * 1024 * 1024,
            swap_total: 8 * 1024 * 1024 * 1024,
            disk_read_bps: 1024 * 1024,
            disk_write_bps: 300 * 1024,
            net_rx_bps: 512 * 1024,
            net_tx_bps: 2048,
            gpu: Some(sample_gpu()),
            processes: Vec::new(),
        });
        app
    }

    fn app_with_processes(names: &[(&str, u32, f32)]) -> App {
        let mut app = App::new();
        app.on_tick(Metrics {
            processes: names
                .iter()
                .map(|(name, pid, cpu)| ProcessRow {
                    pid: *pid,
                    name: name.to_string(),
                    cpu: *cpu,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        app
    }

    #[test]
    fn snapshot_has_exact_height() {
        let shots = snapshot(&App::new(), 40, 12);
        // 末尾の空行も 1 行として数える（lines() は末尾の改行の後を数えない）
        assert_eq!(shots.split('\n').count(), 12);
    }

    #[test]
    fn performance_tab_shows_every_panel() {
        let wide = snapshot(&app_with_metrics(), 120, 40);
        for label in [
            "Performance",
            "Processes",
            "CPU",
            "Memory",
            "Disk",
            "Network",
            "GPU",
            "Cores",
            "42.0%",
        ] {
            assert!(wide.contains(label), "snapshot missing {label}:\n{wide}");
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

    #[test]
    fn panels_use_their_accent_colors() {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();
        let app = app_with_metrics();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        // 枠の左上の角（1 行目はタブ、以降 4 行ごとに次 のパネル）。
        for (y, expected) in [
            (1, Color::LightBlue),
            (5, Color::LightMagenta),
            (9, Color::LightGreen),
            (13, Color::LightCyan),
            (17, Color::LightYellow),
        ] {
            assert_eq!(buffer[(0, y)].style().fg, Some(expected), "panel at y={y}");
        }
    }

    #[test]
    fn processes_tab_lists_sorted_rows_with_headers() {
        let mut app = app_with_processes(&[("alpha.exe", 111, 5.0), ("beta.exe", 222, 50.0)]);
        app.tab = Tab::Processes;
        let shot = snapshot(&app, 120, 30);
        for label in ["Name", "PID", "CPU%", "MEM", "DISK R", "DISK W", "alpha.exe", "beta.exe"] {
            assert!(shot.contains(label), "snapshot missing {label}:\n{shot}");
        }
        let (beta, alpha) = (shot.find("beta.exe").unwrap(), shot.find("alpha.exe").unwrap());
        assert!(beta < alpha, "CPU 降順なので beta.exe が先に来るべき:\n{shot}");
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
}
