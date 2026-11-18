//! Private machine coding consumer. Plane retains durable task authority.
use super::*;
use serde_json::{json, Value};

fn current(state: &AppState, consent: tasks::Consent) -> bool {
    state.has_session(consent.session) && consent.matches(&lock(&state.automation))
}
fn context_matches(value: &Value, session: Uuid) -> bool {
    acp_machine::binding(value, value["workspace"].as_str().unwrap_or("")) == Ok(session)
}
fn metadata(value: &Value) -> Result<Value, ()> {
    if value["version"] != 1 {
        return Err(());
    }
    let status = value["state"].as_str().ok_or(())?;
    if status == "idle" {
        return Ok(json!({"version":1,"state":"idle"}));
    }
    let id = Uuid::parse_str(value["task_id"].as_str().ok_or(())?).map_err(|_| ())?;
    if id.is_nil() || !matches!(status, "unknown" | "output-observed") {
        return Err(());
    }
    let mut result = json!({"version":1,"task_id":id,"state":status});
    if let Some(digest) = value["output_sha256"].as_str() {
        if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(());
        }
        result["output_sha256"] = json!(digest);
    }
    Ok(result)
}

pub(super) async fn status(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Value>, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    let value = state
        .host_bridge
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Coding target unavailable"))?
        .machine_context()
        .await
        .map_err(|_| ApiError::unavailable("Coding target unavailable"))?;
    let saved = lock(&state.coding_status);
    if saved["identity"] != value["identity"] || saved["workspace"] != value["workspace"] {
        // Expiring context timestamps are deliberately excluded below.
        if !same_scope(&saved["identity"], &value["identity"])
            || saved["workspace"] != value["workspace"]
        {
            return Ok(Json(json!({"version":1,"state":"idle"})));
        }
    }
    Ok(Json(saved["result"].clone()))
}
fn same_scope(a: &Value, b: &Value) -> bool {
    [
        "tenant_id",
        "owner_id",
        "principal",
        "organization_id",
        "device_id",
        "bot_id",
        "assignment_id",
        "session_id",
    ]
    .iter()
    .all(|key| a[*key].is_string() && a[*key] == b[*key])
}

async fn cancellable(
    request: impl std::future::Future<Output = Result<Value, ()>>,
    stop: &mut tokio::sync::watch::Receiver<bool>,
    current: impl Fn() -> bool,
) -> Result<Value, ()> {
    if *stop.borrow() || !current() {
        return Err(());
    }
    tokio::pin!(request);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            value=&mut request=>return if current() && !*stop.borrow() {value.and_then(|v|metadata(&v))} else {Err(())},
            _=async {let _ = stop.wait_for(|v|*v).await;}=>return Err(()),
            _=interval.tick()=>if !current(){return Err(())},
        }
    }
}

async fn context_or_stop(
    request: impl std::future::Future<Output = Result<Value, ()>>,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Value, ()> {
    if *stop.borrow() {
        return Err(());
    }
    tokio::select! {value=request=>value,_=stop.wait_for(|v|*v)=>Err(())}
}

pub(super) async fn run(
    state: Arc<AppState>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    let Some(bridge) = state.host_bridge.as_ref() else {
        let _ = stop.wait_for(|v| *v).await;
        return Ok(());
    };
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let consent = {
            let gate = lock(&state.automation);
            gate.session()
                .filter(|_| gate.phase() == Phase::Running)
                .map(|session| tasks::Consent::capture(&gate, session))
        };
        if let Some(consent) = consent {
            if current(&state, consent) {
                let context = context_or_stop(bridge.machine_context(), &mut stop).await;
                if let Ok(context) = context {
                    if context_matches(&context, consent.session) && current(&state, consent) {
                        let result = cancellable(bridge.coding_run(), &mut stop, || {
                            current(&state, consent)
                        })
                        .await;
                        if current(&state, consent) {
                            let result =
                                result.unwrap_or_else(|_| json!({"version":1,"state":"unknown"}));
                            if result["state"] != "idle" {
                                *lock(&state.coding_status) = json!({"identity":context["identity"],"workspace":context["workspace"],"result":result});
                            }
                        }
                    }
                }
            }
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>{},_=stop.wait_for(|v|*v)=>return Ok(())}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_is_metadata_not_authority() {
        let id = Uuid::new_v4();
        let value = json!({"version":1,"task_id":id,"state":"output-observed","output_sha256":"a".repeat(64),"prompt":"private","next_command":"shell"});
        let safe = metadata(&value).unwrap();
        assert!(safe.get("prompt").is_none());
        assert!(safe.get("next_command").is_none());
        assert!(metadata(&json!({"version":1,"task_id":id,"state":"execute"})).is_err());
    }
    #[test]
    fn changed_scope_cannot_read_previous_results() {
        let mut a = json!({});
        for key in [
            "tenant_id",
            "owner_id",
            "principal",
            "organization_id",
            "device_id",
            "bot_id",
            "assignment_id",
            "session_id",
        ] {
            a[key] = json!(Uuid::new_v4());
        }
        assert!(same_scope(&a, &a));
        let mut b = a.clone();
        b["device_id"] = json!(Uuid::new_v4());
        assert!(!same_scope(&a, &b));
        assert!(!same_scope(&json!({}), &json!({})));
    }
    #[tokio::test]
    async fn pause_drops_active_request_and_late_output() {
        let allowed = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicBool::new(false));
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let (start, ready) = tokio::sync::oneshot::channel();
        let flag = finished.clone();
        let permit = allowed.clone();
        let job = tokio::spawn(async move {
            cancellable(
                async move {
                    let _guard = Guard(flag);
                    let _ = start.send(());
                    std::future::pending::<Result<Value, ()>>().await
                },
                &mut rx,
                || permit.load(Ordering::SeqCst),
            )
            .await
        });
        ready.await.unwrap();
        allowed.store(false, Ordering::SeqCst);
        assert!(tokio::time::timeout(Duration::from_secs(1), job)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(finished.load(Ordering::SeqCst));
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        assert!(cancellable(
            async { Ok(json!({"version":1,"state":"idle"})) },
            &mut rx,
            || false
        )
        .await
        .is_err());
    }
    #[tokio::test]
    async fn stalled_context_is_cancelled_by_shutdown() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let job = tokio::spawn(async move {
            context_or_stop(std::future::pending::<Result<Value, ()>>(), &mut rx).await
        });
        tx.send(true).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), job)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }
    #[cfg(feature = "headless")]
    #[tokio::test]
    async fn disabled_worker_waits_for_shutdown() {
        let state = Arc::new(AppState::new(
            Arc::new(crate::desktop::HeadlessDesktop),
            Duration::from_secs(1),
        ));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let job = tokio::spawn(run(state, rx));
        tokio::task::yield_now().await;
        assert!(!job.is_finished());
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
