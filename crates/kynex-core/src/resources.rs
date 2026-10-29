//! What Kynex itself costs the machine: CPU and memory for its own process and
//! every one under it (`claude` sessions, terminals, the tools they run), for
//! the status footer (#172). Read with `sysinfo`, every few seconds.

use std::collections::HashMap;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// One process in Kynex's tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// Share of the whole machine, 0–100.
    pub cpu: f32,
    /// Resident memory, in bytes.
    pub memory: u64,
}

/// Kynex and everything under it, Kynex first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub processes: Vec<Proc>,
}

impl Usage {
    /// CPU share of the whole machine, 0–100.
    pub fn cpu(&self) -> f32 {
        self.processes.iter().fold(0., |sum, p| sum + p.cpu)
    }

    pub fn memory(&self) -> u64 {
        self.processes.iter().map(|p| p.memory).sum()
    }

    /// "CPU 3% · 412 MB".
    pub fn label(&self) -> String {
        format!("CPU {} · {}", percent(self.cpu()), bytes(self.memory()))
    }

    /// The breakdown for a tooltip: one line per kind of process, biggest first,
    /// "claude ×3 · 1% · 610 MB".
    pub fn describe(&self) -> String {
        let mut kinds: Vec<(String, usize, f32, u64)> = Vec::new();
        for p in &self.processes {
            let name = p.name.strip_suffix(".exe").unwrap_or(&p.name);
            match kinds.iter_mut().find(|k| k.0 == name) {
                Some(k) => (k.1, k.2, k.3) = (k.1 + 1, k.2 + p.cpu, k.3 + p.memory),
                None => kinds.push((name.to_string(), 1, p.cpu, p.memory)),
            }
        }
        kinds.sort_by_key(|k| std::cmp::Reverse(k.3));
        kinds
            .iter()
            .map(|(name, n, cpu, mem)| {
                let name = if *n > 1 { format!("{name} ×{n}") } else { name.clone() };
                format!("{name} · {} · {}", percent(*cpu), bytes(*mem))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Reads the process table. Keep one: CPU shares are measured between reads.
pub struct Sampler {
    /// None where the OS won't give out its process table.
    sys: Option<System>,
    me: Option<Pid>,
    cores: f32,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            sys: System::new().ok(),
            me: sysinfo::get_current_pid().ok(),
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()) as f32,
        }
    }
}

impl Sampler {
    /// Kynex's tree as of now; CPU is the average since the last call (0 on the
    /// first). Blocks while it reads every process: call it off the UI thread.
    pub fn sample(&mut self) -> Usage {
        let (Some(me), Some(sys)) = (self.me, self.sys.as_mut()) else { return Usage::default() };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let procs = sys.processes();
        let parents: Vec<(u32, Option<u32>)> =
            procs.iter().map(|(pid, p)| (pid.as_u32(), p.parent().map(Pid::as_u32))).collect();
        let processes = tree(me.as_u32(), &parents)
            .into_iter()
            .filter_map(|pid| {
                let p = procs.get(&Pid::from_u32(pid))?;
                Some(Proc {
                    pid,
                    name: p.name().to_string_lossy().into_owned(),
                    cpu: p.cpu_usage() / self.cores,
                    memory: p.memory(),
                })
            })
            .collect();
        Usage { processes }
    }
}

/// `root` and every process under it, given each process's parent; root first,
/// then breadth first. A loop in the table (pids reused) can't make it repeat.
fn tree(root: u32, parents: &[(u32, Option<u32>)]) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, parent) in parents {
        if let Some(parent) = parent.filter(|&p| p != pid) {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut out = vec![root];
    let mut i = 0;
    while let Some(&pid) = out.get(i) {
        for &c in children.get(&pid).into_iter().flatten() {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        i += 1;
    }
    out
}

fn percent(p: f32) -> String {
    if p > 0. && p < 1. { "<1%".into() } else { format!("{:.0}%", p) }
}

/// "412 MB", "1.3 GB".
fn bytes(n: u64) -> String {
    const MB: f64 = 1024. * 1024.;
    let mb = n as f64 / MB;
    if mb >= 1024. { format!("{:.1} GB", mb / 1024.) } else { format!("{:.0} MB", mb) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_is_kynex_and_everything_under_it() {
        let parents =
            [(1, None), (10, Some(1)), (11, Some(10)), (12, Some(10)), (20, Some(1)), (30, Some(2)), (31, Some(31))];
        assert_eq!(tree(10, &parents), [10, 11, 12]);
        assert_eq!(tree(31, &parents), [31], "its own parent");
        // A reused pid pointing back up can't loop.
        assert_eq!(tree(10, &[(10, Some(11)), (11, Some(10))]), [10, 11]);
    }

    #[test]
    fn labels_and_breakdown() {
        let p = |name: &str, cpu: f32, mb: u64| Proc { pid: 0, name: name.into(), cpu, memory: mb * 1024 * 1024 };
        let u = Usage {
            processes: vec![
                p("kynex.exe", 2.4, 300),
                p("claude.exe", 0.3, 400),
                p("claude.exe", 0.2, 500),
                p("pwsh.exe", 0., 80),
            ],
        };
        assert_eq!(u.label(), "CPU 3% · 1.2 GB");
        assert_eq!(u.describe(), "claude ×2 · <1% · 900 MB\nkynex · 2% · 300 MB\npwsh · 0% · 80 MB");
        assert_eq!(Usage::default().label(), "CPU 0% · 0 MB");
    }

    #[test]
    fn samples_this_process() {
        let mut s = Sampler::default();
        let u = s.sample();
        assert_eq!(u.processes.first().map(|p| p.pid), Some(std::process::id()));
        assert!(u.memory() > 0);
    }
}
