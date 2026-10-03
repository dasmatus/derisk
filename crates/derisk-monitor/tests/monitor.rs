use std::{fs, path::PathBuf};

use derisk_monitor::{
    MonitorApp, Sampler, SortKey,
    proc::{
        format_duration, format_kib, parse_cpu_count, parse_loadavg, parse_meminfo, parse_pid_rss,
        parse_pid_stat, parse_stat, parse_uptime,
    },
};
use mcsapi_ui::{Theme, egui, run_frame};

struct FakeProc(PathBuf);

impl FakeProc {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("derisk-monitor-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meminfo"), MEMINFO).unwrap();
        fs::write(dir.join("loadavg"), "0.50 0.25 0.10 1/99 1234\n").unwrap();
        fs::write(dir.join("uptime"), "93784.5 1000.0\n").unwrap();
        fs::create_dir(dir.join("self")).unwrap();
        Self(dir)
    }

    fn cpu(&self, user: u64, idle: u64) {
        let text = format!("cpu  {user} 0 0 {idle} 0 0 0 0 0 0\ncpu0 0 0 0 0\ncpu1 0 0 0 0\n");
        fs::write(self.0.join("stat"), text).unwrap();
    }

    fn process(&self, pid: u32, name: &str, ticks: u64, rss_kib: u64) {
        let dir = self.0.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        let stat = format!("{pid} ({name}) S 1 1 1 0 -1 0 0 0 0 0 {ticks} 0 0 0 20 0 3 0 100 0 0");
        fs::write(dir.join("stat"), stat).unwrap();
        fs::write(
            dir.join("status"),
            format!("Name:\t{name}\nVmRSS:\t{rss_kib} kB\n"),
        )
        .unwrap();
    }
}

impl Drop for FakeProc {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const MEMINFO: &str = "MemTotal:       16000000 kB
MemFree:         1000000 kB
MemAvailable:   12000000 kB
SwapTotal:       2000000 kB
SwapFree:        1500000 kB
";

#[test]
fn parses_proc_files() {
    let a = parse_stat("cpu  1 2 3 4 5 6 7 8 9 10\n").unwrap();
    assert_eq!((a.total, a.idle), (36, 9));
    assert!(parse_stat("intr 1 2\n").is_none());
    assert_eq!(
        parse_cpu_count("cpu  1\ncpu0 1\ncpu1 1\ncpufreq 1\nintr 2\n"),
        2
    );
    let memory = parse_meminfo(MEMINFO).unwrap();
    assert_eq!((memory.used(), memory.swap_used()), (4_000_000, 500_000));
    let old_kernel = parse_meminfo("MemTotal: 100 kB\nMemFree: 40 kB\n").unwrap();
    assert_eq!(old_kernel.available, 40);
    assert_eq!(
        parse_loadavg("1.00 0.50 0.25 2/300 99"),
        Some([1.0, 0.5, 0.25])
    );
    assert_eq!(parse_uptime("12.5 3.0"), Some(12.5));
    assert!(parse_pid_stat("garbage").is_none());
    assert_eq!(parse_pid_rss("Name: kthreadd\n"), 0);
    assert_eq!(parse_pid_rss("VmRSS:\t  2048 kB\n"), 2048);
}

#[test]
fn formats_durations_and_sizes() {
    assert_eq!(format_duration(59.0), "0m");
    assert_eq!(format_duration(3_900.0), "1h 05m");
    assert_eq!(format_duration(93_784.5), "1d 2h 03m");
    assert_eq!(format_kib(2048), "2 MiB");
    assert_eq!(format_kib(3 * 1024 * 1024), "3.0 GiB");
}

#[test]
fn samples_turn_counters_into_rates() {
    let proc = FakeProc::new("rates");
    proc.cpu(100, 900);
    proc.process(1, "init", 10, 1024);
    proc.process(42, "busy (worker)", 50, 4096);
    let mut sampler = Sampler::new(&proc.0);
    let first = sampler.sample();
    assert_eq!(first.cpu, 0.0, "no rate without a previous sample");
    assert_eq!(first.cpus, 2);
    assert_eq!(first.processes.len(), 2, "non-numeric entries are skipped");
    assert_eq!(first.load, [0.5, 0.25, 0.1]);

    proc.cpu(160, 940); // 60 busy of 100 ticks
    proc.process(42, "busy (worker)", 80, 4096); // 30 of 100 ticks
    proc.process(7, "new", 5, 0);
    let second = sampler.sample();
    assert_eq!(second.cpu, 60.0);
    let busy = second.processes.iter().find(|p| p.pid == 42).unwrap();
    assert_eq!(
        (busy.name.as_str(), busy.cpu, busy.rss, busy.threads),
        ("busy (worker)", 30.0, 4096, 3)
    );
    let new = second.processes.iter().find(|p| p.pid == 7).unwrap();
    assert_eq!(new.cpu, 0.0, "new processes have no rate yet");
    assert_eq!(
        sampler.history.iter().copied().collect::<Vec<_>>(),
        [0.0, 60.0]
    );
}

#[test]
fn app_sorts_and_filters_rows() {
    let proc = FakeProc::new("rows");
    proc.cpu(0, 0);
    proc.process(1, "init", 0, 1000);
    proc.process(2, "Browser", 0, 9000);
    proc.process(3, "agent", 0, 5000);
    let mut app = MonitorApp::new(Sampler::new(&proc.0));
    app.refresh();
    app.sort = SortKey::Memory;
    let pids: Vec<u32> = app.rows().iter().map(|p| p.pid).collect();
    assert_eq!(pids, [2, 3, 1]);
    app.sort = SortKey::Name;
    let pids: Vec<u32> = app.rows().iter().map(|p| p.pid).collect();
    assert_eq!(pids, [3, 2, 1]);
    let mut output = run_frame(
        &mut app,
        &egui::Context::default(),
        egui::RawInput::default(),
        &Theme::default(),
    );
    assert!(!output.shapes.is_empty());
    output.textures_delta.clear();
}
