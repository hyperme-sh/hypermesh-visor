//! Opt-in outbound bot polling. Remote control never grants native consent.
use super::*;
use serde_json::{json, Value};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    origin: String,
    tenant_id: Uuid,
    bot_id: Uuid,
    session_id: Uuid,
    mode: String,
    credential_file: PathBuf,
}

pub(super) struct Client {
    settings: Settings,
    credential: zeroize::Zeroizing<String>,
    connection: Uuid,
    agent: ureq::Agent,
}

fn private_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "bot configuration cannot be opened")?;
    let meta = file
        .metadata()
        .map_err(|_| "bot configuration cannot be inspected")?;
    if !meta.is_file()
        || meta.uid() != crate::current_euid()
        || meta.mode() & 0o077 != 0
        || meta.len() > 8192
    {
        return Err("bot configuration must be an owner-only regular file under 8 KiB".into());
    }
    let mut data = Vec::new();
    file.take(8193)
        .read_to_end(&mut data)
        .map_err(|_| "bot configuration cannot be read")?;
    if data.len() > 8192 {
        return Err("bot configuration is too large".into());
    }
    Ok(data)
}

fn validate_origin(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|_| "invalid bot service origin")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("bot service must be a credential-free HTTPS origin".into());
    }
    Ok(url.origin().ascii_serialization())
}

pub(super) fn configured() -> Result<Option<Arc<Client>>, String> {
    let Some(path) = std::env::var_os("HYPERMESH_BOT_CONFIG") else {
        return Ok(None);
    };
    let mut settings: Settings =
        serde_json::from_slice(&private_file(std::path::Path::new(&path))?)
            .map_err(|_| "invalid bot configuration")?;
    settings.origin = validate_origin(&settings.origin)?;
    if !matches!(settings.mode.as_str(), "interactive" | "container")
        || !settings.credential_file.is_absolute()
    {
        return Err("bot mode and absolute credential file are required".into());
    }
    let bytes = zeroize::Zeroizing::new(private_file(&settings.credential_file)?);
    let credential = std::str::from_utf8(&bytes)
        .map_err(|_| "invalid bot credential")?
        .trim();
    if credential.len() != 71
        || !credential.starts_with("hm_bot_")
        || !credential[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid scoped bot credential".into());
    }
    Ok(Some(Arc::new(Client {
        settings,
        credential: zeroize::Zeroizing::new(credential.into()),
        connection: Uuid::new_v4(),
        agent: ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(2))
            .build(),
    })))
}

impl Client {
    async fn post(self: &Arc<Self>, action: &'static str, body: Value) -> Result<Value, String> {
        let client = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let target = format!(
                "{}/api/v1/hypermesh/bots/{}/agent/{action}",
                client.settings.origin, client.settings.bot_id
            );
            let response = client
                .agent
                .post(&target)
                .set("Authorization", &format!("Bearer {}", &*client.credential))
                .set("X-Tenant-ID", &client.settings.tenant_id.to_string())
                .set("Content-Type", "application/json")
                .send_string(&body.to_string())
                .map_err(|_| "bot service request failed")?;
            if response.status() != 200 {
                return Err("bot service refused request".into());
            }
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(1_048_577)
                .read_to_end(&mut bytes)
                .map_err(|_| "bot response read failed")?;
            if bytes.len() > 1_048_576 {
                return Err("bot response too large".to_string());
            }
            serde_json::from_slice(&bytes).map_err(|_| "invalid bot response".to_string())
        })
        .await
        .map_err(|_| "bot request worker failed")?
    }

    fn status_matches(&self, value: &Value) -> bool {
        value["bot_id"].as_str() == Some(self.settings.bot_id.to_string().as_str())
            && value["tenant_id"].as_str() == Some(self.settings.tenant_id.to_string().as_str())
            && value["mode"].as_str() == Some(self.settings.mode.as_str())
            && value["revoked"] == false
    }
}

fn task_binding(task: &Value, bot: &Value, session: Uuid) -> bool {
    task["capability"] == "desktop"
        && task["session_id"].as_str() == Some(session.to_string().as_str())
        && bot["assignment"]["session_id"] == task["session_id"]
        && task["assignment_id"].is_string()
        && bot["assignment"]["id"] == task["assignment_id"]
        && task["status"] == "delivered"
        && Uuid::parse_str(task["id"].as_str().unwrap_or("")).is_ok()
}

fn remaining(deadline: f64, now: f64) -> Result<Duration, String> {
    let seconds = deadline - now;
    if !deadline.is_finite()
        || !now.is_finite()
        || !(0.0..=15.0).contains(&seconds)
        || seconds == 0.0
    {
        return Err("bot authorization expired or clock differs".into());
    }
    Ok(Duration::from_secs_f64(seconds))
}

pub(super) async fn run(
    client: Arc<Client>,
    state: Arc<AppState>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    let result = poll(client, Arc::clone(&state), &mut stop).await;
    if let Err(message) = result {
        lock(&state.automation).pause();
        return Err(std::io::Error::other(message));
    }
    Ok(())
}

async fn poll(
    client: Arc<Client>,
    state: Arc<AppState>,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let session = client.settings.session_id;
    let mut active: Option<(String, String)> = None;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let consent =
            state.has_session(session) && lock(&state.automation).phase() == Phase::Running;
        let status = client
            .post(
                "heartbeat",
                json!({"connection_id":client.connection,"local_consent":consent}),
            )
            .await?;
        if !client.status_matches(&status) {
            return Err("bot registration binding differs".into());
        }
        if let Some((cloud_id, local_id)) = &active {
            let cancelled = status["tasks"]
                .as_array()
                .and_then(|items| items.iter().find(|t| t["id"] == *cloud_id))
                .map(|t| {
                    t["cancel_requested"] == true
                        || t["status"] == "unknown"
                        || t["status"] == "cancelled"
                })
                .unwrap_or(true);
            if status["desired"] != "running" || !consent || cancelled {
                lock(&state.automation).pause();
                let _ = tasks::cancel(
                    State(Arc::clone(&state)),
                    Path((session.to_string(), local_id.clone())),
                )
                .await;
            }
            let Json(local) = tasks::status(
                State(Arc::clone(&state)),
                Path((session.to_string(), local_id.clone())),
            )
            .await
            .map_err(|_| "local bot task unavailable")?;
            if matches!(
                local["status"].as_str(),
                Some("completed" | "failed" | "cancelled")
            ) {
                client.post("complete", json!({"connection_id":client.connection,"task_id":cloud_id,"status":local["status"]})).await?;
                active = None;
            }
        } else if consent && status["desired"] == "running" {
            let reply = client
                .post("claim", json!({"connection_id":client.connection}))
                .await?;
            if !client.status_matches(&reply["bot"]) {
                return Err("bot claim binding differs".into());
            }
            if !reply["task"].is_null() {
                let task = &reply["task"];
                if !task_binding(task, &reply["bot"], session)
                    || task["connection_id"] != client.connection.to_string()
                {
                    return Err("bot task assignment differs".into());
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| "invalid system clock")?
                    .as_secs_f64();
                let deadline = remaining(
                    reply["authorized_until"]
                        .as_f64()
                        .ok_or("missing bot deadline")?,
                    now,
                )?;
                let consent_word = {
                    let gate = lock(&state.automation);
                    if gate.phase() != Phase::Running {
                        return Err("local bot consent changed".into());
                    }
                    automation::pack_snap(gate.epoch(), gate.phase())
                };
                let body = serde_json::from_value(task["payload"].clone())
                    .map_err(|_| "invalid bot task payload")?;
                let (_, Json(local)) = tasks::submit_bounded(
                    Arc::clone(&state),
                    session.to_string(),
                    body,
                    deadline,
                    Some(consent_word),
                )
                .await
                .map_err(|_| "bot task refused locally")?;
                active = Some((
                    task["id"].as_str().unwrap().into(),
                    local["id"].as_str().ok_or("missing local task ID")?.into(),
                ));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            _ = stop.wait_for(|v| *v) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_rejects_plaintext_redirect_paths_and_embedded_credentials() {
        for origin in [
            "http://localhost",
            "https://user:secret@example.test",
            "https://example.test/path",
            "https://example.test/?token=x",
            "https://example.test/#x",
        ] {
            assert!(validate_origin(origin).is_err());
        }
        assert_eq!(
            validate_origin("https://example.test:8443/").unwrap(),
            "https://example.test:8443"
        );
    }
    #[test]
    fn deadline_is_bounded_and_never_extended() {
        assert_eq!(remaining(110.0, 100.0).unwrap(), Duration::from_secs(10));
        for deadline in [100.0, 99.0, 116.0, f64::NAN, f64::INFINITY] {
            assert!(remaining(deadline, 100.0).is_err());
        }
    }
    #[test]
    fn task_requires_exact_local_session_and_remote_assignment() {
        let session = Uuid::new_v4();
        let assignment = Uuid::new_v4();
        let mut task = json!({"id":Uuid::new_v4(),"session_id":session,"assignment_id":assignment,"capability":"desktop","status":"delivered"});
        let bot = json!({"assignment":{"id":assignment,"session_id":session}});
        assert!(task_binding(&task, &bot, session));
        assert!(!task_binding(&task, &bot, Uuid::new_v4()));
        task["assignment_id"] = json!(Uuid::new_v4());
        assert!(!task_binding(&task, &bot, session));
    }
    #[test]
    fn private_configuration_refuses_symlinks_and_shared_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("visor-bot-private-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, b"fixture").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(private_file(&path).unwrap(), b"fixture");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(private_file(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_file(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn transport_sends_scoped_headers_and_refuses_redirects() {
        use std::io::Write;
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\n\r\n",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let tenant = Uuid::new_v4();
            let bot = Uuid::new_v4();
            let credential = format!("hm_bot_{}", "a".repeat(64));
            let expected = credential.clone();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
                assert!(headers.contains(&format!("/api/v1/hypermesh/bots/{bot}/agent/heartbeat")));
                assert!(headers.contains(&format!("authorization: bearer {expected}")));
                assert!(headers.contains(&format!("x-tenant-id: {tenant}")));
                let length: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["local_consent"],
                    false
                );
                socket.write_all(response.as_bytes()).unwrap();
            });
            let client = Arc::new(Client {
                settings: Settings {
                    origin: format!("http://{addr}"),
                    tenant_id: tenant,
                    bot_id: bot,
                    session_id: Uuid::new_v4(),
                    mode: "interactive".into(),
                    credential_file: PathBuf::new(),
                },
                credential: zeroize::Zeroizing::new(credential),
                connection: Uuid::new_v4(),
                agent: ureq::AgentBuilder::new()
                    .redirects(0)
                    .timeout(Duration::from_secs(2))
                    .build(),
            });
            let result = client
                .post("heartbeat", json!({"local_consent":false}))
                .await;
            assert_eq!(result.is_ok(), response.starts_with("HTTP/1.1 200"));
            if let Err(error) = result {
                assert!(!error.contains("hm_bot_"));
            }
            server.join().unwrap();
        }
    }
}
