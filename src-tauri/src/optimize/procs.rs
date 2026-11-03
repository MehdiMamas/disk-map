//! Heavy processes: top 8 by CPU and by memory, grouped by process name. Read-only.
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProcGroup {
    pub name: String,
    /// Percent of the whole machine.
    pub cpu: f32,
    pub mem: u64,
    pub count: usize,
}

#[derive(Serialize)]
pub struct ProcReport {
    pub by_cpu: Vec<ProcGroup>,
    pub by_mem: Vec<ProcGroup>,
}

pub const TOP_N: usize = 8;

/// Sums cpu and memory of processes that share a name.
pub fn group(items: impl IntoIterator<Item = (String, f32, u64)>) -> Vec<ProcGroup> {
    let mut m: HashMap<String, ProcGroup> = HashMap::new();
    for (name, cpu, mem) in items {
        let g = m.entry(name.clone()).or_insert(ProcGroup { name, cpu: 0.0, mem: 0, count: 0 });
        g.cpu += cpu;
        g.mem += mem;
        g.count += 1;
    }
    m.into_values().collect()
}

pub fn top(groups: &[ProcGroup], n: usize, by_cpu: bool) -> Vec<ProcGroup> {
    let mut v = groups.to_vec();
    if by_cpu {
        v.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal).then(a.name.cmp(&b.name)));
    } else {
        v.sort_by(|a, b| b.mem.cmp(&a.mem).then(a.name.cmp(&b.name)));
    }
    v.truncate(n);
    v
}

pub fn collect() -> ProcReport {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    // CPU use needs two samples at least MINIMUM_CPU_UPDATE_INTERVAL apart.
    sys.refresh_processes(ProcessesToUpdate::All, true);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    let items: Vec<(String, f32, u64)> = sys
        .processes()
        .values()
        .map(|p| (p.name().to_string_lossy().into_owned(), p.cpu_usage() / cores, p.memory()))
        .filter(|(n, _, _)| n != "System Idle Process")
        .collect();
    let groups = group(items);
    ProcReport { by_cpu: top(&groups, TOP_N, true), by_mem: top(&groups, TOP_N, false) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_name_and_ranks() {
        let g = group(vec![
            ("chrome.exe".to_string(), 5.0, 100),
            ("chrome.exe".to_string(), 3.0, 200),
            ("code.exe".to_string(), 9.0, 50),
        ]);
        assert_eq!(g.len(), 2);
        let by_mem = top(&g, 1, false);
        assert_eq!(by_mem[0].name, "chrome.exe");
        assert_eq!(by_mem[0].mem, 300);
        assert_eq!(by_mem[0].count, 2);
        let by_cpu = top(&g, 8, true);
        assert_eq!(by_cpu[0].name, "code.exe");
        assert_eq!(by_cpu.len(), 2);
    }
}
