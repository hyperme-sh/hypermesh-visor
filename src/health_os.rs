//! Minimal Linux aggregate sampling. No process, hostname or disk identifiers.
use crate::{
    health_consent::{Category, Store},
    health_metrics::{Metric, PendingMetrics},
};
use std::io;

#[derive(Clone, Copy)]
enum Source {
    Memory,
    Uptime,
}
impl Source {
    fn limit(self) -> usize {
        match self {
            Self::Memory => 16_384,
            Self::Uptime => 128,
        }
    }
}
fn unavailable() -> io::Error {
    io::Error::other("OS health source unavailable")
}

/// Off consent performs no source reads. Missing values are omitted, never zeroed.
/// This is a Visor entry point; host agreement policy is a separate #217 owner.
pub fn collect(store: &Store) -> io::Result<Option<PendingMetrics>> {
    collect_with(store, read_source, read_cpu, read_disk)
}
fn collect_with(
    store: &Store,
    read: impl Fn(Source) -> io::Result<Vec<u8>>,
    cpu: impl FnOnce() -> io::Result<u8>,
    disk: impl FnOnce() -> io::Result<u8>,
) -> io::Result<Option<PendingMetrics>> {
    PendingMetrics::collect(store, Category::OsHealth, || {
        let mut metrics = Vec::new();
        for source in [Source::Memory, Source::Uptime] {
            if let Ok(bytes) = read(source) {
                if bytes.len() <= source.limit() {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        let metric = match source {
                            Source::Memory => memory(text).map(Metric::MemoryPercent),
                            Source::Uptime => uptime(text).map(Metric::UptimeHours),
                        };
                        if let Some(metric) = metric {
                            metrics.push(metric);
                        }
                    }
                }
            }
        }
        if let Ok(percent) = cpu() {
            metrics.push(Metric::CpuPercent(percent));
        }
        if let Ok(percent) = disk() {
            metrics.push(Metric::DiskPercent(percent));
        }
        if metrics.is_empty() {
            Err(unavailable())
        } else {
            Ok(metrics)
        }
    })
}
#[cfg(target_os = "linux")]
fn read_source(source: Source) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let path = match source {
        Source::Memory => "/proc/meminfo",
        Source::Uptime => "/proc/uptime",
    };
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| unavailable())?
        .take((source.limit() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > source.limit() {
        return Err(unavailable());
    }
    Ok(bytes)
}
#[cfg(not(target_os = "linux"))]
fn read_source(_: Source) -> io::Result<Vec<u8>> {
    Err(unavailable())
}

// Only the aggregate first line is read. Per-CPU/process counters and labels
// never enter the sample. Guest counters already contribute to user/nice.
fn cpu_counters(bytes: &[u8]) -> Option<[u64; 8]> {
    if bytes.len() > 1024 || !bytes.ends_with(b"\n") {
        return None;
    }
    let mut words = std::str::from_utf8(bytes).ok()?.split_ascii_whitespace();
    if words.next()? != "cpu" {
        return None;
    }
    let mut counters = [0; 8];
    for counter in &mut counters {
        let word = words.next()?;
        if !word.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *counter = word.parse().ok()?;
    }
    // Modern Linux supplies two guest fields. Validate but do not double count.
    for _ in 0..2 {
        let word = words.next()?;
        if !word.bytes().all(|b| b.is_ascii_digit()) || word.parse::<u64>().is_err() {
            return None;
        }
    }
    words.next().is_none().then_some(counters)
}
fn cpu_percent(before: [u64; 8], after: [u64; 8]) -> Option<u8> {
    let mut total = 0u128;
    let mut idle = 0u128;
    for (index, (before, after)) in before.into_iter().zip(after).enumerate() {
        // Reset, hotplug and decreasing iowait are unavailable for this window.
        let delta = after.checked_sub(before)? as u128;
        total += delta;
        if index == 3 || index == 4 {
            idle += delta;
        }
    }
    (total != 0).then(|| (((total - idle) * 100) / total) as u8)
}
fn disk_percent(blocks: u64, free: u64) -> Option<u8> {
    if blocks == 0 || free > blocks {
        return None;
    }
    Some((((blocks - free) as u128 * 100) / blocks as u128) as u8)
}
#[cfg(target_os = "linux")]
fn read_cpu() -> io::Result<u8> {
    use std::io::{BufRead, BufReader, Read};
    fn snapshot() -> io::Result<[u64; 8]> {
        let file = std::fs::File::open("/proc/stat").map_err(|_| unavailable())?;
        let mut bytes = Vec::new();
        BufReader::new(file.take(1025))
            .read_until(b'\n', &mut bytes)
            .map_err(|_| unavailable())?;
        cpu_counters(&bytes).ok_or_else(unavailable)
    }
    let before = snapshot()?;
    // One short window, no persistent baseline, background task or retry. The
    // caller holds current OS consent throughout both reads and this delay.
    std::thread::sleep(std::time::Duration::from_millis(100));
    cpu_percent(before, snapshot()?).ok_or_else(unavailable)
}
#[cfg(target_os = "linux")]
fn read_disk() -> io::Result<u8> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: fixed NUL-terminated path and a valid writable output structure.
    if unsafe { libc::statvfs(c"/".as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(unavailable());
    }
    // SAFETY: statvfs initialized the structure on success.
    let stats = unsafe { stats.assume_init() };
    disk_percent(stats.f_blocks as u64, stats.f_bfree as u64).ok_or_else(unavailable)
}
#[cfg(not(target_os = "linux"))]
fn read_cpu() -> io::Result<u8> {
    Err(unavailable())
}
#[cfg(not(target_os = "linux"))]
fn read_disk() -> io::Result<u8> {
    Err(unavailable())
}

fn memory(text: &str) -> Option<u8> {
    let (mut total, mut available) = (None, None);
    for line in text.lines() {
        let mut words = line.split_ascii_whitespace();
        let target = match words.next()? {
            "MemTotal:" => &mut total,
            "MemAvailable:" => &mut available,
            _ => continue,
        };
        if target.is_some() {
            return None;
        }
        let word = words.next()?;
        if !word.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value: u64 = word.parse().ok()?;
        if words.next()? != "kB" || words.next().is_some() {
            return None;
        }
        *target = Some(value);
    }
    let (total, available) = (total?, available?);
    if total == 0 || available > total {
        return None;
    }
    Some((((total - available) as u128 * 100) / total as u128) as u8)
}
fn uptime(text: &str) -> Option<u32> {
    let first = text.split_ascii_whitespace().next()?;
    let (seconds, fraction) = first.split_once('.')?;
    if seconds.is_empty()
        || fraction.is_empty()
        || !seconds
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hours = seconds.parse::<u64>().ok()? / 3600;
    (hours <= 876_000).then_some(hours as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::Scope;
    use uuid::Uuid;
    #[test]
    fn cpu_window_excludes_guest_and_refuses_reset_or_empty_windows() {
        let before = cpu_counters(b"cpu 100 0 100 600 100 0 0 100 90 0\n").unwrap();
        let after = cpu_counters(b"cpu 120 0 120 650 110 0 0 100 110 0\n").unwrap();
        assert_eq!(cpu_percent(before, after), Some(40));
        assert_eq!(cpu_percent(before, before), None);
        assert_eq!(cpu_percent(after, before), None);
        let mut decreasing_iowait = after;
        decreasing_iowait[4] = 99;
        assert_eq!(cpu_percent(before, decreasing_iowait), None);
        assert_eq!(cpu_percent([0; 8], [u64::MAX; 8]), Some(75));
        assert_eq!(cpu_percent([0; 8], [0, 0, 0, 10, 0, 0, 0, 0]), Some(0));
        assert_eq!(cpu_percent([0; 8], [10, 0, 0, 0, 0, 0, 0, 0]), Some(100));
        for bytes in [
            b"cpu0 1 2 3 4 5 6 7 8 9 10\n".as_slice(),
            b"cpu 1 2 3 4 5 6 7 8 9 10",
            b"cpu 1 2 3 4 5 6 7 8\n",
            b"cpu 1 2 3 4 5 6 7 8 9 10 private-text\n",
            b"cpu -1 2 3 4 5 6 7 8 9 10\n",
            b"cpu 1 2 3 4 5 6 7 8 9 NaN\n",
            b"cpu 18446744073709551616 2 3 4 5 6 7 8 9 10\n",
        ] {
            assert!(cpu_counters(bytes).is_none());
        }
        assert!(cpu_counters(&vec![b'0'; 1025]).is_none());
    }
    #[test]
    fn filesystem_projection_uses_free_blocks_without_identifiers() {
        assert_eq!(disk_percent(100, 25), Some(75));
        assert_eq!(disk_percent(3, 2), Some(33));
        assert_eq!(disk_percent(100, 100), Some(0));
        assert_eq!(disk_percent(u64::MAX, 0), Some(100));
        assert_eq!(disk_percent(0, 0), None);
        assert_eq!(disk_percent(10, 11), None);
    }
    #[test]
    fn aggregate_projection_rejects_missing_invalid_and_duplicate_values() {
        assert_eq!(
            memory("MemTotal: 1000 kB\nMemAvailable: 750 kB\nIgnored: private-text"),
            Some(25)
        );
        for text in [
            "MemTotal: 1000 kB",
            "MemTotal: 0 kB\nMemAvailable: 0 kB",
            "MemTotal: 10 kB\nMemAvailable: 11 kB",
            "MemTotal: 10 MB\nMemAvailable: 1 kB",
            "MemTotal: 10 kB\nMemAvailable: 1 kB\nMemTotal: 10 kB",
        ] {
            assert_eq!(memory(text), None);
        }
        assert_eq!(uptime("7199.99 8000.00"), Some(1));
        for text in [
            "NaN",
            "-10.0",
            "1e4.0",
            "999999999999999999999.00",
            "3153603600.00",
        ] {
            assert_eq!(uptime(text), None);
        }
    }
    #[test]
    fn consent_precedes_reads_and_unavailable_sources_are_not_zeroes() {
        let path = std::env::temp_dir().join(format!("visor-os-unit-{}", Uuid::new_v4()));
        let store = Store::open(
            &path,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap();
        assert!(collect_with(
            &store,
            |_| panic!("off source read"),
            || panic!("off CPU"),
            || panic!("off disk")
        )
        .unwrap()
        .is_none());
        store.decide(Category::OsHealth, true).unwrap();
        let pending = collect_with(
            &store,
            |source| match source {
                Source::Memory => {
                    Ok(b"MemTotal: 100 kB\nMemAvailable: 75 kB\nignored: private-text".to_vec())
                }
                Source::Uptime => Err(unavailable()),
            },
            || Err(unavailable()),
            || Err(unavailable()),
        )
        .unwrap()
        .unwrap();
        pending
            .send_once(&store, |body| {
                let text = std::str::from_utf8(body).unwrap();
                assert!(text.contains("visor.os.memory.utilization"));
                assert!(!text.contains("uptime"));
                assert!(!text.contains("private-text"));
                Ok(())
            })
            .unwrap();
        // CPU/disk still produce a usable batch if the other sources fail.
        let pending = collect_with(&store, |_| Err(unavailable()), || Ok(40), || Ok(75))
            .unwrap()
            .unwrap();
        pending
            .send_once(&store, |body| {
                let value: serde_json::Value = serde_json::from_slice(body).unwrap();
                let metrics = value["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
                    .as_array()
                    .unwrap();
                assert_eq!(metrics.len(), 2);
                assert_eq!(metrics[0]["name"], "visor.os.cpu.utilization");
                assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asInt"], "40");
                assert_eq!(metrics[1]["name"], "visor.os.disk.utilization");
                assert_eq!(metrics[1]["gauge"]["dataPoints"][0]["asInt"], "75");
                assert!(metrics
                    .iter()
                    .all(|m| m["gauge"]["dataPoints"][0].get("attributes").is_none()));
                Ok(())
            })
            .unwrap();
        let stale = collect_with(&store, |_| Err(unavailable()), || Ok(40), || Ok(75))
            .unwrap()
            .unwrap();
        store.decide(Category::OsHealth, false).unwrap();
        store.decide(Category::OsHealth, true).unwrap();
        assert!(stale
            .send_once(&store, |_| panic!("old sample uploaded"))
            .is_err());
        assert!(collect_with(
            &store,
            |_| Err(unavailable()),
            || Err(unavailable()),
            || Err(unavailable())
        )
        .is_err());
        assert!(collect_with(
            &store,
            |s| Ok(vec![b'0'; s.limit() + 1]),
            || Err(unavailable()),
            || Err(unavailable())
        )
        .is_err());
        store.decide(Category::OsHealth, false).unwrap();
        assert!(collect_with(
            &store,
            |_| panic!("withdrawn source read"),
            || panic!("withdrawn CPU"),
            || panic!("withdrawn disk")
        )
        .unwrap()
        .is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
}
