//! Native app embedding. The privileged companion router is callable only in-process.
use crate::{api, desktop::NativeDesktop};
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

fn mcp_ui_request(action: &str, value: &serde_json::Value) -> Option<(&'static str, &'static str, String)> {
    let path = match action {
        "mcp-profiles" => "/mcp/profiles",
        "mcp-sources" => "/mcp/sources",
        "mcp-trusted" => "/mcp/trust",
        "mcp-preview" | "mcp-decision" => {
            let (path, key) = if action == "mcp-preview" { ("/mcp/discovery", "selection") } else { ("/mcp/trust", "decision") };
            let body = value.get(key).filter(|body| body.is_object())?;
            return Some(("POST", path, body.to_string()));
        }
        _ => return None,
    };
    Some(("GET", path, String::new()))
}

#[no_mangle]
pub extern "C" fn hypermesh_native_start(callback: Option<Callback>) -> i32 {
    #[cfg(windows)]
    if !crate::desktop::unelevated() { return 1; }
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
        let config = match crate::native_prompt::configuration(|name| std::env::var_os(name)) {
            Ok(config) => config,
            Err(error) => { eprintln!("native prompt configuration: {error}"); return 1; }
        };
        let state = match api::AppState::with_orchestrator(
            Arc::new(NativeDesktop::embedded(callback)), Duration::from_millis(250), config,
        ) {
            Ok(state) => Arc::new(state),
            Err(error) => { eprintln!("native prompt startup: {error}"); return 1; }
        };
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
        let result = if matches!(action, "idle-pause" | "idle-resume") {
            let Some(session) = value["session"].as_str().and_then(|id| uuid::Uuid::parse_str(id).ok()) else { return 1; };
            let epoch = if action == "idle-pause" {
                engine.state.native_idle_pause(session)
            } else {
                let Some(epoch) = value["epoch"].as_u64() else { return 1; };
                engine.state.native_idle_resume(session, epoch)
            };
            Some(match epoch {
                Some(epoch) => serde_json::json!({"status":200,"body":{"epoch":epoch}}),
                None => serde_json::json!({"status":409,"body":null}),
            })
        } else {
            let (method, path, body) = match action {
                "poll" | "destinations" => ("GET", "/companion", String::new()),
                "start" | "resume" => {
                    let Some(session) = value["session"].as_str().and_then(crate::session_destinations::canonical_session) else { return 1; };
                    ("POST", "/companion/automation", serde_json::json!({"action":action,"expected_session":session}).to_string())
                },
                "pause" | "stop" => ("POST", "/companion/automation", serde_json::json!({"action":action}).to_string()),
                "monitor-lost" => ("POST", "/companion/interrupt", "{\"available\":false}".into()),
                _ => match mcp_ui_request(action, &value) { Some(request) => request, None => return 1 },
            };
            let request = Request::builder().method(method).uri(path).header("content-type", "application/json").body(Body::from(body)).unwrap();
            engine.runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(3), async {
                    let response = api::companion_router(Arc::clone(&engine.state)).oneshot(request).await.ok()?;
                    let status = response.status().as_u16();
                    let bytes = to_bytes(response.into_body(), 256 * 1024).await.ok()?;
                    Some(serde_json::json!({"status":status,"body":serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null)}))
                }).await.ok().flatten()
            })
        };
        let Some(mut result) = result else { return 1; };
        if action == "destinations" {
            let expected = value["session"].as_str().and_then(crate::session_destinations::canonical_session);
            let selected = result["body"]["session"]["id"].as_str();
            if result["status"] != 200 || result["body"]["open"] != true || expected.as_deref().is_none() || expected.as_deref() != selected {
                result = serde_json::json!({"status":409,"body":null});
            } else {
                let directory = crate::mcp::config_dir();
                let session = expected.as_deref().unwrap();
                let forum = crate::session_destinations::configured(&directory, session, crate::session_destinations::Destination::Forum).ok().map(|v| v.argument);
                let workspace = crate::session_destinations::configured(&directory, session, crate::session_destinations::Destination::Editor).ok().map(|v| v.argument);
                result = serde_json::json!({"status":200,"body":{"session":session,"forum":forum,"workspace":workspace}});
            }
        }
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
    fn native_mcp_ui_has_only_fixed_review_routes() {
        let value = serde_json::json!({"selection":{"profile":"default"},"decision":{"action":"cancel","review_id":"00000000-0000-4000-8000-000000000001"}});
        assert_eq!(mcp_ui_request("mcp-sources", &value), Some(("GET", "/mcp/sources", String::new())));
        let (method, path, body) = mcp_ui_request("mcp-preview", &value).unwrap();
        assert_eq!((method, path), ("POST", "/mcp/discovery"));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap(), value["selection"]);
        let (method, path, body) = mcp_ui_request("mcp-decision", &value).unwrap();
        assert_eq!((method, path), ("POST", "/mcp/trust"));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap(), value["decision"]);
        for action in ["mcp-edit", "/companion/automation", "mcp-../companion/automation"] {
            assert!(mcp_ui_request(action, &value).is_none());
        }
        assert!(mcp_ui_request("mcp-preview", &serde_json::json!({"selection":"path"})).is_none());
        assert!(mcp_ui_request("mcp-decision", &serde_json::Value::Null).is_none());
    }
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

    #[test]
    fn embedded_prompts_use_the_configured_conductor() {
        const FLAG: &str = "HYPERMESH_NATIVE_PROMPT_TEST";
        if std::env::var_os(FLAG).is_some() {
            unsafe extern "C" fn denied(_: *const u8, _: usize, _: *mut *mut u8, _: *mut usize) -> i32 { 1 }
            assert_eq!(hypermesh_native_start(Some(denied)), 0);
            let engine = ENGINE.get().unwrap();
            engine.runtime.block_on(async {
                let request = Request::builder().method("POST").uri("/prompt")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"prompt":"native route contract","api_key":"org_native_contract"}"#)).unwrap();
                let response = api::router(Arc::clone(&engine.state)).oneshot(request).await.unwrap();
                assert_eq!(response.status(), 200);
                let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["response"], "native-conductor-ok");
                assert_eq!(body["passed_through"], true);
                assert!(!String::from_utf8_lossy(&bytes).contains("org_native_contract"));
            });
            return;
        }
        let fixture = crate::prompt::ChatFixture::spawn(200,
            r#"{"choices":[{"message":{"content":"native-conductor-ok"}}]}"#);
        let directory = std::env::temp_dir().join(format!("hv-native-prompt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let lease = "11111111-1111-4111-8111-111111111111";
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_engine::tests::embedded_prompts_use_the_configured_conductor"])
            .env(FLAG, "1")
            .env("HYPERMESH_CONDUCTOR_ADDR", fixture.url.trim_start_matches("http://"))
            .env("HYPERMESH_CONDUCTOR_LEASE_ID", lease)
            .env("HYPERMESH_DEFAULT_MODEL", "qwen-2.5-32b-q4")
            .env_remove("HYPERMESH_CONDUCTOR_CONFIG")
            .env_remove("HYPERMESH_SUPERVISOR_URL")
            .env_remove("HYPERMESH_SUPERVISOR_URL_2")
            .env("HYPERMESH_RUNTIME_DIR", directory.join("run"))
            .env("HYPERMESH_CONFIG_DIR", directory.join("config"))
            .output().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let hits = fixture.hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, crate::prompt::CHAT_COMPLETIONS_PATH);
        assert_eq!(hits[0].authorization, "Bearer org_native_contract");
        assert_eq!(hits[0].expected_model, "qwen-2.5-32b-q4");
        assert_eq!(hits[0].conductor_task, lease);
        assert!(hits[0].api_key.is_empty() && hits[0].lease.is_empty() && hits[0].hypermesh_lease.is_empty());
    }

    #[tokio::test]
    async fn external_native_companion_socket_has_no_pause_lift_authority() {
        let state = Arc::new(api::AppState::new(
            Arc::new(NativeDesktop::default()),
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

/// Allocate and release callback buffers through this DLL's allocator.
#[no_mangle]
pub unsafe extern "C" fn hypermesh_native_alloc(length: usize) -> *mut u8 {
    if length == 0 || length > 48 * 1024 * 1024 { return std::ptr::null_mut(); }
    libc::malloc(length).cast()
}
#[no_mangle]
pub unsafe extern "C" fn hypermesh_native_free(buffer: *mut u8) { libc::free(buffer.cast()); }
