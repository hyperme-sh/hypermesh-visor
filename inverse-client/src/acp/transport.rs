//! Cancellable async transport. The caller supplies authorized routing and I/O.
use super::{Action, Protocol, MAX_FRAME};
use serde_json::Value;
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    time::Instant,
};

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ()>> + Send + 'a>>;
/// Bind one assigned machine/session and recheck authorization on every prompt.
/// Use Conductor and AIuditor. Futures must use cancellable I/O and release all
/// owned work when dropped; do not detach workers. No production route is installed.
pub trait Backend: Send + Sync + 'static {
    fn open<'a>(&'a self, cancel: &'a AtomicBool) -> BackendFuture<'a, uuid::Uuid>;
    fn prompt<'a>(&'a self, text: &'a str, cancel: &'a AtomicBool) -> BackendFuture<'a, String>;
}
enum Completion {
    Open(Value, Result<uuid::Uuid, ()>),
    Prompt(Result<String, ()>),
}
struct Active {
    cancel: Arc<AtomicBool>,
    future: Pin<Box<dyn Future<Output = Completion> + Send>>,
    expires: Instant,
    open_id: Option<Value>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}
struct Pending {
    bytes: Vec<u8>,
    offset: usize,
    expires: Instant,
}
fn enqueue(queue: &mut VecDeque<Pending>, value: Value, timeout: Duration) -> io::Result<()> {
    if queue.len() >= 16 {
        return Err(io::Error::other("ACP output queue full"));
    }
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    queue.push_back(Pending {
        bytes,
        offset: 0,
        expires: Instant::now() + timeout,
    });
    Ok(())
}
async fn output_step(
    output: &mut (impl AsyncWrite + Unpin),
    pending: &Pending,
) -> io::Result<usize> {
    if pending.offset == pending.bytes.len() {
        output.flush().await?;
        Ok(0)
    } else {
        let n = output.write(&pending.bytes[pending.offset..]).await?;
        if n == 0 {
            Err(io::ErrorKind::WriteZero.into())
        } else {
            Ok(n)
        }
    }
}
/// Own one connection. Dropping this future releases its I/O and backend future.
/// Supply truly cancellable platform handles, not blocking stdio wrappers.
/// EOF, framing errors, stalled output and backend deadlines cancel active work.
pub async fn serve(
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
    backend: Arc<dyn Backend>,
) -> io::Result<()> {
    serve_until(input, output, backend, Duration::from_secs(30)).await
}
async fn serve_until(
    input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
    backend: Arc<dyn Backend>,
    timeout: Duration,
) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let mut protocol = Protocol::default();
    let mut active: Option<Active> = None;
    let mut queue: VecDeque<Pending> = VecDeque::new();
    let mut frame = Vec::new();
    loop {
        let expires = active
            .as_ref()
            .map(|a| a.expires)
            .into_iter()
            .chain(queue.front().map(|p| p.expires))
            .min();
        let mut byte = [0];
        enum Event {
            Read(io::Result<usize>),
            Written(io::Result<usize>),
            Done(Completion),
            Timeout,
        }
        let event = tokio::select! {
            n = input.read(&mut byte) => Event::Read(n),
            n = async { output_step(&mut output, queue.front().unwrap()).await }, if !queue.is_empty() => Event::Written(n),
            result = async { active.as_mut().unwrap().future.as_mut().await }, if active.is_some() => Event::Done(result),
            _ = async { tokio::time::sleep_until(expires.unwrap()).await }, if expires.is_some() => Event::Timeout,
        };
        match event {
            Event::Timeout => return Err(io::ErrorKind::TimedOut.into()),
            Event::Written(n) => {
                let n = n?;
                if n == 0 {
                    queue.pop_front();
                } else {
                    queue.front_mut().unwrap().offset += n;
                }
            }
            Event::Done(result) => {
                active = None;
                match result {
                    Completion::Open(id, result) => {
                        let action = match result {
                            Ok(session) => protocol.opened(id, session),
                            Err(()) => protocol.open_failed(id),
                        };
                        if let Action::Reply(v) = action {
                            enqueue(&mut queue, v, timeout)?;
                        }
                    }
                    Completion::Prompt(result) => {
                        for v in protocol.finish(result.as_deref().map_err(|_| ()), false) {
                            enqueue(&mut queue, v, timeout)?;
                        }
                    }
                }
            }
            Event::Read(n) => {
                if n? == 0 {
                    return if frame.is_empty() {
                        Ok(())
                    } else {
                        Err(io::ErrorKind::InvalidData.into())
                    };
                }
                frame.push(byte[0]);
                if frame.len() > MAX_FRAME {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                if byte[0] != b'\n' {
                    continue;
                }
                let line = std::str::from_utf8(&frame).map_err(|_| io::ErrorKind::InvalidData)?;
                let action = protocol.receive(line);
                frame.clear();
                match action {
                    Action::Ignore => {}
                    Action::Reply(v) => enqueue(&mut queue, v, timeout)?,
                    Action::Cancel => {
                        if let Some(job) = active.take() {
                            let id = job.open_id.clone();
                            drop(job);
                            if let Some(id) = id {
                                if let Action::Reply(v) = protocol.open_failed(id) {
                                    enqueue(&mut queue, v, timeout)?;
                                }
                            } else {
                                for v in protocol.finish(Err(()), true) {
                                    enqueue(&mut queue, v, timeout)?;
                                }
                            }
                        }
                    }
                    action @ (Action::Open { .. } | Action::Prompt { .. }) => {
                        let cancel = Arc::new(AtomicBool::new(false));
                        let flag = cancel.clone();
                        let route = backend.clone();
                        let open_id = match &action {
                            Action::Open { id } => Some(id.clone()),
                            _ => None,
                        };
                        let future = Box::pin(async move {
                            match action {
                                Action::Open { id } => {
                                    Completion::Open(id, route.open(&flag).await)
                                }
                                Action::Prompt { text, .. } => {
                                    Completion::Prompt(route.prompt(&text, &flag).await)
                                }
                                _ => unreachable!(),
                            }
                        });
                        active = Some(Active {
                            cancel,
                            future,
                            expires: Instant::now() + timeout,
                            open_id,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, DuplexStream};
    struct WaitingBackend(Arc<AtomicBool>);
    struct Released(Arc<AtomicBool>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl Backend for WaitingBackend {
        fn open<'a>(&'a self, _: &'a AtomicBool) -> BackendFuture<'a, uuid::Uuid> {
            Box::pin(async { Ok(uuid::Uuid::nil()) })
        }
        fn prompt<'a>(&'a self, _: &'a str, _: &'a AtomicBool) -> BackendFuture<'a, String> {
            Box::pin(async move {
                let _guard = Released(self.0.clone());
                std::future::pending().await
            })
        }
    }
    async fn put(input: &mut DuplexStream, id: Option<u32>, method: &str, params: Value) {
        let mut v = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params});
        if let Some(id) = id {
            v["id"] = id.into();
        }
        input.write_all(format!("{v}\n").as_bytes()).await.unwrap();
    }
    async fn get(output: &mut BufReader<DuplexStream>) -> Value {
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }
    #[tokio::test]
    async fn cancellation_disconnect_and_stalled_output_release_work() {
        for mode in 0..3 {
            let (mut input, server_input) = tokio::io::duplex(4096);
            let (output, server_output) = tokio::io::duplex(16);
            let mut output = BufReader::new(output);
            let released = Arc::new(AtomicBool::new(false));
            let backend = Arc::new(WaitingBackend(released.clone()));
            let runner = tokio::spawn(serve_until(
                server_input,
                server_output,
                backend,
                Duration::from_millis(200),
            ));
            put(
                &mut input,
                Some(1),
                "initialize",
                serde_json::json!({"protocolVersion":1}),
            )
            .await;
            assert_eq!(get(&mut output).await["id"], 1);
            put(
                &mut input,
                Some(2),
                "session/new",
                serde_json::json!({"cwd":std::env::temp_dir(),"mcpServers":[]}),
            )
            .await;
            let session = get(&mut output).await["result"]["sessionId"].clone();
            put(
                &mut input,
                Some(3),
                "session/prompt",
                serde_json::json!({"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            if mode == 1 {
                drop(input);
            } else {
                if mode == 2 {
                    put(&mut input, Some(4), "unknown", serde_json::json!({})).await;
                }
                put(
                    &mut input,
                    None,
                    "session/cancel",
                    serde_json::json!({"sessionId":session}),
                )
                .await;
                if mode == 0 {
                    assert_eq!(get(&mut output).await["result"]["stopReason"], "cancelled");
                    drop(input);
                }
            }
            let result = tokio::time::timeout(Duration::from_secs(2), runner)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.is_err(), mode == 2);
            assert!(released.load(Ordering::SeqCst));
        }
    }
    #[tokio::test]
    async fn stalled_output_deadline_drops_input_without_a_reader_thread() {
        let (mut input, server_input) = tokio::io::duplex(4096);
        let (_output, server_output) = tokio::io::duplex(1);
        let backend = Arc::new(WaitingBackend(Arc::new(AtomicBool::new(false))));
        let runner = tokio::spawn(serve_until(
            server_input,
            server_output,
            backend,
            Duration::from_millis(20),
        ));
        put(
            &mut input,
            Some(1),
            "initialize",
            serde_json::json!({"protocolVersion":1}),
        )
        .await;
        assert_eq!(
            runner.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(input.read(&mut [0]).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn malformed_frames_close_connection() {
        for bytes in [vec![255, b'\n'], vec![b'x'; MAX_FRAME + 1], b"{}".to_vec()] {
            let (mut input, server_input) = tokio::io::duplex(MAX_FRAME + 2);
            let (_output, server_output) = tokio::io::duplex(16);
            input.write_all(&bytes).await.unwrap();
            input.shutdown().await.unwrap();
            let backend = Arc::new(WaitingBackend(Arc::new(AtomicBool::new(false))));
            assert_eq!(
                serve(server_input, server_output, backend)
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
