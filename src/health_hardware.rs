//! Host hardware aggregates. Raw tool output stays in bounded memory only.
use crate::{health_consent::Category, health_metrics::Metric};
use serde_json::Value;
use std::{
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const LIMIT: usize = 65536;
static NULL: Value = Value::Null;
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Jetson,
    Nvidia,
    Amd,
    Board,
    Disk,
}
#[derive(Clone, Copy)]
pub enum Reading {
    Available(bool),
    Temperature(i16),
    Power(u32),
    Healthy(bool),
}
fn unavailable() -> io::Error {
    io::Error::other("hardware source unavailable")
}
impl Source {
    pub(crate) fn fields(
        self,
        value: Reading,
    ) -> io::Result<(Category, &'static str, &'static str, i64)> {
        let names = match self {
            Self::Jetson => [
                "host.jetson.available",
                "host.jetson.temperature",
                "host.jetson.power",
                "",
            ],
            Self::Nvidia => [
                "host.nvidia.available",
                "host.nvidia.temperature",
                "host.nvidia.power",
                "",
            ],
            Self::Amd => [
                "host.amd.available",
                "host.amd.temperature",
                "host.amd.power",
                "",
            ],
            Self::Board => ["host.board.available", "host.board.temperature", "", ""],
            Self::Disk => [
                "host.disk.available",
                "host.disk.temperature",
                "",
                "host.disk.healthy",
            ],
        };
        let (index, unit, number) = match value {
            Reading::Available(v) => (0, "1", i64::from(v)),
            Reading::Temperature(v) if (-100..=200).contains(&v) => (1, "Cel", i64::from(v)),
            Reading::Power(v) if v <= 20000 => (2, "W", i64::from(v)),
            Reading::Healthy(v) => (3, "1", i64::from(v)),
            _ => return Err(unavailable()),
        };
        if names[index].is_empty() {
            return Err(unavailable());
        }
        Ok((Category::Sensors, names[index], unit, number))
    }
}
#[derive(Default)]
struct Sample {
    temperature: Option<i16>,
    power: Option<u32>,
    healthy: Option<bool>,
}
impl Sample {
    fn present(&self) -> bool {
        self.temperature.is_some() || self.power.is_some() || self.healthy.is_some()
    }
    fn metrics(self, source: Source) -> Vec<Metric> {
        let mut out = vec![Metric::Hardware(source, Reading::Available(self.present()))];
        if let Some(v) = self.temperature {
            out.push(Metric::Hardware(source, Reading::Temperature(v)));
        }
        if let Some(v) = self.power {
            out.push(Metric::Hardware(source, Reading::Power(v)));
        }
        if let Some(v) = self.healthy {
            out.push(Metric::Hardware(source, Reading::Healthy(v)));
        }
        out
    }
    fn merge(&mut self, other: Self) {
        if let Some(v) = other.temperature {
            self.temperature = Some(self.temperature.map_or(v, |old| old.max(v)));
        }
        if let Some(v) = other.power {
            self.power = Some(self.power.map_or(v, |old| old.max(v)));
        }
        if let Some(v) = other.healthy {
            self.healthy = Some(self.healthy.unwrap_or(true) && v);
        }
    }
}
fn number(text: &str, low: f64, high: f64) -> Option<f64> {
    let value: f64 = text.parse().ok()?;
    (value.is_finite() && (low..=high).contains(&value)).then_some(value)
}
fn temperature(text: &str) -> Option<i16> {
    number(text, -100.0, 200.0).map(|v| v.round() as i16)
}
fn text(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > LIMIT {
        None
    } else {
        std::str::from_utf8(bytes).ok()
    }
}
fn jetson(bytes: &[u8]) -> Sample {
    let Some(line) = text(bytes) else {
        return Sample::default();
    };
    let mut out = Sample::default();
    let words: Vec<_> = line.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        if let Some((label, value)) = word.split_once('@') {
            if matches!(
                label,
                "CPU" | "GPU" | "AO" | "thermal" | "cpu" | "gpu" | "soc0" | "soc1" | "soc2" | "tj"
            ) {
                if let Some(v) = value.strip_suffix('C').and_then(temperature) {
                    out.temperature = Some(out.temperature.map_or(v, |old| old.max(v)));
                }
            }
        }
        if matches!(*word, "VDD_IN" | "POM_5V_IN") {
            out.power = words
                .get(index + 1)
                .and_then(|v| v.split('/').next())
                .and_then(|v| number(v.trim_end_matches("mW"), 0.0, 20_000_000.0))
                .map(|v| (v / 1000.0).round() as u32);
        }
    }
    out
}
fn dcgm(bytes: &[u8]) -> Sample {
    let Some(text) = text(bytes) else {
        return Sample::default();
    };
    let mut out = Sample::default();
    let mut rows = 0;
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.first() != Some(&"GPU") {
            continue;
        }
        if words.len() != 4 || words[1].parse::<u32>().is_err() {
            continue;
        }
        rows += 1;
        if rows > 32 {
            return Sample::default();
        }
        out.merge(Sample {
            temperature: temperature(words[2]),
            power: number(words[3], 0.0, 20000.0).map(|v| v.round() as u32),
            healthy: None,
        });
    }
    out
}
fn json(bytes: &[u8]) -> Option<Value> {
    if bytes.len() > LIMIT {
        None
    } else {
        serde_json::from_slice(bytes).ok()
    }
}
fn scalar(value: &Value, units: &[&str], low: f64, high: f64) -> Option<f64> {
    if value.is_object() {
        let unit = value
            .get("unit")
            .or_else(|| value.get("UNIT"))
            .and_then(Value::as_str)?;
        if !units.contains(&unit) {
            return None;
        }
        let number = value.get("value").or_else(|| value.get("VALUE"))?;
        let v = number.as_f64()?;
        return (v.is_finite() && (low..=high).contains(&v)).then_some(v);
    }
    if let Some(v) = value.as_f64() {
        return (v.is_finite() && (low..=high).contains(&v)).then_some(v);
    }
    let value = value.as_str()?;
    let mut words = value.split_whitespace();
    let n = words.next()?;
    if let Some(unit) = words.next() {
        if !units.contains(&unit) || words.next().is_some() {
            return None;
        }
    }
    number(n, low, high)
}
fn field<'a>(value: &'a Value, lower: &str, upper: &str) -> &'a Value {
    value
        .get(lower)
        .or_else(|| value.get(upper))
        .unwrap_or(&NULL)
}
fn amd(bytes: &[u8]) -> Sample {
    let Some(value) = json(bytes) else {
        return Sample::default();
    };
    let Some(rows) = value.as_array() else {
        return Sample::default();
    };
    if rows.len() > 32 {
        return Sample::default();
    }
    let mut out = Sample::default();
    for row in rows {
        let temp = [("edge", "EDGE"), ("hotspot", "HOTSPOT"), ("mem", "MEM")]
            .iter()
            .filter_map(|(lower, upper)| {
                scalar(
                    field(field(row, "temperature", "TEMPERATURE"), lower, upper),
                    &["C", "°C"],
                    -100.0,
                    200.0,
                )
            })
            .map(|v| v.round() as i16)
            .max();
        let power = scalar(
            field(field(row, "power", "POWER"), "socket_power", "SOCKET_POWER"),
            &["W"],
            0.0,
            20000.0,
        )
        .map(|v| v.round() as u32);
        out.merge(Sample {
            temperature: temp,
            power,
            healthy: None,
        });
    }
    out
}
fn smart(bytes: &[u8], exit: i32) -> Sample {
    // smartctl uses a health bitmask: low three bits mean the command failed.
    if exit < 0 || exit & 7 != 0 {
        return Sample::default();
    }
    let Some(value) = json(bytes) else {
        return Sample::default();
    };
    let Some(healthy) = value["smart_status"]["passed"].as_bool() else {
        return Sample::default();
    };
    let temp = value["temperature"]["current"]
        .as_i64()
        .filter(|v| (-100..=200).contains(v))
        .map(|v| v as i16);
    Sample {
        temperature: temp,
        power: None,
        healthy: Some(healthy && exit & 8 == 0),
    }
}
fn trusted_executable(path: &Path) -> io::Result<PathBuf> {
    let path = path.canonicalize()?;
    for ancestor in path.ancestors() {
        let meta = std::fs::symlink_metadata(ancestor)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(unavailable());
        }
    }
    let meta = std::fs::metadata(&path)?;
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return Err(unavailable());
    }
    Ok(path)
}
fn command(
    path: &str,
    args: &[String],
    deadline: Instant,
    first_line: bool,
) -> io::Result<(Vec<u8>, i32)> {
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    let executable = trusted_executable(Path::new(path))?;
    let mut child = Command::new(executable)
        .args(args)
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let mut reaped = false;
    let result = (|| {
        let mut stdout = child.stdout.take().ok_or_else(unavailable)?;
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            match stdout.read(&mut buffer) {
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.len() > LIMIT {
                        return Err(unavailable());
                    }
                    if first_line {
                        if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
                            bytes.truncate(end + 1);
                            return Ok((bytes, 0));
                        }
                    }
                    if n == 0 {
                        if let Some(status) = child.try_wait()? {
                            reaped = true;
                            return Ok((bytes, status.code().unwrap_or(-1)));
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(unavailable()),
            }
            if Instant::now() >= deadline {
                return Err(unavailable());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    // Kill only this invocation, never tegrastats --stop or a machine-wide daemon.
    if !reaped {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    result
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
fn board(deadline: Instant) -> Sample {
    let Ok(entries) = std::fs::read_dir("/sys/class/thermal") else {
        return Sample::default();
    };
    let mut paths = Vec::new();
    for entry in entries.take(64).flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("thermal_zone")
        {
            continue;
        }
        if let Ok(path) = entry.path().join("temp").canonicalize() {
            if path.starts_with("/sys/devices") {
                paths.push(path.to_string_lossy().into_owned());
            }
        }
    }
    if paths.is_empty() || paths.len() > 32 {
        return Sample::default();
    }
    let Ok((bytes, 0)) = command("/bin/cat", &paths, deadline, false) else {
        return Sample::default();
    };
    board_values(&bytes)
}
fn board_values(bytes: &[u8]) -> Sample {
    let temp = text(bytes)
        .into_iter()
        .flat_map(str::lines)
        .filter_map(|v| number(v, -100000.0, 200000.0))
        .map(|v| (v / 1000.0).round() as i16)
        .max();
    Sample {
        temperature: temp,
        ..Default::default()
    }
}
fn disk_path(path: &Path) -> bool {
    path.parent() == Some(Path::new("/dev"))
        && path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| {
                !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_alphanumeric())
                    && (name.starts_with("sd")
                        || name.starts_with("nvme")
                        || name.starts_with("mmcblk"))
            })
        && std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.uid() == 0 && m.file_type().is_block_device())
}
/// Called only inside the host agreement-authorized sample closure.
pub(crate) fn sample(devices: &[PathBuf]) -> Vec<Metric> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let bounded = || deadline.min(Instant::now() + Duration::from_secs(2));
    let mut out = Vec::new();
    for (source, path, argv, first) in [
        (
            Source::Jetson,
            "/usr/bin/tegrastats",
            args(&["--interval", "100"]),
            true,
        ),
        (
            Source::Nvidia,
            "/usr/bin/dcgmi",
            args(&["dmon", "-e", "150,155", "-c", "1"]),
            false,
        ),
        (
            Source::Amd,
            "/opt/rocm/bin/amd-smi",
            args(&["metric", "--temperature", "--power", "--json"]),
            false,
        ),
    ] {
        let value = match command(path, &argv, bounded(), first) {
            Ok((bytes, 0)) => match source {
                Source::Jetson => jetson(&bytes),
                Source::Nvidia => dcgm(&bytes),
                Source::Amd => amd(&bytes),
                _ => Sample::default(),
            },
            _ => Sample::default(),
        };
        out.extend(value.metrics(source));
    }
    out.extend(board(bounded()).metrics(Source::Board));
    let mut disks = Sample::default();
    let mut complete = !devices.is_empty() && devices.len() <= 8;
    if complete {
        for path in devices {
            if !disk_path(path) {
                complete = false;
                break;
            }
            let argv = args(&[
                "--json=c",
                "--health",
                "--attributes",
                path.to_str().unwrap_or(""),
            ]);
            let value = command("/usr/sbin/smartctl", &argv, bounded(), false)
                .map(|(bytes, code)| smart(&bytes, code))
                .unwrap_or_default();
            if value.healthy.is_none() {
                complete = false;
            }
            disks.merge(value);
        }
    }
    if !complete {
        disks = Sample::default();
    }
    out.extend(disks.metrics(Source::Disk));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_metrics;
    fn projection(sample: Sample, source: Source) -> Value {
        serde_json::from_slice(&health_metrics::encode_host(sample.metrics(source)).unwrap())
            .unwrap()
    }
    #[test]
    fn jetson_units_are_projected_without_board_labels() {
        let value = jetson(
            b"RAM 123/8192MB GR3D_FREQ 10% cpu@42.5C gpu@51C soc0@48C VDD_IN 12345mW/12000mW\n",
        );
        assert_eq!(value.temperature, Some(51));
        assert_eq!(value.power, Some(12));
        assert!(!projection(value, Source::Jetson)
            .to_string()
            .contains("GR3D"));
    }
    #[test]
    fn dcgm_fields_have_fixed_order_and_unavailable_is_not_zero() {
        let sample =
            dcgm(b"# Entity GPU_TEMP POWER_USAGE\nGPU 0 45 120.5\nGPU 1 N/A N/A\nGPU 2 55 130.0\n");
        assert_eq!(sample.temperature, Some(55));
        assert_eq!(sample.power, Some(130));
        assert!(!dcgm(b"GPU 0 N/A N/A\n").present());
        assert!(!dcgm(b"GPU 0 NaN inf\n").present());
    }
    #[test]
    fn amd_structured_units_and_legacy_values_are_allowlisted() {
        let value=amd(br#"[{"gpu":0,"temperature":{"edge":{"value":45,"unit":"C"},"hotspot":{"value":55,"unit":"C"}},"power":{"socket_power":{"value":120.2,"unit":"W"}},"serial_number":"private-serial-canary"}]"#);
        assert_eq!(value.temperature, Some(55));
        assert_eq!(value.power, Some(120));
        assert!(!projection(value, Source::Amd)
            .to_string()
            .contains("private-serial-canary"));
        assert_eq!(
            amd(br#"[{"TEMPERATURE":{"HOTSPOT":"60 C"},"POWER":{"SOCKET_POWER":"150 W"}}]"#)
                .temperature,
            Some(60)
        );
        assert!(!amd(
            br#"[{"temperature":{"edge":{"value":45,"unit":"F"}},"power":{"socket_power":"N/A"}}]"#
        )
        .present());
    }
    #[test]
    fn smart_failure_is_real_false_and_serials_never_reach_metrics() {
        let data=br#"{"serial_number":"private-serial-canary","wwn":{"id":"private-wwn"},"smart_status":{"passed":false},"temperature":{"current":38}}"#;
        let value = smart(data, 8);
        assert_eq!(value.healthy, Some(false));
        assert_eq!(value.temperature, Some(38));
        let body = projection(value, Source::Disk).to_string();
        assert!(!body.contains("private-"));
        assert!(!body.contains("serial"));
        assert!(!body.contains("wwn"));
        for code in [1, 2, 4, -1] {
            assert!(!smart(data, code).present());
        }
        assert!(!smart(br#"{"serial_number":"private-serial-canary"}"#, 0).present());
    }
    #[test]
    fn sysfs_millidegrees_and_missing_sources_are_distinct() {
        assert_eq!(board_values(b"42000\n51000\n").temperature, Some(51));
        assert!(!board_values(b"N/A\n999999999\n").present());
        let body = projection(Sample::default(), Source::Board);
        let metrics = body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0]["name"], "host.board.available");
        assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asInt"], "0");
    }
    #[test]
    fn bounds_missing_tools_and_client_policy_refuse() {
        let oversized = vec![b'0'; LIMIT + 1];
        assert!(!jetson(&oversized).present());
        assert!(!dcgm(&oversized).present());
        assert!(!amd(&oversized).present());
        assert!(!smart(&oversized, 0).present());
        assert!(command(
            "/missing/hardware-tool",
            &[],
            Instant::now() + Duration::from_secs(1),
            false
        )
        .is_err());
        assert!(command("/bin/cat", &[], Instant::now(), false).is_err());
        assert!(health_metrics::encode(
            Category::Sensors,
            Sample::default().metrics(Source::Board),
            "hypermesh-visor"
        )
        .is_err());
        assert!(!disk_path(Path::new("/dev/../etc/passwd")));
    }
}
