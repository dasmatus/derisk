//! Parsers for Linux `/proc` files. Each takes the file's text, so they are
//! testable without a real `/proc`.

/// Aggregate CPU time from the first line of `/proc/stat`, in clock ticks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuTimes {
    /// All time, busy or idle.
    pub total: u64,
    /// Idle and I/O wait time.
    pub idle: u64,
}

impl CpuTimes {
    /// Busy share between two samples, from 0 to 100.
    pub fn usage_since(self, earlier: Self) -> f32 {
        let total = self.total.saturating_sub(earlier.total);
        let idle = self.idle.saturating_sub(earlier.idle);
        if total == 0 {
            0.0
        } else {
            (total.saturating_sub(idle) as f64 * 100.0 / total as f64) as f32
        }
    }
}

/// Parses the aggregate `cpu` line of `/proc/stat`.
pub fn parse_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    // user nice system idle iowait irq softirq steal; guest time is already
    // counted in user and nice.
    let total = fields.iter().take(8).sum();
    let idle = fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes { total, idle })
}

/// Counts `cpuN` lines in `/proc/stat`.
pub fn parse_cpu_count(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count()
}

/// Memory figures from `/proc/meminfo`, in KiB.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Memory {
    /// Installed memory.
    pub total: u64,
    /// Memory available for new work without swapping.
    pub available: u64,
    /// Swap space.
    pub swap_total: u64,
    /// Unused swap space.
    pub swap_free: u64,
}

impl Memory {
    /// Memory in use.
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Swap in use.
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

fn kib_field(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(':')?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// Parses `/proc/meminfo`.
pub fn parse_meminfo(text: &str) -> Option<Memory> {
    let total = kib_field(text, "MemTotal")?;
    Some(Memory {
        total,
        // Kernels before 3.14 lack MemAvailable; MemFree underestimates it.
        available: kib_field(text, "MemAvailable")
            .or_else(|| kib_field(text, "MemFree"))
            .unwrap_or(0),
        swap_total: kib_field(text, "SwapTotal").unwrap_or(0),
        swap_free: kib_field(text, "SwapFree").unwrap_or(0),
    })
}

/// Parses the 1, 5, and 15 minute averages from `/proc/loadavg`.
pub fn parse_loadavg(text: &str) -> Option<[f32; 3]> {
    let mut fields = text.split_whitespace().map(str::parse::<f32>);
    Some([
        fields.next()?.ok()?,
        fields.next()?.ok()?,
        fields.next()?.ok()?,
    ])
}

/// Parses seconds since boot from `/proc/uptime`.
pub fn parse_uptime(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// Fields of `/proc/<pid>/stat` the monitor uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PidStat {
    /// Executable name, at most 15 bytes.
    pub name: String,
    /// One-letter state such as `R` (running) or `S` (sleeping).
    pub state: char,
    /// User plus system time, in clock ticks.
    pub ticks: u64,
    /// Number of threads.
    pub threads: u64,
}

/// Parses `/proc/<pid>/stat`. The name may contain spaces and parentheses,
/// so it is everything between the first `(` and the last `)`.
pub fn parse_pid_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // `rest[0]` is field 3 (state); utime is field 14, stime 15, threads 20.
    let field = |n: usize| rest.get(n - 3)?.parse::<u64>().ok();
    Some(PidStat {
        name,
        state: rest.first()?.chars().next()?,
        ticks: field(14)? + field(15)?,
        threads: field(20)?,
    })
}

/// Parses resident memory (`VmRSS`, KiB) from `/proc/<pid>/status`. Kernel
/// threads have none.
pub fn parse_pid_rss(text: &str) -> u64 {
    kib_field(text, "VmRSS").unwrap_or(0)
}

/// Formats seconds as `3d 4h 05m` or `4h 05m` or `5m`.
pub fn format_duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0) as u64;
    let (days, hours, minutes) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes:02}m"),
        _ => format!("{days}d {hours}h {minutes:02}m"),
    }
}

/// Formats KiB as MiB or GiB.
pub fn format_kib(kib: u64) -> String {
    if kib >= 1024 * 1024 {
        format!("{:.1} GiB", kib as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.0} MiB", kib as f64 / 1024.0)
    }
}
