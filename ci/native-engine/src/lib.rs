//! Native engine protocol contracts without opening or controlling a desktop.
#[cfg(all(test, windows))]
mod tests {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hypermesh_local_transport::{connect, Endpoint};
    use hypermesh_visor::{AppState, NativeDesktop};
    use std::{sync::Arc, time::Duration};

    async fn request(
        endpoint: &Endpoint,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, serde_json::Value) {
        let stream = connect(endpoint).await.unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        let request = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        drop(sender);
        driver.abort();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn native_pipe_metadata_pause_authority_and_shutdown() {
        let instance = format!(
            "engine-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(&instance);
        hypermesh_local_storage::create_dir(&directory).unwrap();
        std::env::set_var("HYPERMESH_RUNTIME_DIR", &directory);
        std::env::set_var("HYPERMESH_CONFIG_DIR", &directory);
        std::env::set_var("HYPERMESH_VISOR_INSTANCE", &instance);
        let control = Endpoint::windows(&instance, "control").unwrap();
        let companion = Endpoint::windows(&instance, "companion").unwrap();
        let state = Arc::new(AppState::new(
            Arc::new(NativeDesktop::default()),
            Duration::from_millis(20),
        ));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(hypermesh_visor::serve_until(None, state, async {
            let _ = stopped.await;
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if connect(&control).await.is_ok() && connect(&companion).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for endpoint in [&control, &companion] {
            let (status, value) = request(endpoint, "GET", "/capabilities", "").await;
            assert_eq!(status, 200);
            assert_eq!(value["backend"], "windows");
            assert_eq!(value["authorization_granted"], false);
            assert_eq!(value["desktop"]["view"], "disabled");
            assert_eq!(
                request(
                    endpoint,
                    "POST",
                    "/companion/automation",
                    r#"{"action":"pause"}"#
                )
                .await
                .0,
                204
            );
            assert_eq!(
                request(
                    endpoint,
                    "POST",
                    "/companion/automation",
                    r#"{"action":"resume"}"#
                )
                .await
                .0,
                403
            );
            assert_eq!(
                request(
                    endpoint,
                    "POST",
                    "/companion/interrupt",
                    r#"{"available":false}"#
                )
                .await
                .0,
                403
            );
            assert_eq!(request(endpoint, "GET", "/mcp/sources", "").await.0, 403);
            assert_eq!(request(endpoint, "GET", "/mcp/trust", "").await.0, 403);
            assert_eq!(request(endpoint, "POST", "/mcp/discovery", r#"{"profile":"default"}"#).await.0, 403);
            assert_eq!(request(endpoint, "POST", "/mcp/trust", r#"{"action":"approve","review_id":"00000000-0000-4000-8000-000000000001"}"#).await.0, 403);
        }
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        // Windows has no filesystem socket to remove; shutdown releases the names.
        assert!(hypermesh_local_transport::Listener::bind(&control).is_ok());
        assert!(hypermesh_local_transport::Listener::bind(&companion).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(all(test, windows))]
#[path = "../../../src/desktop/focus_windows.rs"]
mod identity;

#[cfg(all(test, windows))]
#[test]
fn native_process_identity_and_buffer_contract() {
    let image = identity::process_executable(std::process::id()).unwrap();
    assert_eq!(
        std::path::Path::new(&image).file_name(),
        std::env::current_exe().unwrap().file_name()
    );
    assert!(identity::process_executable(0).is_none());
    assert!(identity::process_executable(u32::MAX).is_none());
    type Callback = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;
    extern "C" {
        fn hypermesh_native_start(callback: Option<Callback>) -> i32;
        fn hypermesh_native_alloc(length: usize) -> *mut u8;
        fn hypermesh_native_free(buffer: *mut u8);
    }
    unsafe {
        assert_eq!(hypermesh_native_start(None), 1);
        assert!(hypermesh_native_alloc(0).is_null());
        assert!(hypermesh_native_alloc(49 * 1024 * 1024).is_null());
        let buffer = hypermesh_native_alloc(4);
        assert!(!buffer.is_null());
        std::ptr::copy_nonoverlapping(b"test".as_ptr(), buffer, 4);
        assert_eq!(std::slice::from_raw_parts(buffer, 4), b"test");
        hypermesh_native_free(buffer);
        hypermesh_native_free(std::ptr::null_mut());
    }
}
