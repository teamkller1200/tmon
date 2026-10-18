use crate::app::{App, History, Mode, SortKey, StatusKind, Tab};
use crate::format::{bar_string, bytes, percent, rate};
use crate::gpu::{gpu_util_string, vram_string};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState, Tabs},
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

/// 見出し + グラフだけのパネル（枠 2 行以外は全部グラフ）。
fn graph_panel(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    color: Color,
    sparkline: Sparkline<'_>,
) {
    f.render_widget(panel_block(title, color), area);
    f.render_widget(
        sparkline.style(Style::default().fg(color)),
        panel_block(title, color).inner(area),
    );
}

/// 空き領域に CPU と GPU の推移を左右に並べる。高さ 4 未満では枠すら入らないので描かない。
fn trend_row(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    if area.height < 4 {
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    graph_panel(f, cols[0], "CPU trend", Color::LightBlue, spark(&app.hist_cpu, Some(100)));
    graph_panel(f, cols[1], "GPU trend", Color::LightYellow, spark(&app.hist_gpu, Some(100)));
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
    // Cores は本来の高さ（枠 2 + コア数）だけ使い、余った下の空きを CPU/GPU の推移に回す。
    let cores_h = (m.per_core.len() as u16 + 2).min(rows[5].height);
    let rest = Layout::vertical([Constraint::Length(cores_h), Constraint::Min(0)]).split(rows[5]);
    let title = format!("Cores ({})", m.per_core.len());
    f.render_widget(
        Paragraph::new(cores)
            .style(Style::default().fg(Color::LightBlue))
            .block(panel_block(&title, Color::LightBlue)),
        rest[0],
    );
    trend_row(f, rest[1], app);
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
/// 入力と確認は中央のモーダルに出すので、ここは常にキーガイド（q / Tab はヘッダにある）。
fn footer_text(app: &App) -> String {
    if app.tab != Tab::Processes {
        return String::new();
    }
    let filter = if app.filter.is_empty() { "(none)".to_string() } else { app.filter.clone() };
    let order = if app.desc { "desc" } else { "asc" };
    let base = format!(
        "Filter: {filter}   (/)    x kill    s sort {} {order}    r reverse",
        sort_label(app.sort)
    );
    // 結果を先頭に出す: 狭いターミナルで切れるのはキーガイド側にする。
    match &app.status {
        Some(status) => format!("{status}    {base}"),
        None => base,
    }
}

/// 画面中央の矩形（ターミナルが小さければそこに収まるまで縮める）。
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

/// 中央のモーダル枠を描く。中身は 2 行（枠が隠れないよう Clear で下を消す）。
fn modal(
    f: &mut ratatui::Frame<'_>,
    screen: Rect,
    title: &str,
    color: Color,
    body: [String; 2],
) {
    let rect = centered(screen, 56, 4);
    f.render_widget(Clear, rect);
    f.render_widget(panel_block(title, color), rect);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
        .split(panel_block(title, color).inner(rect));
    let [first, second] = body;
    f.render_widget(Paragraph::new(first).style(Style::default().fg(Color::White)), rows[0]);
    f.render_widget(Paragraph::new(second).style(Style::default().fg(Color::Gray)), rows[1]);
}

/// kill の確認（y 以外はすべて中止）。
fn confirm_modal(f: &mut ratatui::Frame<'_>, screen: Rect, app: &App) {
    let Some(pid) = app.pending_kill else {
        return;
    };
    let name = app.pending_name.as_deref().unwrap_or("?");
    modal(
        f,
        screen,
        "Kill process",
        Color::Yellow,
        [format!("PID {pid} ({name})"), "[y] kill    [n] cancel".to_string()],
    );
}

/// フィルタ入力（Enter / Esc で閉じる。絞り込みは入力と同時に効く）。
fn filter_modal(f: &mut ratatui::Frame<'_>, screen: Rect, app: &App) {
    modal(
        f,
        screen,
        "Filter",
        Color::Cyan,
        [format!("{}_", app.filter), "Enter/Esc: close    Backspace: delete".to_string()],
    );
}

/// プロセス表の数値列の幅（`processes_tab` の `widths` と揃える）。
const PID_W: u16 = 7;
const MEM_W: u16 = 10;
const DISK_W: u16 = 11;

/// 桁を揃えるため右寄せにする（幅より長い文字列はそのまま出す）。
fn right(width: u16, text: &str) -> String {
    let width = width as usize;
    if text.chars().count() >= width {
        return text.to_string();
    }
    format!("{}{text}", " ".repeat(width - text.chars().count()))
}

/// プロセス一覧。選択行は `app.selected` から毎フレーム作り直す（範囲外に出ないよう追従する）。
fn processes_tab(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = app.visible();
    // 一覧が画面を埋めないときだけ、下の空きに CPU/GPU の推移を出す。
    // テーブルの必要高さは「枠 2 + 見出し 1 + 行数」。
    let leftover = area.height.saturating_sub(rows.len() as u16 + 3);
    let (table_area, trend_area) = if leftover >= 4 {
        let c = Layout::vertical([Constraint::Min(3), Constraint::Length(leftover)]).split(area);
        (c[0], c[1])
    } else {
        (area, Rect::ZERO)
    };
    let widths = [
        Constraint::Min(20),
        Constraint::Length(PID_W),
        Constraint::Length(PID_W),
        Constraint::Length(MEM_W),
        Constraint::Length(DISK_W),
        Constraint::Length(DISK_W),
    ];
    // 数値は右寄せ（桁が揃う）。幅は下の `widths` と一致させる。
    let body = rows.iter().map(|p| {
        Row::new(vec![
            Cell::from(p.name.clone()),
            Cell::from(right(PID_W, &p.pid.to_string())),
            Cell::from(right(PID_W, &percent(p.cpu))),
            Cell::from(right(MEM_W, &bytes(p.memory))),
            Cell::from(right(DISK_W, &rate(p.disk_read_bps))),
            Cell::from(right(DISK_W, &rate(p.disk_write_bps))),
        ])
    });
    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from(right(PID_W, "PID")),
        Cell::from(right(PID_W, "CPU%")),
        Cell::from(right(MEM_W, "MEM")),
        Cell::from(right(DISK_W, "DISK R")),
        Cell::from(right(DISK_W, "DISK W")),
    ])
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
    f.render_stateful_widget(table, table_area, &mut state);
    trend_row(f, trend_area, app);
}

pub fn draw(f: &mut ratatui::Frame<'_>, app: &App) {
    let screen = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(screen);

    let header = Layout::horizontal([Constraint::Min(0), Constraint::Length(23)]).split(chunks[0]);
    f.render_widget(
        Tabs::new(Tab::titles())
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

    // spec §6.0: kill の結果は 成功=Green / 警告=Yellow / 失敗=Red。無ければキーガイド色。
    let style = if app.status.is_none() {
        Style::default().fg(Color::DarkGray)
    } else {
        match app.status_kind {
            StatusKind::Ok => Style::default().fg(Color::Green),
            StatusKind::Warn => Style::default().fg(Color::Yellow),
            StatusKind::Err => Style::default().fg(Color::Red),
            StatusKind::Info => Style::default().fg(Color::DarkGray),
        }
    };
    f.render_widget(Paragraph::new(footer_text(app)).style(style), chunks[2]);

    // 入力と確認は中央のモーダルに出す（フッタ 1 行だと狭いターミナルで切れるため）。
    match app.mode {
        Mode::ConfirmKill => confirm_modal(f, screen, app),
        Mode::Filter => filter_modal(f, screen, app),
        Mode::Normal => {}
    }
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
    use crate::metrics::{KillOutcome, Metrics, ProcessRow};
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

    /// 入力と確認はフッタではなく画面中央のモーダル。縦横の中央寄せと中身を桁で確認する。
    #[test]
    fn kill_confirm_modal_is_centered_on_screen() {
        let mut app = app_with_processes(&[("notepad.exe", 1234, 1.0)]);
        app.tab = Tab::Processes;
        app.request_kill(Some(999));
        let shot = snapshot(&app, 120, 30);
        let lines = shot.split('\n').collect::<Vec<_>>();
        let top = lines.iter().position(|l| l.contains("Kill process")).expect("モーダルが無い");
        assert_eq!(top, 13, "縦中央に無い:\n{shot}");
        // 背後のパネルの枠が左端に残るので、モーダル内の文字の桁で中央寄せを見る。
        let column = lines[top].chars().position(|c| c == 'K').expect("タイトルが無い");
        assert_eq!(column, (120 - 56) / 2 + 1, "横中央に無い:\n{shot}");
        assert!(lines[top + 1].contains("PID 1234 (notepad.exe)"), "対象が無い:\n{shot}");
        assert!(lines[top + 2].contains("[y] kill"), "確認の文言が無い:\n{shot}");
    }

    #[test]
    fn filter_modal_shows_the_live_input() {
        let mut app = app_with_processes(&[("chrome.exe", 7, 1.0)]);
        app.tab = Tab::Processes;
        app.filter = "chr".to_string();
        app.mode = Mode::Filter;
        let shot = snapshot(&app, 100, 20);
        let lines = shot.split('\n').collect::<Vec<_>>();
        let top = lines.iter().position(|l| l.contains("Filter")).expect("モーダルが無い");
        assert_eq!(top, 8, "縦中央に無い:\n{shot}");
        assert!(lines[top + 1].contains("chr_"), "入力とカーソルが無い:\n{shot}");
    }

    #[test]
    fn processes_tab_shows_trends_in_the_leftover_space() {
        // 行が 2 つだけなので、30 行の下に空きができる。
        let mut app = app_with_processes(&[("a.exe", 1, 1.0), ("b.exe", 2, 2.0)]);
        app.tab = Tab::Processes;
        let shot = snapshot(&app, 120, 30);
        assert!(shot.contains("CPU trend"), "CPU の推移が無い:\n{shot}");
        assert!(shot.contains("GPU trend"), "GPU の推移が無い:\n{shot}");
    }

    #[test]
    fn trends_are_omitted_when_there_is_no_room() {
        let mut app = app_with_processes(&[("a.exe", 1, 1.0), ("b.exe", 2, 2.0)]);
        app.tab = Tab::Processes;
        let shot = snapshot(&app, 120, 6);
        assert!(!shot.contains("trend"), "空きが無いのに描かれた:\n{shot}");
    }

    #[test]
    fn performance_tab_shows_trends_below_the_cores() {
        // テストデータはコア 3 つなので、40 行では Cores の下が空く。
        let shot = snapshot(&app_with_metrics(), 120, 40);
        assert!(shot.contains("CPU trend"), "CPU の推移が無い:\n{shot}");
        assert!(shot.contains("GPU trend"), "GPU の推移が無い:\n{shot}");
    }

    /// `Tab::hit` の桁計算が実際の描画と一致することを、描いたヘッダから確認する。
    #[test]
    fn tab_hit_test_agrees_with_the_rendered_header() {
        let shots = snapshot(&app_with_metrics(), 120, 20);
        let header = shots.split('\n').next().unwrap();
        // 区切りの `│` はマルチバイトなので、バイト位置ではなく文字数で桁を数える。
        let column = |needle: &str| header[..header.find(needle).unwrap()].chars().count() as u16;

        assert_eq!(Tab::hit(column("Performance")), Some(Tab::Performance));
        assert_eq!(Tab::hit(column("Processes")), Some(Tab::Processes));
        assert_eq!(
            Tab::hit(column("Processes") - 2),
            Some(Tab::Performance),
            "見出しの間の区切りは左のタブ: {header:?}"
        );
        assert_eq!(Tab::hit(column("Processes") + 10), None, "見出しの右側は無効: {header:?}");
    }

    #[test]
    fn kill_result_footer_is_colored_by_kind() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        app.tab = Tab::Processes;
        app.set_kill_result(1, KillOutcome::Killed);
        let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        // フッタは最下行。成功は Green / 警告は Yellow / 失敗は Red。
        assert_eq!(buffer[(0, 19)].style().fg, Some(Color::Green));
        app.set_kill_result(1, KillOutcome::Failed);
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 19)].style().fg, Some(Color::Red));
    }

    #[test]
    fn numeric_columns_are_right_aligned() {
        let mut app = app_with_processes(&[("a.exe", 1234, 12.5)]);
        app.tab = Tab::Processes;
        let shot = snapshot(&app, 100, 12);
        assert!(shot.contains("    PID"), "見出しが右寄せでない:\n{shot}");
        assert!(shot.contains("   1234"), "PID が右寄せでない:\n{shot}");
    }

    #[test]
    fn processes_snapshot_at_tiny_size_does_not_panic() {
        let mut app = app_with_processes(&[("a", 1, 1.0)]);
        app.tab = Tab::Processes;
        let _ = snapshot(&app, 20, 5);
    }
}
