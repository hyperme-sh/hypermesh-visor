//! Bounded, typed OTLP metrics. No free-form labels, disk identifiers or queue.
use crate::health_consent::{Category, Store, Ticket};
use serde_json::{json, Value};
use std::{
    io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Fixed names and numeric limits are the local minimization boundary.
#[derive(Clone, Copy)]
pub enum Metric {
    CpuPercent(u8),
    MemoryPercent(u8),
    DiskPercent(u8),
    UptimeHours(u32),
    TemperatureCelsius(i16),
    PowerWatts(u32),
    ServiceFailures(u32),
}
impl Metric {
    fn fields(self) -> io::Result<(Category, &'static str, &'static str, i64)> {
        let fields = match self {
            Self::CpuPercent(n) if n <= 100 => (
                Category::OsHealth,
                "visor.os.cpu.utilization",
                "%",
                n.into(),
            ),
            Self::MemoryPercent(n) if n <= 100 => (
                Category::OsHealth,
                "visor.os.memory.utilization",
                "%",
                n.into(),
            ),
            Self::DiskPercent(n) if n <= 100 => (
                Category::OsHealth,
                "visor.os.disk.utilization",
                "%",
                n.into(),
            ),
            Self::UptimeHours(n) if n <= 876_000 => {
                (Category::OsHealth, "visor.os.uptime", "h", n.into())
            }
            Self::TemperatureCelsius(n) if (-100..=200).contains(&n) => (
                Category::Sensors,
                "visor.sensor.temperature",
                "Cel",
                n.into(),
            ),
            Self::PowerWatts(n) if n <= 20_000 => {
                (Category::Sensors, "visor.sensor.power", "W", n.into())
            }
            Self::ServiceFailures(n) if n <= 1_000_000 => (
                Category::ServiceHealth,
                "visor.service.failures",
                "{failure}",
                n.into(),
            ),
            _ => return Err(refused()),
        };
        Ok(fields)
    }
}
fn refused() -> io::Error {
    io::Error::other("health sample or upload refused")
}

/// One in-memory sample; consumed on every send attempt and never persisted.
pub struct PendingMetrics {
    ticket: Ticket,
    body: Vec<u8>,
    created: Instant,
}
impl PendingMetrics {
    pub fn collect(
        store: &Store,
        category: Category,
        sample: impl FnOnce() -> io::Result<Vec<Metric>>,
    ) -> io::Result<Option<Self>> {
        if !matches!(
            category,
            Category::OsHealth | Category::Sensors | Category::ServiceHealth
        ) {
            return Err(refused());
        }
        let created = Instant::now();
        let Some((ticket, body)) = store.collect(category, || {
            let samples = sample()?;
            if samples.is_empty() || samples.len() > 16 { return Err(refused()); }
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| refused())?.as_nanos().to_string();
            let mut metrics = Vec::new();
            let mut names = std::collections::HashSet::new();
            for metric in samples {
                let (actual, name, unit, value) = metric.fields()?;
                if actual != category || !names.insert(name) { return Err(refused()); }
                metrics.push(json!({"name":name, "unit":unit, "gauge":{"dataPoints":[{"timeUnixNano":timestamp, "asInt":value.to_string()}]}}));
            }
            // Tenant/device identity is assigned by Infra's authenticated ingress.
            // No client-supplied tenant, host, machine, path or process attributes.
            let body: Value = json!({"resourceMetrics":[{
                "resource":{"attributes":[
                    {"key":"service.name", "value":{"stringValue":"hypermesh-visor"}},
                    {"key":"service.version", "value":{"stringValue":env!("CARGO_PKG_VERSION")}}
                ]},
                "scopeMetrics":[{"scope":{"name":"hypermesh.health"}, "metrics":metrics}]
            }]});
            serde_json::to_vec(&body).map_err(|_| refused())
        })? else { return Ok(None); };
        Ok(Some(Self {
            ticket,
            body,
            created,
        }))
    }
    /// The consent owner must confirm this exact revision before transport.
    pub fn consent_ticket(&self) -> &Ticket {
        &self.ticket
    }
    /// Caller supplies the authenticated, synchronous bounded OTLP transport.
    /// No background sender, retry or arbitrary endpoint belongs in this layer.
    pub fn send_once(
        self,
        store: &Store,
        send: impl FnOnce(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.created.elapsed() >= Duration::from_secs(30) {
            return Err(refused());
        }
        store.upload(&self.ticket, || send(&self.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::Scope;
    use uuid::Uuid;
    struct Fixture {
        path: std::path::PathBuf,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("visor-health-metrics-{}", Uuid::new_v4()));
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
            Self { path, store }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn default_off_does_not_invoke_sampler_and_other_categories_do_not_enable_it() {
        let f = Fixture::new();
        f.store.decide(Category::Sensors, true).unwrap();
        assert!(
            PendingMetrics::collect(&f.store, Category::OsHealth, || panic!("off"))
                .unwrap()
                .is_none()
        );
        assert!(
            PendingMetrics::collect(&f.store, Category::Crashes, || panic!("wrong path")).is_err()
        );
    }
    #[test]
    fn withdrawal_during_sample_cannot_report_success_and_old_sample_cannot_send() {
        let f = Fixture::new();
        f.store.decide(Category::OsHealth, true).unwrap();
        let sample = PendingMetrics::collect(&f.store, Category::OsHealth, || {
            assert!(f.store.decide(Category::OsHealth, false).is_err());
            Ok(vec![Metric::CpuPercent(17)])
        })
        .unwrap()
        .unwrap();
        f.store.synchronized(sample.consent_ticket()).unwrap();
        f.store.decide(Category::OsHealth, false).unwrap();
        assert!(sample.send_once(&f.store, |_| panic!("withdrawn")).is_err());
    }
    #[test]
    fn payload_is_fixed_otlp_without_client_identity_or_freeform_data() {
        let f = Fixture::new();
        f.store.decide(Category::OsHealth, true).unwrap();
        let sample = PendingMetrics::collect(&f.store, Category::OsHealth, || {
            Ok(vec![Metric::CpuPercent(12), Metric::UptimeHours(2)])
        })
        .unwrap()
        .unwrap();
        f.store.synchronized(sample.consent_ticket()).unwrap();
        sample
            .send_once(&f.store, |body| {
                let value: Value = serde_json::from_slice(body).unwrap();
                let resource = &value["resourceMetrics"][0];
                assert_eq!(
                    resource["resource"]["attributes"].as_array().unwrap().len(),
                    2
                );
                let metrics = &resource["scopeMetrics"][0]["metrics"];
                assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asInt"], "12");
                assert_eq!(metrics[1]["gauge"]["dataPoints"][0]["asInt"], "2");
                assert!(metrics[0]["gauge"]["dataPoints"][0]
                    .get("attributes")
                    .is_none());
                assert!(String::from_utf8_lossy(body).find("tenant").is_none());
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn hostile_values_duplicates_and_wrong_category_are_refused() {
        let f = Fixture::new();
        f.store.decide(Category::OsHealth, true).unwrap();
        for metrics in [
            vec![Metric::CpuPercent(101)],
            vec![Metric::CpuPercent(1), Metric::CpuPercent(2)],
            vec![Metric::TemperatureCelsius(20)],
            vec![],
        ] {
            assert!(PendingMetrics::collect(&f.store, Category::OsHealth, || Ok(metrics)).is_err());
        }
    }
    #[test]
    fn no_consent_ack_or_expired_sample_cannot_reach_transport() {
        let f = Fixture::new();
        f.store.decide(Category::OsHealth, true).unwrap();
        let make = || {
            PendingMetrics::collect(&f.store, Category::OsHealth, || {
                Ok(vec![Metric::MemoryPercent(50)])
            })
            .unwrap()
            .unwrap()
        };
        assert!(make()
            .send_once(&f.store, |_| panic!("not confirmed"))
            .is_err());
        let mut expired = make();
        f.store.synchronized(expired.consent_ticket()).unwrap();
        expired.created = Instant::now() - Duration::from_secs(31);
        assert!(expired.send_once(&f.store, |_| panic!("expired")).is_err());
    }
}
