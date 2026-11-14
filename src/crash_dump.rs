//! Local minidump reduction. Raw bytes never cross the returned value boundary.
use hypermesh_local_storage as storage;
use std::{
    fs,
    io::{self, Read},
    path::Path,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashCode {
    InvalidMemoryAccess,
    IllegalInstruction,
    ArithmeticFault,
    NativeCrash,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "local crash dump rejected")
}
fn word(data: &[u8], offset: usize) -> io::Result<u32> {
    let bytes = data
        .get(offset..offset.checked_add(4).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| invalid())?))
}
fn region(data: &[u8], start: usize, size: usize) -> io::Result<&[u8]> {
    data.get(start..start.checked_add(size).ok_or_else(invalid)?)
        .ok_or_else(invalid)
}

/// Read only the exception code; never decode strings, stack memory, modules,
/// register contents, environment, comments or attached files. Unknown codes
/// produce a fixed classification rather than echoing raw numeric/text values.
fn classify(data: &[u8]) -> io::Result<CrashCode> {
    if data.len() < 32
        || data.len() as u64 > MAX_BYTES
        || word(data, 0)? != 0x504d444d
        || word(data, 4)? & 0xffff != 0xa793
    {
        return Err(invalid());
    }
    let count = word(data, 8)? as usize;
    if count == 0 || count > 128 {
        return Err(invalid());
    }
    let directory = word(data, 12)? as usize;
    if directory < 32 {
        return Err(invalid());
    }
    let entries = region(data, directory, count * 12)?;
    let mut exception = None;
    for entry in entries.chunks_exact(12) {
        let kind = word(entry, 0)?;
        let size = word(entry, 4)? as usize;
        let offset = word(entry, 8)? as usize;
        let stream = region(data, offset, size)?;
        if kind == 6 {
            let overlaps_directory = offset < directory + count * 12 && offset + size > directory;
            if exception.is_some() || size < 168 || offset < 32 || overlaps_directory {
                return Err(invalid());
            }
            exception = Some(word(stream, 8)?);
        }
    }
    Ok(match exception.ok_or_else(invalid)? {
        0xc0000005 => CrashCode::InvalidMemoryAccess,
        0xc000001d => CrashCode::IllegalInstruction,
        0xc0000094 | 0xc000008e => CrashCode::ArithmeticFault,
        _ => CrashCode::NativeCrash,
    })
}

/// Consume one completed dump in an owner-private, adapter-owned directory.
/// The capture adapter must stop its writer before this call and must bind the
/// dump to its original consent ticket; this function grants no upload authority.
/// Only UUID-named .dmp files are addressable, not arbitrary operator paths.
/// Every normal parse/read outcome removes the raw file before returning.
/// Unsafe/linked files are refused without following or deleting their targets.
pub fn consume(directory: &Path, id: Uuid) -> io::Result<CrashCode> {
    reduce(directory, id).map(|dump| dump.code)
}
pub(crate) struct ReducedDump {
    pub code: CrashCode,
    pub captured_at: std::time::SystemTime,
}
pub(crate) fn reduce(directory: &Path, id: Uuid) -> io::Result<ReducedDump> {
    if id.is_nil() {
        return Err(invalid());
    }
    storage::check_dir(directory)?;
    let lock = storage::open(&directory.join("sanitize.lock"), storage::Open::Lock)?;
    storage::lock(&lock)?;
    let _unlock = storage::UnlockOnDrop(&lock);
    let path = directory.join(format!("{id}.dmp"));
    let file = storage::open(&path, storage::Open::Read { private: true })?;
    let result = (|| {
        if file.metadata()?.len() > MAX_BYTES {
            return Err(invalid());
        }
        let mut data = Zeroizing::new(Vec::new());
        file.take(MAX_BYTES + 1).read_to_end(&mut data)?;
        let code = classify(&data)?;
        let captured_at =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(word(&data, 20)? as u64);
        Ok(ReducedDump { code, captured_at })
    })();
    fs::remove_file(&path)?;
    storage::sync_dir(directory)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("visor-dump-{}", Uuid::new_v4()));
            storage::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn put(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture(code: u32) -> Vec<u8> {
        let mut data = vec![0; 44 + 168];
        put(&mut data, 0, 0x504d444d);
        put(&mut data, 4, 0xa793);
        put(&mut data, 8, 1);
        put(&mut data, 12, 32);
        put(
            &mut data,
            20,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32,
        );
        put(&mut data, 32, 6);
        put(&mut data, 36, 168);
        put(&mut data, 40, 44);
        put(&mut data, 52, code);
        data.extend_from_slice(b"secret-prompt secret-file secret-disk-serial");
        data
    }
    fn write(dir: &Path, id: Uuid, bytes: &[u8]) {
        let mut f = storage::open(&dir.join(format!("{id}.dmp")), storage::Open::New).unwrap();
        f.write_all(bytes).unwrap();
    }
    #[test]
    fn raw_content_is_removed_and_only_fixed_code_returns() {
        let dir = Temp::new();
        let id = Uuid::new_v4();
        write(&dir.0, id, &fixture(0xc0000005));
        assert_eq!(consume(&dir.0, id).unwrap(), CrashCode::InvalidMemoryAccess);
        assert!(!dir.0.join(format!("{id}.dmp")).exists());
        assert!(consume(&dir.0, id).is_err());
        assert_eq!(
            classify(&fixture(0xdeadbeef)).unwrap(),
            CrashCode::NativeCrash
        );
    }
    #[test]
    fn malformed_dump_is_also_deleted() {
        let dir = Temp::new();
        let id = Uuid::new_v4();
        write(&dir.0, id, b"private malformed bytes");
        assert!(consume(&dir.0, id).is_err());
        assert!(!dir.0.join(format!("{id}.dmp")).exists());
    }
    #[test]
    fn oversize_dump_is_deleted_without_loading_it() {
        let dir = Temp::new();
        let id = Uuid::new_v4();
        let path = dir.0.join(format!("{id}.dmp"));
        storage::open(&path, storage::Open::New)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(consume(&dir.0, id).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn exception_stream_may_precede_its_directory() {
        let mut data = fixture(0xc0000005);
        data.copy_within(44..212, 32);
        put(&mut data, 12, 200);
        put(&mut data, 200, 6);
        put(&mut data, 204, 168);
        put(&mut data, 208, 32);
        assert_eq!(classify(&data).unwrap(), CrashCode::InvalidMemoryAccess);
        put(&mut data, 204, 169);
        assert!(classify(&data).is_err());
    }
    #[test]
    fn truncated_and_overflowing_directories_are_refused() {
        let valid = fixture(0xc0000005);
        for length in 0..212 {
            assert!(classify(&valid[..length]).is_err());
        }
        for (offset, value) in [
            (8, u32::MAX),
            (12, u32::MAX),
            (36, u32::MAX),
            (40, u32::MAX),
            (36, 8),
            (40, 0),
        ] {
            let mut broken = valid.clone();
            put(&mut broken, offset, value);
            assert!(classify(&broken).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_shared_files_are_not_read_or_deleted() {
        use std::os::unix::fs::symlink;
        let dir = Temp::new();
        let id = Uuid::new_v4();
        let target = dir.0.join("other");
        storage::open(&target, storage::Open::New)
            .unwrap()
            .write_all(b"private")
            .unwrap();
        let path = dir.0.join(format!("{id}.dmp"));
        symlink(&target, &path).unwrap();
        assert!(consume(&dir.0, id).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"private");
        assert!(fs::symlink_metadata(&path).is_ok());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&target, &path).unwrap();
        assert!(consume(&dir.0, id).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"private");
        assert!(path.exists());
    }

    #[test]
    fn recovered_dumps_do_not_receive_a_fresh_retention_window() {
        use crate::{
            crash_report::PendingCrash,
            health_consent::{Category, Scope, Store},
        };
        let dir = Temp::new();
        let consent = Temp::new();
        let store = Store::open(
            &consent.0,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap();
        store.decide(Category::Crashes, true).unwrap();
        for timestamp in [0, u32::MAX] {
            let ticket = store.capture(Category::Crashes).unwrap().unwrap();
            let mut bytes = fixture(0xc0000005);
            put(&mut bytes, 20, timestamp);
            let id = Uuid::new_v4();
            write(&dir.0, id, &bytes);
            assert!(PendingCrash::from_dump(
                &store,
                ticket,
                "https://crashes.example.invalid/api/1/store/",
                &dir.0,
                id
            )
            .is_err());
            assert!(!dir.0.join(format!("{id}.dmp")).exists());
        }
    }
    #[test]
    fn dump_preview_preserves_original_consent_and_requires_approval() {
        use crate::{
            crash_report::PendingCrash,
            health_consent::{Category, Scope, Store},
        };
        let dir = Temp::new();
        let consent = Temp::new();
        let store = Store::open(
            &consent.0,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap();
        let destination = "https://crashes.example.invalid/api/1/store/";
        store.decide(Category::Crashes, true).unwrap();
        let ticket = store.capture(Category::Crashes).unwrap().unwrap();
        let id = Uuid::new_v4();
        write(&dir.0, id, &fixture(0xc0000005));
        let report =
            PendingCrash::from_dump(&store, ticket.clone(), destination, &dir.0, id).unwrap();
        let preview = String::from_utf8_lossy(report.preview().sanitized_json);
        assert!(preview.contains("invalid_memory_access"));
        assert!(!preview.contains("secret-"));
        assert!(!dir.0.join(format!("{id}.dmp")).exists());
        store.synchronized(report.consent_ticket()).unwrap();
        assert!(report
            .send_once(&store, |_, _| panic!("unapproved upload"))
            .is_err());

        store.decide(Category::Crashes, false).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let id = Uuid::new_v4();
        write(&dir.0, id, &fixture(0xc0000005));
        assert!(PendingCrash::from_dump(&store, ticket, destination, &dir.0, id).is_err());
        assert!(!dir.0.join(format!("{id}.dmp")).exists());
    }
}
