#[derive(Clone, Debug)]
pub struct GpuStats {
    pub gpu_util: u32,
    pub memory_used_mb: u32,
    pub memory_total_mb: u32,
    pub temperature_c: u32,
    pub power_w: f64,
    pub power_limit_w: f64,
    pub fan_speed_pct: u32,
}

/// nvidia-smi を 1 回起動して GPU 統計を読む。失敗（未インストール/非対応 GPU）は `None`。
pub fn read_gpu_stats() -> Option<GpuStats> {
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

pub fn power_percent(stats: &GpuStats) -> u32 {
    if stats.power_limit_w <= 0.0 {
        0
    } else {
        ((stats.power_w / stats.power_limit_w) * 100.0).round() as u32
    }
}

pub fn gpu_util_string(util: u32) -> String {
    format!("{}%", util)
}

pub fn vram_string(used_mb: u32, total_mb: u32) -> String {
    let used_gb = (used_mb as f64 / 1024.0).max(1.0).round() as u32;
    let total_gb = (total_mb as f64 / 1024.0).max(1.0).round() as u32;
    format!("{}/{}GB", used_gb, total_gb)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
