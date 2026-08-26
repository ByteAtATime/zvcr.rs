use std::time::Duration;

pub(super) fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub(super) fn format_duration(dur: Duration) -> String {
    let secs = dur.as_secs();
    if secs >= 60 {
        format!("{:02}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{:.2}s", dur.as_secs_f64())
    }
}

pub(super) fn format_rate(rate: f64, unit: &str) -> String {
    const K: f64 = 1e3;
    const M: f64 = 1e6;
    const G: f64 = 1e9;
    if rate >= G {
        format!("{:.2} G{}/s", rate / G, unit)
    } else if rate >= M {
        format!("{:.2} M{}/s", rate / M, unit)
    } else if rate >= K {
        format!("{:.2} K{}/s", rate / K, unit)
    } else {
        format!("{rate:.0} {}/s", unit)
    }
}
