//! Process-owned stdio shell for ACP. Production routing belongs to #219.
use hypermesh_local_storage as storage;
use hypermesh_visor_client::acp::transport::{self, Backend, BackendFuture};
use serde::Deserialize;
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    machine: Uuid,
    session: Uuid,
    assigned: bool,
    paused: bool,
}
struct Fixture {
    path: PathBuf,
    machine: Uuid,
    session: Uuid,
}
impl Fixture {
    fn read(path: &std::path::Path) -> Result<Assignment, ()> {
        if !path.is_absolute() {
            return Err(());
        }
        storage::check_dir(path.parent().ok_or(())?).map_err(|_| ())?;
        let file = storage::open(path, storage::Open::Read { private: true }).map_err(|_| ())?;
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes).map_err(|_| ())?;
        if bytes.len() > 4096 {
            return Err(());
        }
        let record: Assignment = serde_json::from_slice(&bytes).map_err(|_| ())?;
        if record.machine.is_nil() || record.session.is_nil() {
            return Err(());
        }
        Ok(record)
    }
    async fn authorized(&self, cancel: &AtomicBool) -> Result<(), ()> {
        if cancel.load(Ordering::SeqCst) {
            return Err(());
        }
        let path = self.path.clone();
        let current = tokio::task::spawn_blocking(move || Self::read(&path))
            .await
            .map_err(|_| ())??;
        if cancel.load(Ordering::SeqCst)
            || !current.assigned
            || current.paused
            || current.machine != self.machine
            || current.session != self.session
        {
            return Err(());
        }
        Ok(())
    }
}
impl Backend for Fixture {
    fn open<'a>(&'a self, cancel: &'a AtomicBool) -> BackendFuture<'a, Uuid> {
        Box::pin(async move {
            self.authorized(cancel).await?;
            Ok(self.session)
        })
    }
    fn prompt<'a>(&'a self, _text: &'a str, cancel: &'a AtomicBool) -> BackendFuture<'a, String> {
        Box::pin(async move {
            // Fixture only: no model, desktop, filesystem content or tool action.
            // Recheck assignment and pause while the cancellable turn is active.
            for _ in 0..10 {
                self.authorized(cancel).await?;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            self.authorized(cancel).await?;
            Ok("Fixture turn completed.".into())
        })
    }
}
fn run() -> i32 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("hypermesh-acp --fixture-assignment PRIVATE_JSON\nLocal ACP stdio adapter. Fixture mode performs no model or tool calls.\nProduction Conductor routing is not configured by this executable yet.");
        return 0;
    }
    if args.len() != 2 || args[0] != "--fixture-assignment" {
        eprintln!("ACP backend unavailable; use --help for explicit fixture mode");
        return 2;
    }
    let path = PathBuf::from(&args[1]);
    let Ok(assignment) = Fixture::read(&path) else {
        eprintln!("ACP fixture configuration refused");
        return 2;
    };
    let backend = Arc::new(Fixture {
        path,
        machine: assignment.machine,
        session: assignment.session,
    });
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 1;
    };
    let runtime = std::mem::ManuallyDrop::new(runtime);
    let code = runtime.block_on(async {
        match transport::serve(tokio::io::stdin(), tokio::io::stdout(), backend).await {
            Ok(()) => 0,
            Err(_) => {
                eprintln!("ACP connection closed");
                1
            }
        }
    });
    // Tokio's portable stdio uses blocking workers. This standalone process owns
    // them all: exit without dropping/waiting on the runtime after serve drops
    // its active backend future. OS process exit closes every stdio worker and
    // handle, including a stalled pipe. Never use this shell inside a host app.
    std::process::exit(code)
}
fn main() {
    std::panic::set_hook(Box::new(|_| eprintln!("ACP adapter failed")));
    std::process::exit(std::panic::catch_unwind(run).unwrap_or(1));
}
