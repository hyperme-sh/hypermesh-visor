//! Exercise the released stdio interface against local HTTP/Unix visor peers.
use base64::Engine;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

const SESSION: &str = "00000000-0000-0000-0000-000000000001";

// A rejected valid request should fail this test, not leave accept blocked.
macro_rules! accept_bounded {
    ($listener:expr) => {{
        $listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match $listener.accept() {
                Ok(pair) => {
                    // BSD sockets inherit O_NONBLOCK from the listener.
                    pair.0.set_nonblocking(false).unwrap();
                    pair.0
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    pair.0
                        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    break pair;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "adapter did not connect"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    }};
}

fn initialize(version: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":"handshake", "method":"initialize", "params":{
        "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"contract-client","version":"1"}}})
}

fn run(endpoint: &str, unix: bool, session: &str, requests: &[Value]) -> Vec<Value> {
    let mut messages = vec![
        initialize("2024-11-05"),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ];
    messages.extend_from_slice(requests);
    run_raw(endpoint, unix, session, &messages)
        .into_iter()
        .filter(|message| message["id"] != "handshake")
        .collect()
}

fn run_raw(endpoint: &str, unix: bool, session: &str, requests: &[Value]) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hypermesh-desktop-mcp"));
    command
        .env("HYPERMESH_SESSION_ID", session)
        .env_remove("HYPERMESH_VISOR_TOKEN")
        .env_remove("HYPERMESH_VISOR_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if unix {
        command.env("HYPERMESH_RUNTIME_DIR", endpoint);
    } else {
        command
            .env("HYPERMESH_VISOR_URL", endpoint)
            .env("HYPERMESH_VISOR_TOKEN", "test-bearer");
    }
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

fn call(tool: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}})
}

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(include_str!("fixtures/view.png.b64").trim())
        .unwrap()
}

fn exchange(
    mut stream: impl Read + Write,
    status: u16,
    mime: &str,
    body: &[u8],
    chunked: bool,
) -> String {
    let mut request = Vec::new();
    let mut byte = [0];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
    }
    let header = String::from_utf8(request).unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).unwrap();
    write!(
        stream,
        "HTTP/1.1 {status} Response\r\nContent-Type: {mime}\r\n"
    )
    .unwrap();
    if chunked {
        write!(stream, "Transfer-Encoding: chunked\r\n\r\n").unwrap();
        for chunk in body.chunks(7) {
            write!(stream, "{:x}\r\n", chunk.len()).unwrap();
            stream.write_all(chunk).unwrap();
            stream.write_all(b"\r\n").unwrap();
        }
        stream.write_all(b"0\r\n\r\n").unwrap();
    } else {
        write!(stream, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
    }
    stream.flush().unwrap();
    // Keep the connection open: a framed response must not wait for EOF.
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).unwrap();
    format!("{header}{}", String::from_utf8_lossy(&payload))
}

fn roundtrip(
    unix: bool,
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    chunked: bool,
    request: Value,
) -> (Value, String) {
    let directory = std::env::temp_dir().join(format!("hm-inverse-{}", uuid::Uuid::new_v4()));
    let (result, wire) = if unix {
        std::fs::create_dir(&directory).unwrap();
        let listener = UnixListener::bind(directory.join("control.sock")).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = accept_bounded!(listener);
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            exchange(stream, status, mime, &body, chunked)
        });
        let responses = run(directory.to_str().unwrap(), true, SESSION, &[request]);
        (
            responses.into_iter().next().unwrap(),
            server.join().unwrap(),
        )
    } else {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = accept_bounded!(listener);
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            exchange(stream, status, mime, &body, chunked)
        });
        let responses = run(&endpoint, false, SESSION, &[request]);
        (
            responses.into_iter().next().unwrap(),
            server.join().unwrap(),
        )
    };
    if unix {
        std::fs::remove_dir_all(directory).unwrap();
    }
    (result, wire)
}

#[test]
fn capabilities_preserve_authenticated_contract_over_both_transports() {
    let contract = json!({"schema_version": 1, "verbs": [], "limits": {"max": 128}});
    for unix in [false, true] {
        let (response, wire) = roundtrip(
            unix,
            200,
            "application/json; charset=utf-8",
            serde_json::to_vec(&contract).unwrap(),
            true,
            call("capabilities", json!({})),
        );
        assert!(wire.starts_with("GET /capabilities HTTP/1.1"));
        if !unix {
            assert!(wire
                .to_lowercase()
                .contains("authorization: bearer test-bearer"));
        }
        assert_eq!(response["result"]["isError"], false);
        let actual: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(actual, contract);
        let (denied, _) = roundtrip(
            unix,
            401,
            "application/json",
            br#"{"error":"unauthorized"}"#.to_vec(),
            false,
            call("capabilities", json!({})),
        );
        assert_eq!(denied["result"]["isError"], true);
    }
    for (mime, body) in [
        ("text/html", "{}"),
        ("application/json", "[]"),
        ("application/json", "broken"),
    ] {
        let (response, _) = roundtrip(
            false,
            200,
            mime,
            body.as_bytes().to_vec(),
            false,
            call("capabilities", json!({})),
        );
        assert_eq!(response["result"]["isError"], true);
    }
}

#[test]
fn screenshots_preserve_png_bytes_over_both_transports_and_framings() {
    for unix in [false, true] {
        for chunked in [false, true] {
            let expected = png();
            let (response, wire) = roundtrip(
                unix,
                200,
                "image/png",
                expected.clone(),
                chunked,
                call("view", json!({})),
            );
            assert_eq!(response["id"], 1);
            assert_eq!(response["result"]["isError"], false);
            let image = &response["result"]["content"][0];
            assert_eq!(image["type"], "image");
            assert_eq!(image["mimeType"], "image/png");
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(image["data"].as_str().unwrap())
                    .unwrap(),
                expected
            );
            assert!(wire.starts_with(&format!("POST /session/{SESSION}/view HTTP/1.1")));
            if !unix {
                assert!(wire
                    .to_lowercase()
                    .contains("authorization: bearer test-bearer"));
            }
        }
    }
}

#[test]
fn invalid_arguments_never_reach_the_transport() {
    let mut requests = vec![];
    for args in [
        json!({}),
        json!({"x":null,"y":1}),
        json!({"x":"1","y":1}),
        json!({"x":true,"y":1}),
        json!({"x":1.5,"y":1}),
        json!({"x":-1,"y":1}),
        json!({"x":32768,"y":1}),
        json!({"x":1,"y":18446744073709551615u64}),
        json!([]),
        json!(null),
    ] {
        requests.push(call("mouse_move", args));
    }
    for button in [json!(null), json!(1), json!("wheel"), json!("Left")] {
        requests.push(call("mouse_click", json!({"x":1,"y":1,"button":button})));
    }
    for text in [json!(null), json!(1), json!(""), json!("a".repeat(4097))] {
        requests.push(call("type_text", json!({"text":text})));
    }
    for (dx, dy) in [
        (json!(0), json!(0)),
        (json!(129), json!(0)),
        (json!(64), json!(65)),
        (json!(null), json!(1)),
        (json!(1.5), json!(0)),
        (json!("2"), json!(0)),
        (json!(-9223372036854775808i64), json!(0)),
    ] {
        requests.push(call(
            "mouse_scroll",
            json!({"x":1,"y":2,"delta_x":dx,"delta_y":dy}),
        ));
    }
    requests.push(call("mouse_scroll", json!({"x":1,"y":2,"delta_y":1})));
    requests.extend([
        call("type_text", json!({})),
        call("view", json!({"extra":true})),
        call("capabilities", json!({"session":"override"})),
        call("resume", json!({})),
        call("read_file", json!({})),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":null}),
    ]);
    // This unbound endpoint turns any accidental connection into a tool error,
    // whereas every malformed request must return an invalid-params error.
    let responses = run("http://127.0.0.1:0", false, SESSION, &requests);
    assert_eq!(responses.len(), requests.len());
    for response in responses {
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    for session in ["", "../companion", "bad\r\nheader"] {
        let responses = run(
            "http://127.0.0.1:0",
            false,
            session,
            &[call("view", json!({}))],
        );
        assert_eq!(responses[0]["error"]["code"], -32602);
    }
}

#[test]
fn valid_arguments_and_tool_failures_preserve_semantics() {
    for unix in [false, true] {
        let (response, wire) = roundtrip(
            unix,
            204,
            "application/json",
            vec![],
            false,
            call(
                "mouse_scroll",
                json!({"x":3,"y":4,"delta_x":-64,"delta_y":64}),
            ),
        );
        assert_eq!(response["result"]["isError"], false);
        let body: Value = serde_json::from_str(wire.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"action":"scroll","x":3,"y":4,"delta_x":-64,"delta_y":64})
        );
        for (tool, args) in [
            ("mouse_move", json!({"x":32767,"y":0})),
            ("mouse_click", json!({"x":1,"y":2,"button":"right"})),
            ("mouse_click", json!({"x":1,"y":2,"button":"middle"})),
            ("type_text", json!({"text":"hello\nworld"})),
        ] {
            let (response, _) = roundtrip(
                unix,
                204,
                "application/json",
                vec![],
                false,
                call(tool, args),
            );
            assert_eq!(response["result"]["isError"], false);
        }
        for status in [401, 403, 404, 501] {
            let (response, _) = roundtrip(
                unix,
                status,
                "application/json",
                br#"{"error":"denied"}"#.to_vec(),
                false,
                call("view", json!({})),
            );
            assert_eq!(response["result"]["isError"], true);
            assert!(response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&status.to_string()));
        }
        let (response, wire) = roundtrip(
            unix,
            204,
            "application/json",
            vec![],
            false,
            call("mouse_click", json!({"x":0,"y":32767})),
        );
        assert_eq!(response["result"]["isError"], false);
        assert!(wire.contains("\"button\":\"left\""));
        let (response, _) = roundtrip(
            unix,
            409,
            "application/json",
            br#"{"error":"automation paused after 32 keystrokes"}"#.to_vec(),
            true,
            call("type_text", json!({"text":"hello"})),
        );
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("409"));
        assert!(text.contains("after 32 keystrokes"));
        for (mime, data) in [("text/html", png()), ("image/png", b"not a PNG".to_vec())] {
            let (response, _) = roundtrip(unix, 200, mime, data, false, call("view", json!({})));
            assert_eq!(response["result"]["isError"], true);
        }
    }
}

#[test]
fn tool_notifications_cannot_execute_and_schema_matches_validation() {
    let messages = [
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"view"}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    let responses = run("http://127.0.0.1:0", false, SESSION, &messages);
    assert_eq!(responses.len(), 1);
    let tools = responses[0]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 9);
    assert_eq!(tools[2]["inputSchema"]["properties"]["x"]["maximum"], 32767);
    assert_eq!(
        tools[3]["inputSchema"]["properties"]["button"]["enum"],
        json!(["left", "right", "middle"])
    );
    assert_eq!(
        tools[5]["inputSchema"]["properties"]["text"]["maxLength"],
        4096
    );
    assert!(tools
        .iter()
        .all(|tool| tool["inputSchema"]["additionalProperties"] == false));
}

#[test]
fn queued_task_routes_and_results_preserve_the_assigned_session() {
    let task = "00000000-0000-0000-0000-000000000002";
    for unix in [false, true] {
        for (tool, method) in [("task_status", "GET"), ("task_cancel", "DELETE")] {
            let (result, wire) = roundtrip(
                unix,
                200,
                "application/json",
                serde_json::to_vec(&json!({"id":task,"status":"completed","results":[]})).unwrap(),
                false,
                call(tool, json!({"task_id":task})),
            );
            assert!(wire.starts_with(&format!(
                "{method} /session/{SESSION}/tasks/{task} HTTP/1.1"
            )));
            assert_eq!(result["result"]["isError"], false);
        }
        let (result, wire) = roundtrip(
            unix,
            202,
            "application/json",
            serde_json::to_vec(&json!({"id":task,"status":"completed","results":[]})).unwrap(),
            false,
            call(
                "task_submit",
                json!({"steps":[{"tool":"view","arguments":{}}]}),
            ),
        );
        assert!(wire.starts_with(&format!("POST /session/{SESSION}/tasks HTTP/1.1")));
        assert_eq!(result["result"]["isError"], false);
    }
    let response = json!({"id":task,"status":"completed","results":[{"type":"image","mimeType":"image/png",
        "data":base64::engine::general_purpose::STANDARD.encode(png())}]});
    let (result, _) = roundtrip(
        false,
        200,
        "application/json",
        serde_json::to_vec(&response).unwrap(),
        false,
        call("task_status", json!({"task_id":task})),
    );
    assert_eq!(result["result"]["content"][1], response["results"][0]);
    for args in [
        json!({"steps":[]}),
        json!({"steps":[{"tool":"task_submit","arguments":{}}]}),
        json!({"steps":[{"tool":"view","arguments":{},"session":"other"}]}),
    ] {
        assert_eq!(
            run(
                "http://127.0.0.1:0",
                false,
                SESSION,
                &[call("task_submit", args)]
            )[0]["error"]["code"],
            -32602
        );
    }
    assert_eq!(
        run(
            "http://127.0.0.1:0",
            false,
            SESSION,
            &[call("task_status", json!({"task_id":"../other"}))]
        )[0]["error"]["code"],
        -32602
    );
}

#[test]
fn mcp_disconnect_cancels_only_its_submitted_task() {
    let task = "00000000-0000-0000-0000-000000000002";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded!(listener);
        let first = exchange(
            stream,
            202,
            "application/json",
            serde_json::to_string(&json!({"id":task,"status":"queued"}))
                .unwrap()
                .as_bytes(),
            false,
        );
        let (stream, _) = accept_bounded!(listener);
        let second = exchange(
            stream,
            200,
            "application/json",
            serde_json::to_string(&json!({"id":task,"status":"cancelled"}))
                .unwrap()
                .as_bytes(),
            false,
        );
        (first, second)
    });
    let responses = run(
        &format!("http://{address}"),
        false,
        SESSION,
        &[call(
            "task_submit",
            json!({"steps":[{"tool":"view","arguments":{}}]}),
        )],
    );
    assert_eq!(responses.len(), 1);
    let (first, second) = server.join().unwrap();
    assert!(first.starts_with(&format!("POST /session/{SESSION}/tasks HTTP/1.1")));
    assert!(second.starts_with(&format!("DELETE /session/{SESSION}/tasks/{task} HTTP/1.1")));
}

#[test]
fn task_ownership_limit_never_forgets_active_jobs_and_terminal_status_frees_capacity() {
    check_ownership_capacity("task_status", 200);
}

#[test]
fn evicted_task_status_and_cancel_free_tracking_capacity() {
    check_ownership_capacity("task_status", 404);
    check_ownership_capacity("task_cancel", 404);
}

fn check_ownership_capacity(resolution_tool: &'static str, resolution_status: u16) {
    let tasks: Vec<String> = (0..17).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = tasks.clone();
    let server = std::thread::spawn(move || {
        for task in &expected[..16] {
            let (stream, _) = accept_bounded!(listener);
            let wire = exchange(
                stream,
                202,
                "application/json",
                serde_json::to_vec(&json!({"id":task,"status":"queued"}))
                    .unwrap()
                    .as_slice(),
                false,
            );
            assert!(wire.starts_with(&format!("POST /session/{SESSION}/tasks HTTP/1.1")));
        }
        // The seventeenth submission must be refused before contacting the daemon.
        // Confirm a later task finished or was evicted while the oldest remains active.
        let (stream, _) = accept_bounded!(listener);
        let wire = exchange(
            stream,
            resolution_status,
            "application/json",
            serde_json::to_vec(&if resolution_status == 404 {
                json!({"error":"session closed or unknown"})
            } else {
                json!({"id":expected[15],"status":"completed"})
            })
            .unwrap()
            .as_slice(),
            false,
        );
        assert!(wire.starts_with(&format!(
            "{} /session/{SESSION}/tasks/{} HTTP/1.1",
            if resolution_tool == "task_status" {
                "GET"
            } else {
                "DELETE"
            },
            expected[15]
        )));
        let (stream, _) = accept_bounded!(listener);
        let wire = exchange(
            stream,
            202,
            "application/json",
            serde_json::to_vec(&json!({"id":expected[16],"status":"queued"}))
                .unwrap()
                .as_slice(),
            false,
        );
        assert!(wire.starts_with(&format!("POST /session/{SESSION}/tasks HTTP/1.1")));
        let mut cancelled = Vec::new();
        for _ in 0..16 {
            let (stream, _) = accept_bounded!(listener);
            let wire = exchange(stream, 200, "application/json", b"{}", false);
            cancelled.push(wire.lines().next().unwrap().to_owned());
        }
        for (index, task) in expected.iter().enumerate() {
            assert_eq!(
                cancelled.contains(&format!("DELETE /session/{SESSION}/tasks/{task} HTTP/1.1")),
                index != 15
            );
        }
    });
    let submit = call(
        "task_submit",
        json!({"steps":[{"tool":"view","arguments":{}}]}),
    );
    let mut requests = vec![submit.clone(); 17];
    requests.push(call(resolution_tool, json!({"task_id":tasks[15]})));
    requests.push(submit);
    let responses = run(&format!("http://{address}"), false, SESSION, &requests);
    server.join().unwrap();
    assert_eq!(responses.len(), 19);
    assert!(responses[..16]
        .iter()
        .all(|r| r["result"]["isError"] == false));
    assert_eq!(responses[16]["result"]["isError"], true);
    assert!(responses[16]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("ownership limit"));
    assert_eq!(responses[18]["result"]["isError"], false);
}

#[test]
fn uncertain_task_lookup_keeps_disconnect_ownership() {
    for (status, mime, body) in [
        (
            403,
            "application/json",
            r#"{"error":"session closed or unknown"}"#,
        ),
        (404, "text/html", r#"{"error":"session closed or unknown"}"#),
        (
            404,
            "application/json",
            r#"{"error":"proxy route missing"}"#,
        ),
        (
            200,
            "application/json",
            r#"{"error":"session closed or unknown"}"#,
        ),
    ] {
        let task = uuid::Uuid::new_v4().to_string();
        let expected = task.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = accept_bounded!(listener);
            exchange(
                stream,
                202,
                "application/json",
                serde_json::to_vec(&json!({"id":expected,"status":"queued"}))
                    .unwrap()
                    .as_slice(),
                false,
            );
            let (stream, _) = accept_bounded!(listener);
            exchange(stream, status, mime, body.as_bytes(), false);
            let (stream, _) = accept_bounded!(listener);
            let wire = exchange(stream, 200, "application/json", b"{}", false);
            assert!(wire.starts_with(&format!(
                "DELETE /session/{SESSION}/tasks/{expected} HTTP/1.1"
            )));
        });
        run(
            &format!("http://{address}"),
            false,
            SESSION,
            &[
                call(
                    "task_submit",
                    json!({"steps":[{"tool":"view","arguments":{}}]}),
                ),
                call("task_status", json!({"task_id":task})),
            ],
        );
        server.join().unwrap();
    }
}

#[test]
fn initialization_negotiates_before_tools_and_notifications_never_reply() {
    let messages = [
        call("view", json!({})),
        json!({"jsonrpc":"2.0","method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}),
        initialize("2099-01-01"),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}),
        initialize("2024-11-05"),
        json!({"jsonrpc":"1.0","id":6,"method":"tools/list"}),
    ];
    let replies = run_raw("http://127.0.0.1:0", false, SESSION, &messages);
    assert_eq!(replies.len(), 8);
    assert_eq!(replies[0]["error"]["code"], -32002);
    assert_eq!(replies[1]["error"]["code"], -32602);
    assert_eq!(replies[2]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(replies[3]["result"], json!({}));
    assert_eq!(replies[4]["error"]["code"], -32002);
    assert_eq!(replies[5]["result"]["tools"].as_array().unwrap().len(), 9);
    assert_eq!(replies[6]["error"]["code"], -32600);
    assert_eq!(replies[7]["error"]["code"], -32600);
}

#[test]
fn oversized_stdio_request_is_bounded_and_rejected_without_transport() {
    let replies = run_raw(
        "http://127.0.0.1:0",
        false,
        SESSION,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list","padding":"x".repeat(512 * 1024)})],
    );
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], -32600);
}
