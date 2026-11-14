//! Executable stdio protocol contracts with a side-effect-free fixture backend.
use hypermesh_local_storage as storage;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use uuid::Uuid;
struct Client {
    child: Child,
    messages: mpsc::Receiver<Value>,
    directory: PathBuf,
    machine: Uuid,
    session: Uuid,
}
impl Client {
    fn new(assigned: bool) -> Self {
        let directory = std::env::temp_dir().join(format!("visor-acp-cli-{}", Uuid::new_v4()));
        storage::create_dir(&directory).unwrap();
        let machine = Uuid::new_v4();
        let session = Uuid::new_v4();
        let path = directory.join("assignment.json");
        storage::open(&path, storage::Open::New).unwrap().write_all(&serde_json::to_vec(&json!({"machine":machine,"session":session,"assigned":assigned,"paused":false})).unwrap()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_hypermesh-acp"))
            .arg("--fixture-assignment")
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            messages,
            directory,
            machine,
            session,
        }
    }
    fn send(&mut self, value: Value) {
        let input = self.child.stdin.as_mut().unwrap();
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }
    fn recv(&self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(5))
            .expect("bounded ACP response")
    }
    fn open(&mut self) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
        assert_eq!(self.recv()["result"]["protocolVersion"], 1);
        self.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":self.directory,"mcpServers":[]}}));
        self.recv()
    }
    fn prompt(&mut self, session: Uuid) {
        self.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"private-prompt-canary"}]}}));
    }
    fn pause(&self) {
        let temporary = self.directory.join("replacement.json");
        storage::open(&temporary, storage::Open::New).unwrap().write_all(&serde_json::to_vec(&json!({"machine":self.machine,"session":self.session,"assigned":true,"paused":true})).unwrap()).unwrap();
        storage::replace(&temporary, &self.directory.join("assignment.json")).unwrap();
    }
    fn finish(&mut self) -> String {
        self.child.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "adapter did not close on EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        stderr
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
#[test]
fn initialize_assigned_turn_and_cancel_without_prompt_logging() {
    let mut client = Client::new(true);
    assert_eq!(
        client.open()["result"]["sessionId"],
        client.session.to_string()
    );
    client.prompt(client.session);
    assert_eq!(
        client.recv()["params"]["update"]["content"]["text"],
        "Fixture turn completed."
    );
    assert_eq!(client.recv()["result"]["stopReason"], "end_turn");
    client.prompt(client.session);
    client.send(
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":client.session}}),
    );
    assert_eq!(client.recv()["result"]["stopReason"], "cancelled");
    assert!(!client.finish().contains("private-prompt-canary"));
}
#[test]
fn unassigned_wrong_session_and_changed_pause_are_refused() {
    let mut unassigned = Client::new(false);
    assert!(unassigned.open().get("error").is_some());
    unassigned.finish();
    let mut client = Client::new(true);
    client.open();
    client.prompt(Uuid::new_v4());
    assert!(client.recv().get("error").is_some());
    client.prompt(client.session);
    client.pause();
    assert!(client.recv().get("error").is_some());
    assert!(!client.finish().contains("private-prompt-canary"));
}
#[test]
fn eof_cancels_active_turn_and_process_exits_with_idle_stdio_workers() {
    let mut client = Client::new(true);
    client.open();
    client.prompt(client.session);
    assert!(!client.finish().contains("private-prompt-canary"));
    assert!(client
        .messages
        .try_iter()
        .all(|v| v["method"] != "session/update"));
}
#[test]
fn no_implicit_backend_or_listener_is_enabled() {
    let output = Command::new(env!("CARGO_BIN_EXE_hypermesh-acp"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
