//! Bounded stdio transport. A caller supplies the independently authorized route.
use super::{Action, Protocol, MAX_FRAME};
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

/// Implementations must bind one assigned machine/session, recheck authorization
/// on every prompt, and use Conductor and AIuditor. They must honor cancellation
/// and bound all I/O; a successful open is not a reusable authorization grant.
/// No production backend is installed by this protocol library.
pub trait Backend: Send + Sync + 'static {
    fn open(&self, cancel: &AtomicBool) -> Result<uuid::Uuid, ()>;
    fn prompt(&self, text: &str, cancel: &AtomicBool) -> Result<String, ()>;
}

enum Completion {
    Open(Value, Result<uuid::Uuid, ()>),
    Prompt(Result<String, ()>),
}
struct Active {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Completion>,
    started: Instant,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn send(output: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(output, "{value}")?;
    output.flush()
}

/// Read only one bounded JSON line; an oversized or invalid UTF-8 frame closes
/// the connection without retaining the remainder of its input.
fn frame(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut value = String::new();
    let count = input.take((MAX_FRAME + 1) as u64).read_line(&mut value)?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_FRAME || !value.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid ACP frame",
        ));
    }
    Ok(Some(value))
}

/// Run one connection without spawning configured commands, opening listeners,
/// reading resource links, or writing prompts to logs. EOF, queue overflow and
/// output failure cancel active work. The owner must supply a bounded backend.
pub fn serve(
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
    backend: Arc<dyn Backend>,
) -> io::Result<()> {
    serve_until(input, &mut output, backend, Duration::from_secs(30))
}

fn serve_until(
    mut input: impl BufRead + Send + 'static,
    output: &mut impl Write,
    backend: Arc<dyn Backend>,
    deadline: Duration,
) -> io::Result<()> {
    let (tx, rx) = mpsc::sync_channel(16);
    let closed = Arc::new(AtomicBool::new(false));
    let reader_closed = closed.clone();
    std::thread::spawn(move || {
        while let Ok(Some(line)) = frame(&mut input) {
            if tx.try_send(line).is_err() {
                break;
            }
        }
        reader_closed.store(true, Ordering::SeqCst);
    });
    let mut protocol = Protocol::default();
    let mut active: Option<Active> = None;
    loop {
        if closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(line) => match protocol.receive(&line) {
                Action::Reply(value) => send(output, &value)?,
                Action::Ignore => {}
                Action::Cancel => {
                    if let Some(job) = &active {
                        job.cancel.store(true, Ordering::SeqCst);
                    }
                }
                action @ (Action::Open { .. } | Action::Prompt { .. }) => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let worker_cancel = cancel.clone();
                    let route = backend.clone();
                    let (tx, result) = mpsc::sync_channel(1);
                    std::thread::spawn(move || {
                        let result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || match action {
                                    Action::Open { id } => {
                                        Completion::Open(id, route.open(&worker_cancel))
                                    }
                                    Action::Prompt { text, .. } => {
                                        Completion::Prompt(route.prompt(&text, &worker_cancel))
                                    }
                                    _ => unreachable!(),
                                },
                            ));
                        if let Ok(result) = result {
                            let _ = tx.send(result);
                        }
                    });
                    active = Some(Active {
                        cancel,
                        result,
                        started: Instant::now(),
                    });
                }
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(job) = &active {
            // A timed-out/failed backend is not reused for a new turn. Closing
            // also avoids pretending an unknown remote outcome was cancelled.
            if job.started.elapsed() >= deadline {
                return Ok(());
            }
            match job.result.try_recv() {
                Ok(Completion::Open(id, result)) => {
                    let action = match result {
                        Ok(session) => protocol.opened(id, session),
                        Err(()) => protocol.open_failed(id),
                    };
                    if let Action::Reply(value) = action {
                        send(output, &value)?;
                    }
                    active = None;
                }
                Ok(Completion::Prompt(result)) => {
                    let cancelled = job.cancel.load(Ordering::SeqCst);
                    for value in protocol.finish(result.as_deref().map_err(|_| ()), cancelled) {
                        send(output, &value)?;
                    }
                    active = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_is_bounded_and_invalid_utf8_is_refused() {
        assert_eq!(
            frame(&mut io::Cursor::new(b"{}\n")).unwrap(),
            Some("{}\n".into())
        );
        assert!(frame(&mut io::Cursor::new(vec![b'x'; MAX_FRAME + 1])).is_err());
        assert!(frame(&mut io::Cursor::new(vec![255, b'\n'])).is_err());
        assert!(frame(&mut io::Cursor::new(b"{}")).is_err());
        assert_eq!(frame(&mut io::Cursor::new(Vec::<u8>::new())).unwrap(), None);
    }

    struct Input(mpsc::Receiver<Vec<u8>>);
    impl Read for Input {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            match self.0.recv() {
                Ok(bytes) => {
                    assert!(bytes.len() <= target.len());
                    target[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Err(_) => Ok(0),
            }
        }
    }
    struct Output(mpsc::Sender<Value>, Vec<u8>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.1.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0
                .send(serde_json::from_slice(&self.1).unwrap())
                .unwrap();
            self.1.clear();
            Ok(())
        }
    }
    struct WaitingBackend(mpsc::Sender<&'static str>);
    impl Backend for WaitingBackend {
        fn open(&self, _: &AtomicBool) -> Result<uuid::Uuid, ()> {
            Ok(uuid::Uuid::nil())
        }
        fn prompt(&self, _: &str, cancel: &AtomicBool) -> Result<String, ()> {
            self.0.send("started").unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !cancel.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            if cancel.load(Ordering::SeqCst) {
                self.0.send("cancelled").unwrap();
            }
            Ok("late output".into())
        }
    }
    #[test]
    fn cancellation_and_disconnect_reach_the_active_backend() {
        for disconnect in [false, true] {
            let (input, input_rx) = mpsc::channel();
            let (output_tx, output) = mpsc::channel();
            let (events_tx, events) = mpsc::channel();
            let runner = std::thread::spawn(move || {
                serve(
                    io::BufReader::new(Input(input_rx)),
                    Output(output_tx, Vec::new()),
                    Arc::new(WaitingBackend(events_tx)),
                )
            });
            let put = |id: Option<u32>, method: &str, params: Value| {
                let mut v = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params});
                if let Some(id) = id {
                    v["id"] = id.into();
                }
                input.send(format!("{v}\n").into_bytes()).unwrap();
            };
            let get = || output.recv_timeout(Duration::from_secs(3)).unwrap();
            put(
                Some(1),
                "initialize",
                serde_json::json!({"protocolVersion":1}),
            );
            assert_eq!(get()["id"], 1);
            put(
                Some(2),
                "session/new",
                serde_json::json!({"cwd":std::env::temp_dir(),"mcpServers":[]}),
            );
            let session = get()["result"]["sessionId"].clone();
            put(
                Some(3),
                "session/prompt",
                serde_json::json!({"sessionId":session,"prompt":[{"type":"text","text":"fixture"}]}),
            );
            assert_eq!(
                events.recv_timeout(Duration::from_secs(3)).unwrap(),
                "started"
            );
            if !disconnect {
                put(
                    None,
                    "session/cancel",
                    serde_json::json!({"sessionId":session}),
                );
                assert_eq!(get()["result"]["stopReason"], "cancelled");
            }
            drop(input);
            assert_eq!(
                events.recv_timeout(Duration::from_secs(3)).unwrap(),
                "cancelled"
            );
            runner.join().unwrap().unwrap();
            assert!(output.try_recv().is_err());
        }
    }
}
