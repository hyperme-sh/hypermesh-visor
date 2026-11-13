//! Per-user Windows pipes. Native approval remains inside the app process.
use axum::body::Body;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hypermesh_local_storage as storage;
use hypermesh_local_transport::{Endpoint, Listener};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};
use tower::ServiceExt;

fn endpoint(channel: &str) -> io::Result<Endpoint> {
    let instance = match std::env::var("HYPERMESH_VISOR_INSTANCE") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "default".into(),
        Err(_) => return Err(io::Error::other("invalid Visor instance")),
    };
    Endpoint::windows(&instance, channel)
}
pub fn runtime_dir() -> io::Result<PathBuf> {
    // Validate the instance before a path helper can be called.
    endpoint("control")?;
    if let Some(path) = std::env::var_os("HYPERMESH_RUNTIME_DIR") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(io::Error::other("runtime directory must be absolute"));
        }
        return Ok(path);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::other("LOCALAPPDATA is required"))?;
    let base = PathBuf::from(base);
    if !base.is_absolute() {
        return Err(io::Error::other("LOCALAPPDATA must be absolute"));
    }
    let base = base.join("HypermeshVisor");
    storage::create_dir(&base)?;
    let instance = std::env::var("HYPERMESH_VISOR_INSTANCE").unwrap_or_else(|_| "default".into());
    Ok(base.join(format!("run-{instance}")))
}
pub fn prepare_dir(path: &Path) -> io::Result<()> {
    storage::create_dir(path).map(|_| ())
}
pub fn socket_file(_: &Path) -> PathBuf {
    endpoint("companion")
        .expect("validated instance")
        .path()
        .into()
}
pub fn control_socket_file(_: &Path) -> PathBuf {
    endpoint("control")
        .expect("validated instance")
        .path()
        .into()
}
pub struct RuntimeLock {
    _file: File,
}
pub fn lock_runtime(dir: &Path) -> io::Result<RuntimeLock> {
    let file = storage::open(&dir.join("companion.lock"), storage::Open::Lock)?;
    storage::lock(&file)?;
    Ok(RuntimeLock { _file: file })
}
pub async fn bind(_: &Path) -> io::Result<Listener> {
    Listener::bind(&endpoint("companion")?)
}
pub async fn bind_control(_: &Path) -> io::Result<Listener> {
    Listener::bind(&endpoint("control")?)
}
pub fn write_api_token(_: &Path) -> io::Result<String> {
    Err(io::Error::other(
        "native Windows uses protected pipes; TCP is unavailable",
    ))
}
pub fn read_api_token(_: &Path) -> io::Result<String> {
    Err(io::Error::other("native Windows has no TCP token"))
}
pub fn remove_api_token(_: &Path) -> io::Result<()> {
    Ok(())
}
pub fn bearer_matches(_: &str, _: Option<&str>) -> bool {
    false
}

pub async fn accept_loop(
    mut listener: Listener,
    app: axum::Router,
    _: &'static str,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> io::Result<()> {
    loop {
        tokio::select! {
            biased;
            _ = async { let _ = stop.wait_for(|stop| *stop).await; } => return Ok(()),
            accepted = listener.accept() => {
                let stream = accepted?;
                let app = app.clone();
                let mut connection_stop = stop.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                        let app = app.clone();
                        async move { Ok::<_, std::convert::Infallible>(app.oneshot(request.map(Body::new)).await.unwrap_or_else(|e| match e {})) }
                    });
                    let builder = Builder::new(TokioExecutor::new()).http1_only();
                    tokio::select! {
                        _ = builder.serve_connection(TokioIo::new(stream), service) => {},
                        _ = async { let _ = connection_stop.wait_for(|stop| *stop).await; } => {},
                    }
                });
            }
        }
    }
}
