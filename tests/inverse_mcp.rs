//! Exercise the released stdio interface against local HTTP/Unix visor peers.
use base64::Engine;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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
        .env_remove("APPLICATIONINSIGHTS_CONNECTION_STRING")
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
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let socket = directory.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
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
    assert_eq!(tools.len(), 11);
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
fn evicted_task_status_and_cancel_accept_alternate_uuid_spellings() {
    check_ownership_capacity("task_status", 404);
    check_ownership_capacity("task_cancel", 404);
}

fn check_ownership_capacity(resolution_tool: &'static str, resolution_status: u16) {
    let mut tasks: Vec<String> = (0..17).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    tasks[15] = "abcdefab-cdef-4abc-8def-abcdefabcdef".into();
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
    let lookup_id = if resolution_status == 404 {
        if resolution_tool == "task_status" {
            tasks[15].to_uppercase()
        } else {
            tasks[15].replace('-', "")
        }
    } else {
        tasks[15].clone()
    };
    requests.push(call(resolution_tool, json!({"task_id":lookup_id})));
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
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"unknown-completed-request"}}),
        initialize("2024-11-05"),
        json!({"jsonrpc":"1.0","id":6,"method":"tools/list"}),
    ];
    let replies = run_raw("http://127.0.0.1:0", false, SESSION, &messages);
    assert_eq!(replies.len(), 8);
    assert_eq!(replies[0]["error"]["code"], -32002);
    assert_eq!(replies[1]["error"]["code"], -32602);
    assert_eq!(replies[2]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(replies[2]["result"]["capabilities"]["resources"], json!({}));
    assert_eq!(replies[3]["result"], json!({}));
    assert_eq!(replies[4]["error"]["code"], -32002);
    assert_eq!(replies[5]["result"]["tools"].as_array().unwrap().len(), 11);
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

#[test]
fn drag_preserves_coordinates_button_and_denials_over_both_transports() {
    for unix in [false, true] {
        for status in [200, 403, 409] {
            let (response, wire) = roundtrip(
                unix,
                status,
                "application/json",
                br#"{"error":"input paused after 2 pointer steps"}"#.to_vec(),
                false,
                call(
                    "mouse_drag",
                    json!({"x":1,"y":2,"to_x":300,"to_y":400,"button":"right"}),
                ),
            );
            assert!(wire.starts_with(&format!("POST /session/{SESSION}/mouse HTTP/1.1")));
            let body: Value = serde_json::from_str(wire.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(
                body,
                json!({"action":"drag","x":1,"y":2,"to_x":300,"to_y":400,"button":"right"})
            );
            assert_eq!(response["result"]["isError"], status != 200);
            if status == 409 {
                assert!(response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("2 pointer steps"));
            }
        }
    }
}

#[test]
fn invalid_drag_and_queued_drag_are_rejected_before_transport() {
    let valid = json!({"x":1,"y":2,"to_x":3,"to_y":4});
    let mut requests = Vec::new();
    for key in ["x", "y", "to_x", "to_y"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        requests.push(call("mouse_drag", missing));
        for bad in [json!(-1), json!(32768), json!(1.5), json!(null), json!("3")] {
            let mut args = valid.clone();
            args[key] = bad;
            requests.push(call("mouse_drag", args.clone()));
            requests.push(call(
                "task_submit",
                json!({"steps":[{"tool":"mouse_drag","arguments":args}]}),
            ));
        }
    }
    for bad in [json!("Left"), json!("wheel"), json!(null)] {
        let mut args = valid.clone();
        args["button"] = bad;
        requests.push(call("mouse_drag", args));
    }
    for response in run("http://127.0.0.1:0", false, SESSION, &requests) {
        assert_eq!(response["error"]["code"], -32602);
    }
}

#[test]
fn named_key_chords_preserve_balanced_http_requests_and_denials() {
    for unix in [false, true] {
        for status in [200, 403, 409] {
            let (response, wire) = roundtrip(
                unix,
                status,
                "application/json",
                br#"{"error":"paused"}"#.to_vec(),
                false,
                call("key_chord", json!({"keys":["ctrl","s"]})),
            );
            assert!(wire.starts_with(&format!("POST /session/{SESSION}/type HTTP/1.1")));
            let body: Value = serde_json::from_str(wire.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(
                body,
                json!({"keys":[{"key":"ctrl","action":"down"},{"key":"s","action":"down"},{"key":"s","action":"up"},{"key":"ctrl","action":"up"}]})
            );
            assert_eq!(response["result"]["isError"], status != 200);
        }
    }
    let mut requests = Vec::new();
    for keys in [
        json!([]),
        json!(["ctrl", "control"]),
        json!(["unknown"]),
        json!([null]),
        json!(vec!["a"; 9]),
    ] {
        requests.push(call("key_chord", json!({"keys":keys})));
        requests.push(call(
            "task_submit",
            json!({"steps":[{"tool":"key_chord","arguments":{"keys":keys}}]}),
        ));
    }
    for result in run("http://127.0.0.1:0", false, SESSION, &requests) {
        assert_eq!(result["error"]["code"], -32602);
    }
}

#[test]
fn task_resources_read_only_the_assigned_session_and_preserve_results() {
    let task = "00000000-0000-0000-0000-000000000002";
    let uri = format!("visor-task://{SESSION}/{task}");
    let body = json!({"id":task,"status":"completed","results":[{"type":"text","text":"done"},{"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(png())}]});
    for unix in [false, true] {
        let request =
            json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":uri}});
        let (response, wire) = roundtrip(
            unix,
            200,
            "application/json",
            serde_json::to_vec(&body).unwrap(),
            true,
            request.clone(),
        );
        assert!(wire.starts_with(&format!("GET /session/{SESSION}/tasks/{task} HTTP/1.1")));
        if !unix {
            assert!(wire
                .to_lowercase()
                .contains("authorization: bearer test-bearer"));
        }
        let resource = &response["result"]["contents"][0];
        assert_eq!(resource["uri"], uri);
        assert_eq!(resource["mimeType"], "application/json");
        assert_eq!(
            serde_json::from_str::<Value>(resource["text"].as_str().unwrap()).unwrap(),
            body
        );
        for (status, code) in [(401, -32001), (403, -32001), (404, -32002), (500, -32603)] {
            let (response, _) = roundtrip(
                unix,
                status,
                "application/json",
                br#"{"error":"private backend diagnostic"}"#.to_vec(),
                false,
                request.clone(),
            );
            assert_eq!(response["error"]["code"], code);
            assert!(!response.to_string().contains("private backend diagnostic"));
        }
    }
}

#[test]
fn resource_discovery_is_bounded_and_foreign_or_malformed_uris_never_connect() {
    let task = "00000000-0000-0000-0000-000000000002";
    let messages = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"resources/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"resources/templates/list"}),
    ];
    let responses = run("http://127.0.0.1:0", false, SESSION, &messages);
    assert_eq!(responses[0]["result"], json!({"resources":[]}));
    assert_eq!(
        responses[1]["result"]["resourceTemplates"][0]["uriTemplate"],
        format!("visor-task://{SESSION}/{{task_id}}")
    );
    let mut invalid = vec![];
    for uri in [
        format!("visor-task://{task}/{task}"),
        format!("visor-task://{SESSION}/{task}?token=secret"),
        format!("visor-task://{SESSION}/{task}/../other"),
        "file:///etc/passwd".into(),
        "https://example.test".into(),
    ] {
        invalid
            .push(json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":uri}}));
    }
    invalid.push(
        json!({"jsonrpc":"2.0","id":1,"method":"resources/list","params":{"cursor":"invented"}}),
    );
    for response in run("http://127.0.0.1:0", false, SESSION, &invalid) {
        assert_eq!(response["error"]["code"], -32602);
    }
}

#[test]
fn reading_a_terminal_resource_reclaims_connection_tracking() {
    resource_tracking_case(
        200,
        "application/json",
        br#"{"id":"00000000-0000-0000-0000-000000000002","status":"completed","results":[]}"#,
        true,
    );
}

#[test]
fn only_confirmed_missing_resource_reads_reclaim_tracking() {
    resource_tracking_case(
        404,
        "application/json; charset=utf-8",
        br#"{"error":"session closed or unknown"}"#,
        true,
    );
    resource_tracking_case(
        404,
        "text/html",
        br#"{"error":"session closed or unknown"}"#,
        false,
    );
    resource_tracking_case(
        404,
        "application/json",
        br#"{"error":"proxy not found"}"#,
        false,
    );
    resource_tracking_case(
        403,
        "application/json",
        br#"{"error":"session closed or unknown"}"#,
        false,
    );
    resource_tracking_case(
        500,
        "application/json",
        br#"{"error":"session closed or unknown"}"#,
        false,
    );
}

fn resource_tracking_case(status: u16, mime: &'static str, body: &'static [u8], reclaimed: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = "00000000-0000-0000-0000-000000000002";
    let uri = format!("visor-task://{SESSION}/{task}");
    let server = std::thread::spawn(move || {
        let (stream, _) = accept_bounded!(listener);
        let wire = exchange(
            stream,
            202,
            "application/json",
            serde_json::to_string(&json!({"id":task,"status":"queued"}))
                .unwrap()
                .as_bytes(),
            false,
        );
        assert!(wire.starts_with(&format!("POST /session/{SESSION}/tasks HTTP/1.1")));
        let (stream, _) = accept_bounded!(listener);
        let wire = exchange(stream, status, mime, body, false);
        assert!(wire.starts_with(&format!("GET /session/{SESSION}/tasks/{task} HTTP/1.1")));
        if !reclaimed {
            let (stream, _) = accept_bounded!(listener);
            let wire = exchange(
                stream,
                200,
                "application/json",
                serde_json::to_string(&json!({"id":task,"status":"cancelled"}))
                    .unwrap()
                    .as_bytes(),
                false,
            );
            assert!(wire.starts_with(&format!("DELETE /session/{SESSION}/tasks/{task} HTTP/1.1")));
        }
    });
    let replies = run(
        &endpoint,
        false,
        SESSION,
        &[
            call(
                "task_submit",
                json!({"steps":[{"tool":"view","arguments":{}}]}),
            ),
            json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":uri}}),
            json!({"jsonrpc":"2.0","id":4,"method":"resources/list"}),
        ],
    );
    server.join().unwrap();
    assert_eq!(replies[1]["result"]["resources"][0]["uri"], uri);
    assert_eq!(
        replies[3]["result"]["resources"]
            .as_array()
            .unwrap()
            .is_empty(),
        reclaimed
    );
    if status == 404 {
        assert_eq!(replies[2]["error"]["code"], -32002);
    }
}

struct InteractiveClient {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    output: std::sync::mpsc::Receiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl InteractiveClient {
    fn new(endpoint: &str, unix: bool) -> Self {
        use std::io::BufRead;
        let mut command = Command::new(env!("CARGO_BIN_EXE_hypermesh-desktop-mcp"));
        command
            .env("HYPERMESH_SESSION_ID", SESSION)
            .env_remove("APPLICATIONINSIGHTS_CONNECTION_STRING")
            .env_remove("HYPERMESH_VISOR_TOKEN")
            .env_remove("HYPERMESH_VISOR_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if unix {
            command.env("HYPERMESH_RUNTIME_DIR", endpoint);
        } else {
            command
                .env("HYPERMESH_VISOR_URL", endpoint)
                .env("HYPERMESH_VISOR_TOKEN", "test-bearer");
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            reader: Some(reader),
        };
        client.send(initialize("2024-11-05"));
        assert_eq!(client.receive()["id"], "handshake");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, message: Value) {
        writeln!(self.input.as_mut().unwrap(), "{message}").unwrap();
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("MCP reply exceeded cancellation deadline")
    }
    fn finish(mut self) -> Vec<Value> {
        self.input.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "MCP did not stop after EOF"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.reader.take().unwrap().join().unwrap();
        self.output.try_iter().collect()
    }
}
impl Drop for InteractiveClient {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_stalled_request(mut stream: impl Read, ready: std::sync::mpsc::Sender<()>) {
    let mut request = Vec::new();
    let mut byte = [0];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
    }
    assert!(String::from_utf8(request)
        .unwrap()
        .starts_with(&format!("POST /session/{SESSION}/view HTTP/1.1")));
    ready.send(()).unwrap();
    let mut rest = Vec::new();
    stream
        .read_to_end(&mut rest)
        .expect("cancelled transport was not closed");
    assert!(rest.is_empty());
}

#[test]
fn cancellation_closes_stalled_unix_and_http_requests_and_skips_queued_input() {
    for unix in [false, true] {
        let directory = std::env::temp_dir().join(format!("hm-cancel-{}", uuid::Uuid::new_v4()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (endpoint, server) = if unix {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let socket = directory.join("control.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
            let endpoint = directory.to_str().unwrap().to_owned();
            (
                endpoint,
                std::thread::spawn(move || {
                    let (stream, _) = accept_bounded!(listener);
                    wait_for_stalled_request(stream, ready_tx);
                    assert!(
                        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
                    );
                }),
            )
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            (
                endpoint,
                std::thread::spawn(move || {
                    let (stream, _) = accept_bounded!(listener);
                    wait_for_stalled_request(stream, ready_tx);
                    assert!(
                        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
                    );
                }),
            )
        };
        let mut client = InteractiveClient::new(&endpoint, unix);
        client.send(call("view", json!({})));
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"must not execute"}}}));
        for id in [2, 1] {
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}}));
        }
        client.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
        assert_eq!(
            client.receive(),
            json!({"jsonrpc":"2.0","id":3,"result":{}})
        );
        assert!(client.finish().is_empty());
        server.join().unwrap();
        if unix {
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn lost_stdout_stops_queued_work_even_when_stdin_remains_open() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypermesh-desktop-mcp"))
        .env("HYPERMESH_SESSION_ID", SESSION)
        .env(
            "HYPERMESH_VISOR_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .env("HYPERMESH_VISOR_TOKEN", "test-bearer")
        .env_remove("APPLICATIONINSIGHTS_CONNECTION_STRING")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut input = child.stdin.take().unwrap();
    let messages = format!(
        "{}\n{}\n{}\n",
        initialize("2024-11-05"),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        call("view", json!({}))
    );
    let _ = input.write_all(messages.as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("adapter waited on stdin after losing stdout");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn cancellation_interrupts_a_stalled_tls_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = accept_bounded!(listener);
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], 22); // TLS handshake record, before any HTTP request.
        ready_tx.send(()).unwrap();
        let mut remaining = Vec::new();
        stream
            .read_to_end(&mut remaining)
            .expect("TLS connection stayed open after cancellation");
    });
    let mut client = InteractiveClient::new(&endpoint, false);
    client.send(call("view", json!({})));
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}));
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    assert_eq!(
        client.receive(),
        json!({"jsonrpc":"2.0","id":3,"result":{}})
    );
    // A cancellation arriving after a response must not affect another request.
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}));
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    assert_eq!(
        client.receive(),
        json!({"jsonrpc":"2.0","id":4,"result":{}})
    );
    assert!(client.finish().is_empty());
    server.join().unwrap();
}
