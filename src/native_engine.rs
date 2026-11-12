//! Native app embedding. The privileged companion router is callable only in-process.
use crate::{api, desktop::MacDesktop};
use axum::body::{to_bytes, Body};
use axum::http::Request;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tower::ServiceExt;

struct Engine {
    runtime: tokio::runtime::Runtime,
    state: Arc<api::AppState>,
    failed: AtomicBool,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();

pub(crate) fn is_embedded() -> bool {
    ENGINE.get().is_some()
}

type Callback = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;

#[no_mangle]
pub extern "C" fn hypermesh_native_start(callback: Option<Callback>) -> i32 {
    let Some(callback) = callback else {
        return 1;
    };
    std::panic::catch_unwind(|| {
        if ENGINE.get().is_some() {
            return 1;
        }
        let Ok(runtime) = tokio::runtime::Runtime::new() else {
            return 1;
        };
        let state = Arc::new(api::AppState::new(
            Arc::new(MacDesktop::embedded(callback)),
            Duration::from_millis(250),
        ));
        if ENGINE
            .set(Engine {
                runtime,
                state,
                failed: AtomicBool::new(false),
            })
            .is_err()
        {
            return 1;
        }
        let engine = ENGINE.get().unwrap();
        let state = Arc::clone(&engine.state);
        engine.runtime.spawn(async move {
            if let Err(error) = api::serve(None, state).await {
                eprintln!("native server stopped: {error}");
            }
            ENGINE.get().unwrap().failed.store(true, Ordering::Release);
        });
        0
    })
    .unwrap_or(1)
}

/// Internal UI requests only. External callers never get the companion role.
/// Caller frees the returned malloc buffer using the platform C allocator.
#[no_mangle]
pub unsafe extern "C" fn hypermesh_native_control(
    request: *const u8,
    length: usize,
    output: *mut *mut u8,
    output_length: *mut usize,
) -> i32 {
    if request.is_null() || output.is_null() || output_length.is_null() || length > 65536 {
        return 1;
    }
    *output = std::ptr::null_mut();
    *output_length = 0;
    std::panic::catch_unwind(|| {
        let Some(engine) = ENGINE.get() else { return 1; };
        if engine.failed.load(Ordering::Acquire) { return 1; }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(std::slice::from_raw_parts(request, length)) else { return 1; };
        let action = value["action"].as_str().unwrap_or("");
        let (method, path, body) = match action {
            "poll" => ("GET", "/companion", String::new()),
            "start" | "pause" | "resume" | "stop" => ("POST", "/companion/automation", serde_json::json!({"action":action}).to_string()),
            "monitor-lost" => ("POST", "/companion/interrupt", "{\"available\":false}".into()),
            _ => return 1,
        };
        let request = Request::builder().method(method).uri(path).header("content-type", "application/json").body(Body::from(body)).unwrap();
        let result = engine.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                let response = api::companion_router(Arc::clone(&engine.state)).oneshot(request).await.ok()?;
                let status = response.status().as_u16();
                let bytes = to_bytes(response.into_body(), 256 * 1024).await.ok()?;
                Some(serde_json::json!({"status":status,"body":serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null)}))
            }).await.ok().flatten()
        });
        let Some(result) = result else { return 1; };
        let Ok(bytes) = serde_json::to_vec(&result) else { return 1; };
        let buffer = libc::malloc(bytes.len()) as *mut u8;
        if buffer.is_null() { return 1; }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        *output = buffer;
        *output_length = bytes.len();
        0
    }).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_callback_cannot_start_native_engine() {
        assert_eq!(hypermesh_native_start(None), 1);
    }

    #[test]
    fn embedded_app_cannot_spawn_mcp_children() {
        const FLAG: &str = "HYPERMESH_NATIVE_NO_CHILD_TEST";
        if let Ok(directory) = std::env::var(FLAG) {
            unsafe extern "C" fn denied(
                _: *const u8,
                _: usize,
                _: *mut *mut u8,
                _: *mut usize,
            ) -> i32 {
                1
            }
            assert_eq!(hypermesh_native_start(Some(denied)), 0);
            let marker = std::path::Path::new(&directory).join("spawned");
            let spec = serde_json::from_value(serde_json::json!({"command":"/bin/sh","args":["-c","touch \"$1\"","fixture",marker]})).unwrap();
            let result =
                crate::mcp::attach_specs("fixture".into(), [("fixture".into(), spec)].into());
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains("isolated worker"));
            assert!(!marker.exists());
            return;
        }
        let directory = std::env::temp_dir().join(format!("hv-child-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_engine::tests::embedded_app_cannot_spawn_mcp_children",
            ])
            .env(FLAG, &directory)
            .env("HYPERMESH_RUNTIME_DIR", directory.join("run"))
            .env("HYPERMESH_CONFIG_DIR", directory.join("config"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[tokio::test]
    async fn external_native_companion_socket_has_no_pause_lift_authority() {
        let state = Arc::new(api::AppState::new(
            Arc::new(MacDesktop::default()),
            Duration::from_millis(250),
        ));
        let control = |action: &str| {
            Request::builder()
                .method("POST")
                .uri("/companion/automation")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({"action":action}).to_string()))
                .unwrap()
        };
        assert_eq!(
            api::socket_companion_router(Arc::clone(&state))
                .oneshot(control("pause"))
                .await
                .unwrap()
                .status(),
            204
        );
        for action in ["start", "resume", "stop"] {
            assert_eq!(
                api::socket_companion_router(Arc::clone(&state))
                    .oneshot(control(action))
                    .await
                    .unwrap()
                    .status(),
                403
            );
        }
        assert_eq!(
            api::companion_router(Arc::clone(&state))
                .oneshot(control("stop"))
                .await
                .unwrap()
                .status(),
            204
        );
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/companion/interrupt")
                .header("content-type", "application/json")
                .body(Body::from("{\"available\":false}"))
                .unwrap()
        };
        assert_eq!(
            api::socket_companion_router(Arc::clone(&state))
                .oneshot(request())
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            api::companion_router(state)
                .oneshot(request())
                .await
                .unwrap()
                .status(),
            204
        );
    }
}
