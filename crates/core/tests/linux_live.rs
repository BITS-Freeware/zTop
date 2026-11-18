// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live-host checks of the Linux collector, the path the agent ships.
//! The fixture tests exercise the shared parsers through the replay
//! assembler; these run the live assembler on whatever Linux host runs
//! them, with bounds every host satisfies.
#[cfg(target_os = "linux")]
mod live {
    use ztop_core::collector::Collector;
    use ztop_core::linux::LinuxCollector;

    /// Regression: the live loop never read `/proc/<pid>/cmdline`, so
    /// every process had no command line (only the replay path read
    /// it). This test's own process has one.
    #[test]
    fn the_live_collector_reads_command_lines() {
        let sample = LinuxCollector::default().sample().expect("a live sample");
        let me = std::process::id();
        let own = sample
            .processes
            .iter()
            .find(|process| process.pid == me)
            .expect("this test process is listed");
        let command = own.command.as_deref().unwrap_or_default();
        let exe = std::env::current_exe().expect("current exe");
        let exe_name = exe
            .file_name()
            .and_then(|name| name.to_str())
            .expect("exe name");
        assert!(
            command.contains(exe_name),
            "expected the command line to name {exe_name}, got {command:?}"
        );
        assert!(
            sample
                .processes
                .iter()
                .filter(|p| p.command.is_some())
                .count()
                > 1,
            "more than one process should carry a command line"
        );
    }

    /// Regression: mounts were keyed by the `/proc/mounts` source
    /// (`/dev/vda`) but looked up by the `/proc/diskstats` name (`vda`),
    /// so no disk ever had a mount and `statvfs` never ran. Every disk
    /// whose device is mounted (by its `/dev` path, a `/dev/mapper` or
    /// `/dev/disk/by-*` link) must carry a mount point and a capacity.
    #[test]
    fn the_live_collector_fills_mounted_disks() {
        let sample = LinuxCollector::default().sample().expect("a live sample");
        let mounts = std::fs::read_to_string("/proc/mounts").expect("mounts");
        let mut mounted: Vec<String> = mounts
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|source| source.starts_with("/dev/"))
            .filter_map(|source| std::fs::canonicalize(source).ok())
            .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
            .filter(|name| sample.disks.iter().any(|disk| &disk.name == name))
            .collect();
        mounted.sort();
        mounted.dedup();
        if mounted.is_empty() {
            eprintln!("skipping: no /proc/diskstats device is mounted by a /dev path here");
            return;
        }
        for name in &mounted {
            let disk = sample
                .disks
                .iter()
                .find(|disk| &disk.name == name)
                .expect("listed");
            assert!(
                !disk.mount.is_empty(),
                "{name} is mounted but has no mount point"
            );
            assert!(
                disk.total_bytes > 0,
                "{name} is mounted at {} but has no capacity",
                disk.mount
            );
            assert!(disk.used_bytes <= disk.total_bytes);
        }
    }
}
