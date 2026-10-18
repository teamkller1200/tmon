//! システム全体とプロセスの計測（sysinfo）。

use std::time::Instant;

use sysinfo::{Disks, Networks, Pid, ProcessesToUpdate, System};

use crate::gpu::{self, GpuStats};

/// 前回サンプルからの差分バイト数を 1 秒あたりの値に直す（切り捨て）。
pub fn rate_per_sec(delta_bytes: u64, elapsed_secs: f64) -> u64 {
    if elapsed_secs <= 0.0 {
        return 0;
    }
    (delta_bytes as f64 / elapsed_secs) as u64
}

/// ループバック interface か。Windows の名前は "Loopback Pseudo-Interface 1"、Linux は "lo"。
/// 名前ベースの判定なので、WSL の "vEthernet (WSL)" は除外されない。
pub fn is_loopback_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("loopback") || lower == "lo"
}

/// このモニタ自身の PID。`system` feature の `get_current_pid` を使う。
pub fn own_pid() -> Option<u32> {
    sysinfo::get_current_pid().ok().map(|p| p.as_u32())
}

/// 誤って止めるとシステムや自分が死ぬ PID をガードする。拒否理由が無ければ None。
pub fn kill_blocked_reason(pid: u32, own_pid: Option<u32>) -> Option<&'static str> {
    match pid {
        0 => Some("PID 0 (System Idle Process) cannot be killed"),
        4 => Some("PID 4 (System) cannot be killed"),
        _ if own_pid == Some(pid) => Some("this monitor process cannot kill itself"),
        _ => None,
    }
}

/// 1 tick 分の計測値。
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub cpu_total: f32,
    pub per_core: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    pub gpu: Option<GpuStats>,
    pub processes: Vec<ProcessRow>,
}

/// プロセス一覧の 1 行。
#[derive(Clone, Debug, Default)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    /// プロセス開始時刻（PID 再利用の検出に使う）。
    pub start_secs: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
}

/// プロセス終了の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillOutcome {
    Killed,
    NotFound,
    Failed,
    /// PID が再利用されて別のプロセスになっていた（kill せず中止）。
    Replaced,
}

/// 1 秒ごとに sample() を呼び、差分からレートを出す計測器。
pub struct Sampler {
    sys: System,
    disks: Disks,
    networks: Networks,
    prev_at: Instant,
    prev_disk_read: u64,
    prev_disk_write: u64,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        // CPU 使用率は 2 回の refresh の差分なので、最初の値のために 1 回待つ。
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_all();

        // ディスク/ネットワークの基点は待機の後で取る（sleep 中の通信を初回レートに混ぜない）。
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();
        let (prev_disk_read, prev_disk_write) = disk_totals(&disks);

        Self { sys, disks, networks, prev_at: Instant::now(), prev_disk_read, prev_disk_write }
    }

    pub fn sample(&mut self) -> Metrics {
        // 差分の分母は「前回の計測開始から今回の計測開始まで」。refresh の後（末尾）で取ると
        // 途中の GPU 取得（nvidia-smi で数百 ms）が分母から抜けて速度が過大になる。
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.prev_at).as_secs_f64();
        self.prev_at = now;

        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        self.disks.refresh(true);
        self.networks.refresh(true);

        let cpu_count = self.sys.cpus().len().max(1) as f32;

        let (disk_read, disk_write) = disk_totals(&self.disks);
        let disk_read_bps = rate_per_sec(disk_read.saturating_sub(self.prev_disk_read), elapsed);
        let disk_write_bps =
            rate_per_sec(disk_write.saturating_sub(self.prev_disk_write), elapsed);
        self.prev_disk_read = disk_read;
        self.prev_disk_write = disk_write;

        let (mut rx, mut tx) = (0u64, 0u64);
        for (name, data) in self.networks.list() {
            if is_loopback_name(name) {
                continue;
            }
            // received/transmitted は前回 refresh からの差分（total_* は累積なので速度に使えない）。
            rx += data.received();
            tx += data.transmitted();
        }

        // global_cpu_usage() は Windows では "100 - \Processor(_Total)\% Idle Time" で既に 0..100
        // （sysinfo-0.39.6/src/windows/system.rs:143）。ここで割ると 16 コアで 1/16 の過小表示になる。
        let cpu_total = self.sys.global_cpu_usage().clamp(0.0, 100.0);

        let processes = self
            .sys
            .processes()
            .iter()
            .map(|(pid, p)| {
                let disk = p.disk_usage();
                ProcessRow {
                    pid: pid.as_u32(),
                    name: p.name().to_string_lossy().into_owned(),
                    // プロセスの CPU% も同じくコア数で割る（タスクマネージャーと同じ 0..100）。
                    cpu: (p.cpu_usage() / cpu_count).clamp(0.0, 100.0),
                    memory: p.memory(),
                    start_secs: p.start_time(),
                    disk_read_bps: rate_per_sec(disk.read_bytes, elapsed),
                    disk_write_bps: rate_per_sec(disk.written_bytes, elapsed),
                }
            })
            .collect();

        Metrics {
            cpu_total,
            per_core: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            disk_read_bps,
            disk_write_bps,
            net_rx_bps: rate_per_sec(rx, elapsed),
            net_tx_bps: rate_per_sec(tx, elapsed),
            gpu: gpu::read_gpu_stats(),
            processes,
        }
    }

    /// プロセスを終了する。見つからない場合だけ 1 回再取得してから判定する。
    /// `expected_start` は確認プロンプトを出した時点の `Process::start_time()`。Windows の kill は
    /// PID 指定の taskkill なので、PID が再利用されて別プロセスになっていたら中止する。
    pub fn kill(&mut self, pid: u32, expected_start: u64) -> KillOutcome {
        let pid = Pid::from_u32(pid);
        if self.sys.process(pid).is_none() {
            self.sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }
        match self.sys.process(pid) {
            Some(p) if p.start_time() != expected_start => KillOutcome::Replaced,
            Some(p) if p.kill() => KillOutcome::Killed,
            Some(_) => KillOutcome::Failed,
            None => KillOutcome::NotFound,
        }
    }
}

fn disk_totals(disks: &Disks) -> (u64, u64) {
    let read = disks.list().iter().map(|d| d.usage().total_read_bytes).sum();
    let write = disks.list().iter().map(|d| d.usage().total_written_bytes).sum();
    (read, write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_per_sec_handles_zero_elapsed() {
        assert_eq!(rate_per_sec(2000, 2.0), 1000);
        assert_eq!(rate_per_sec(100, 0.0), 0);
        assert_eq!(rate_per_sec(100, -1.0), 0);
        assert_eq!(rate_per_sec(3, 2.0), 1); // 切り捨て
    }

    #[test]
    fn is_loopback_name_matches_case_insensitively() {
        assert!(is_loopback_name("Loopback Pseudo-Interface 1"));
        assert!(is_loopback_name("loopback"));
        assert!(is_loopback_name("lo"));
        assert!(!is_loopback_name("Ethernet"));
        assert!(!is_loopback_name("vEthernet (WSL)"));
    }

    #[test]
    fn kill_blocked_reason_protects_system_and_self() {
        assert!(kill_blocked_reason(0, Some(999)).is_some());
        assert!(kill_blocked_reason(4, Some(999)).is_some());
        assert!(kill_blocked_reason(999, Some(999)).is_some());
        assert!(kill_blocked_reason(1234, Some(999)).is_none());
        assert!(kill_blocked_reason(1234, None).is_none());
    }

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
        // global_cpu_usage() は Windows では既に 0..100。ここでコア数を割ると過小表示に戻る。
        let raw = s.sys.global_cpu_usage().clamp(0.0, 100.0);
        assert!((m.cpu_total - raw).abs() < f32::EPSILON, "cpu_total {} != raw {raw}", m.cpu_total);
    }
}
