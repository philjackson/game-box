//! Lightweight /proc sampling for the btop-style meters. Linux only, which is
//! the only place wine gaming happens anyway.

use std::collections::VecDeque;
use std::fs;

const HISTORY: usize = 120;

#[derive(Debug, Default, Clone, Copy)]
struct CpuTotals {
    busy: u64,
    total: u64,
}

/// Rolling machine-wide CPU/RAM samples plus per-process samples for the
/// games we launched.
#[derive(Debug)]
pub struct SysMon {
    prev: CpuTotals,
    pub cpu_history: VecDeque<f32>,
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    clock_ticks: f64,
    procs: Vec<(u32, u64, std::time::Instant)>,
}

impl Default for SysMon {
    fn default() -> Self {
        Self::new()
    }
}

impl SysMon {
    pub fn new() -> SysMon {
        SysMon {
            prev: CpuTotals::default(),
            cpu_history: VecDeque::from(vec![0.0; HISTORY]),
            cpu: 0.0,
            mem_used: 0,
            mem_total: 0,
            clock_ticks: 100.0,
            procs: Vec::new(),
        }
    }

    pub fn tick(&mut self) {
        if let Some(t) = read_cpu_totals() {
            if self.prev.total > 0 && t.total > self.prev.total {
                let dt = (t.total - self.prev.total) as f32;
                let db = (t.busy - self.prev.busy) as f32;
                self.cpu = (db / dt * 100.0).clamp(0.0, 100.0);
            }
            self.prev = t;
        }
        self.cpu_history.push_back(self.cpu);
        while self.cpu_history.len() > HISTORY {
            self.cpu_history.pop_front();
        }
        let (used, total) = read_mem();
        self.mem_used = used;
        self.mem_total = total;
    }

    pub fn mem_pct(&self) -> f32 {
        if self.mem_total == 0 {
            0.0
        } else {
            self.mem_used as f32 / self.mem_total as f32 * 100.0
        }
    }

    /// CPU% of a process tree since the last call, and its RSS in bytes.
    pub fn sample_proc(&mut self, pid: u32) -> Option<(f32, u64)> {
        let jiffies = proc_tree_jiffies(pid)?;
        let rss = proc_tree_rss(pid);
        let now = std::time::Instant::now();
        let entry = self.procs.iter_mut().find(|(p, _, _)| *p == pid);
        let cpu = match entry {
            Some((_, prev_j, prev_t)) => {
                let secs = now.duration_since(*prev_t).as_secs_f64();
                let pct = if secs > 0.05 {
                    (jiffies.saturating_sub(*prev_j) as f64 / self.clock_ticks / secs * 100.0) as f32
                } else {
                    0.0
                };
                *prev_j = jiffies;
                *prev_t = now;
                pct
            }
            None => {
                self.procs.push((pid, jiffies, now));
                0.0
            }
        };
        Some((cpu.max(0.0), rss))
    }

    pub fn forget(&mut self, pid: u32) {
        self.procs.retain(|(p, _, _)| *p != pid);
    }
}

fn read_cpu_totals() -> Option<CpuTotals> {
    let raw = fs::read_to_string("/proc/stat").ok()?;
    let line = raw.lines().next()?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTotals { busy: total.saturating_sub(idle), total })
}

fn read_mem() -> (u64, u64) {
    let Ok(raw) = fs::read_to_string("/proc/meminfo") else { return (0, 0) };
    let mut total = 0u64;
    let mut available = 0u64;
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("MemTotal:"), Some(v)) => total = v.parse().unwrap_or(0) * 1024,
            (Some("MemAvailable:"), Some(v)) => available = v.parse().unwrap_or(0) * 1024,
            _ => {}
        }
    }
    (total.saturating_sub(available), total)
}

/// All pids in `pid`'s subtree, walking /proc once.
fn descendants(pid: u32) -> Vec<u32> {
    let mut children: Vec<(u32, u32)> = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else { return vec![pid] };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(p) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if let Some(ppid) = read_ppid(p) {
            children.push((ppid, p));
        }
    }
    let mut out = vec![pid];
    let mut i = 0;
    while i < out.len() && out.len() < 4096 {
        let cur = out[i];
        for (parent, child) in &children {
            if *parent == cur && !out.contains(child) {
                out.push(*child);
            }
        }
        i += 1;
    }
    out
}

fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let raw = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // comm can contain spaces and parens; everything after the last ')' is safe.
    let idx = raw.rfind(')')?;
    let rest = &raw[idx + 1..];
    Some(rest.split_whitespace().map(str::to_string).collect())
}

fn read_ppid(pid: u32) -> Option<u32> {
    // After the comm field: state, ppid, ...
    stat_fields(pid)?.get(1)?.parse().ok()
}

fn proc_tree_jiffies(pid: u32) -> Option<u64> {
    if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
        return None;
    }
    let mut total = 0u64;
    for p in descendants(pid) {
        if let Some(f) = stat_fields(p) {
            // utime is field 11 and stime 12 counting from the state field.
            let utime: u64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
            let stime: u64 = f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
            total += utime + stime;
        }
    }
    Some(total)
}

fn proc_tree_rss(pid: u32) -> u64 {
    let page = 4096u64;
    let mut total = 0u64;
    for p in descendants(pid) {
        if let Ok(raw) = fs::read_to_string(format!("/proc/{}/statm", p)) {
            if let Some(rss) = raw.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()) {
                total += rss * page;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_our_own_process() {
        let me = std::process::id();
        assert!(read_ppid(me).is_some(), "ppid should parse");
        // A running process has burned at least a tick of CPU by now, and its
        // RSS is certainly non-zero.
        assert!(proc_tree_jiffies(me).is_some());
        assert!(proc_tree_rss(me) > 0, "rss should be non-zero");
        assert!(descendants(me).contains(&me));
    }

    #[test]
    fn comm_with_spaces_does_not_shift_fields() {
        // The parser slices after the last ')', so a comm like "(my proc)"
        // must not offset ppid. Verify against pid 1, which always exists.
        let fields = stat_fields(1).expect("pid 1 exists");
        assert!(fields.len() > 12);
        assert!(fields[0].len() == 1, "first field after comm is the state char");
    }

    #[test]
    fn machine_meters_are_sane() {
        let mut sys = SysMon::new();
        sys.tick();
        sys.tick();
        assert!(sys.mem_total > 0);
        assert!(sys.mem_used <= sys.mem_total);
        assert!((0.0..=100.0).contains(&sys.cpu));
        assert!((0.0..=100.0).contains(&sys.mem_pct()));
    }
}
