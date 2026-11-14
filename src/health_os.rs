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
    collect_with(store, read_source)
}
fn collect_with(
    store: &Store,
    read: impl Fn(Source) -> io::Result<Vec<u8>>,
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
        assert!(collect_with(&store, |_| panic!("off source read"))
            .unwrap()
            .is_none());
        store.decide(Category::OsHealth, true).unwrap();
        let pending = collect_with(&store, |source| match source {
            Source::Memory => {
                Ok(b"MemTotal: 100 kB\nMemAvailable: 75 kB\nignored: private-text".to_vec())
            }
            Source::Uptime => Err(unavailable()),
        })
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
        assert!(collect_with(&store, |_| Err(unavailable())).is_err());
        assert!(collect_with(&store, |s| Ok(vec![b'0'; s.limit() + 1])).is_err());
        store.decide(Category::OsHealth, false).unwrap();
        assert!(collect_with(&store, |_| panic!("withdrawn source read"))
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
}
