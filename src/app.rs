use crate::gpu::GpuStats;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

#[derive(Default)]
pub struct App {
    pub gpu: Option<GpuStats>,
    pub frame_count: u64,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// 1 秒 tick ごとに呼ぶ。この時点では GPU 値の差し替えとフレーム数の加算のみ。
    pub fn on_tick(&mut self, gpu: Option<GpuStats>) {
        self.gpu = gpu;
        self.frame_count += 1;
    }

    /// 押下キーを状態に反映する。`KeyEventKind::Press` 以外は無視する。
    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }
}
