//! Bounded session-owned desktop jobs. Every step uses the normal verb handlers.
use super::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use tokio::sync::Semaphore;

const MAX_TASKS: usize = 16;
const MAX_STEPS: usize = 16;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(120);

pub(super) struct Queue {
    records: Mutex<VecDeque<Record>>,
    serial: Arc<Semaphore>,
    slots: Arc<Semaphore>,
}

struct Record {
    id: Uuid,
    session: Uuid,
    status: &'static str,
    completed: usize,
    total: usize,
    results: Vec<Value>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}

impl Queue {
    pub(super) fn new() -> Self {
        Self {
            records: Mutex::new(VecDeque::new()),
            serial: Arc::new(Semaphore::new(1)),
            slots: Arc::new(Semaphore::new(MAX_TASKS)),
        }
    }

    pub(super) fn close(&self, session: Uuid) {
        let mut records = lock(&self.records);
        for record in records.iter().filter(|r| r.session == session) {
            record.cancel.store(true, Ordering::SeqCst);
        }
        records.retain(|r| r.session != session);
    }
}

impl Record {
    fn terminal(&self) -> bool {
        matches!(self.status, "completed" | "failed" | "cancelled")
    }

    fn snapshot(&self) -> Value {
        json!({"id":self.id,"session":self.session,"status":self.status,
            "completed_steps":self.completed,"total_steps":self.total,
            "cancel_requested":self.cancel.load(Ordering::SeqCst),
            "results":self.results,"error":self.error})
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submission {
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    tool: String,
    arguments: Value,
}

enum Action {
    View,
    Mouse(MouseOp),
    Type(String),
    Chord(TypeBody),
}

fn action(step: Step) -> Result<Action, ApiError> {
    let args = step
        .arguments
        .as_object()
        .ok_or_else(|| ApiError::bad("arguments must be an object"))?;
    let allowed: &[&str] = match step.tool.as_str() {
        "view" => &[],
        "mouse_move" => &["x", "y"],
        "mouse_click" => &["x", "y", "button"],
        "mouse_drag" => &["x", "y", "to_x", "to_y", "button"],
        "mouse_scroll" => &["x", "y", "delta_x", "delta_y"],
        "type_text" => &["text"],
        "key_chord" => &["keys"],
        _ => return Err(ApiError::bad("unsupported task tool")),
    };
    if args.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(ApiError::bad("unexpected task argument"));
    }
    let coordinate = |key: &str| -> Result<i32, ApiError> {
        args.get(key)
            .and_then(Value::as_i64)
            .filter(|n| (0..=32767).contains(n))
            .map(|n| n as i32)
            .ok_or_else(|| ApiError::bad("task coordinates must be integers from 0 to 32767"))
    };
    Ok(match step.tool.as_str() {
        "view" => Action::View,
        "key_chord" => {
            let value = crate::key_chord_request(args.get("keys").unwrap_or(&Value::Null))
                .map_err(ApiError::bad)?;
            let body = serde_json::from_value(value).map_err(|_| ApiError::bad("invalid chord"))?;
            Action::Chord(body)
        }
        "type_text" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.chars().count() <= 4096)
                .ok_or_else(|| ApiError::bad("task text must contain 1 to 4096 characters"))?;
            let body = TypeBody {
                text: Some(text.to_owned()),
                keys: Vec::new(),
                secret: None,
            };
            let planned = plan_type(&body, None).map_err(ApiError::from_plan)?;
            if keystroke_count(&planned) > TYPE_MAX_KEYSTROKES {
                return Err(ApiError::bad("task typing exceeds the keystroke limit"));
            }
            Action::Type(text.to_owned())
        }
        "mouse_move" => Action::Mouse(MouseOp::Move {
            x: coordinate("x")?,
            y: coordinate("y")?,
            clickable: false,
        }),
        "mouse_click" => {
            let button = match args.get("button") {
                None => Button::Left,
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|_| ApiError::bad("invalid button"))?,
            };
            Action::Mouse(MouseOp::Click {
                x: coordinate("x")?,
                y: coordinate("y")?,
                button,
            })
        }
        "mouse_drag" => {
            let button = match args.get("button").and_then(Value::as_str) {
                None if !args.contains_key("button") => Button::Left,
                Some("left") => Button::Left,
                Some("right") => Button::Right,
                Some("middle") => Button::Middle,
                _ => return Err(ApiError::bad("invalid button")),
            };
            Action::Mouse(MouseOp::Drag {
                x: coordinate("x")?,
                y: coordinate("y")?,
                to_x: coordinate("to_x")?,
                to_y: coordinate("to_y")?,
                button,
            })
        }
        "mouse_scroll" => {
            let delta = |key: &str| {
                args.get(key)
                    .and_then(Value::as_i64)
                    .filter(|n| (-128..=128).contains(n))
                    .map(|n| n as i32)
                    .ok_or_else(|| ApiError::bad("invalid scroll ticks"))
            };
            let dx = delta("delta_x")?;
            let dy = delta("delta_y")?;
            if !(1..=128).contains(&(dx.abs() + dy.abs())) {
                return Err(ApiError::bad("invalid total scroll ticks"));
            }
            Action::Mouse(MouseOp::Scroll {
                x: coordinate("x")?,
                y: coordinate("y")?,
                delta_x: dx,
                delta_y: dy,
            })
        }
        _ => unreachable!(),
    })
}

pub(super) async fn submit(
    State(state): State<Arc<AppState>>,
    Path(session): Path<String>,
    Json(body): Json<Submission>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    submit_bounded(state, session, body, DEADLINE, None).await
}

#[derive(Clone, Copy)]
pub(super) struct Consent {
    pub(super) word: u64,
    pub(super) session: Uuid,
    pub(super) resume_seq: u64,
}
impl Consent {
    pub(super) fn capture(gate: &automation::Gate, session: Uuid) -> Self {
        Self { word: automation::pack_snap(gate.epoch(), gate.phase()), session, resume_seq: gate.resume_seq() }
    }
    pub(super) fn matches(self, gate: &automation::Gate) -> bool {
        gate.phase() == Phase::Running && gate.session() == Some(self.session) &&
            gate.resume_seq() == self.resume_seq && automation::pack_snap(gate.epoch(), gate.phase()) == self.word
    }
}

/// Short, absolute authorization lease for independently authenticated adapters.
/// A lost adapter cannot leave a task running for the default two-minute budget.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LeasedSubmission {
    task: Submission,
    authorized_until_ms: u64,
    resume_seq: u64,
}

fn lease_duration(until: u64, now: u64) -> Result<Duration, ApiError> {
    let millis = until.checked_sub(now).filter(|v| (1..=15_000).contains(v))
        .ok_or_else(|| ApiError::bad("task authorization lease must be within 15 seconds"))?;
    Ok(Duration::from_millis(millis))
}

pub(super) async fn submit_leased(
    State(state): State<Arc<AppState>>, Path(session): Path<String>, Json(body): Json<LeasedSubmission>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let session_id = parse_id(&session)?;
    let started = Instant::now();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ApiError::bad("invalid clock"))?.as_nanos().div_ceil(1_000_000);
    let now = u64::try_from(now).map_err(|_| ApiError::bad("invalid clock"))?;
    let deadline = lease_duration(body.authorized_until_ms, now)?;
    let consent = {
        let gate = lock(&state.automation);
        if gate.phase() != Phase::Running || gate.session() != Some(session_id) || gate.resume_seq() != body.resume_seq {
            return Err(ApiError::conflict("local consent changed"));
        }
        Consent::capture(&gate, session_id)
    };
    submit_until(state, session, body.task, started + deadline, Some(consent)).await
}

pub(super) async fn submit_bounded(
    state: Arc<AppState>, session: String, body: Submission, deadline: Duration, consent_word: Option<Consent>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if deadline.is_zero() || deadline > DEADLINE {
        return Err(ApiError::bad("invalid task deadline"));
    }
    submit_until(state, session, body, Instant::now() + deadline, consent_word).await
}

async fn submit_until(
    state: Arc<AppState>, session: String, body: Submission, expires_at: Instant, consent_word: Option<Consent>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let session = parse_id(&session)?;
    if expires_at <= Instant::now() { return Err(ApiError::conflict("task authorization expired")); }
    if body.steps.is_empty() || body.steps.len() > MAX_STEPS {
        return Err(ApiError::bad("task requires 1 to 16 steps"));
    }
    let actions: Vec<Action> = body
        .steps
        .into_iter()
        .map(action)
        .collect::<Result<_, _>>()?;
    // Keep session existence and insertion atomic with close_session's removal.
    let sessions = lock(&state.sessions);
    if !sessions.contains_key(&session) {
        return Err(ApiError::missing());
    }
    let slot = Arc::clone(&state.tasks.slots)
        .try_acquire_owned()
        .map_err(|_| ApiError::conflict("task queue is full"))?;
    let id = Uuid::new_v4();
    let cancel = Arc::new(AtomicBool::new(false));
    let snapshot = {
        let mut records = lock(&state.tasks.records);
        if records.len() >= MAX_TASKS {
            if let Some(index) = records.iter().position(Record::terminal) {
                records.remove(index);
            } else {
                return Err(ApiError::conflict("task queue is full"));
            }
        }
        let record = Record {
            id,
            session,
            status: "queued",
            completed: 0,
            total: actions.len(),
            results: Vec::new(),
            error: None,
            cancel: Arc::clone(&cancel),
        };
        let value = record.snapshot();
        records.push_back(record);
        value
    };
    drop(sessions);
    tokio::spawn(async move {
        let _slot = slot;
        run(state, session, id, cancel, actions, expires_at, consent_word).await;
    });
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

pub(super) async fn status(
    State(state): State<Arc<AppState>>,
    Path((session, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let session = parse_id(&session)?;
    let id = parse_id(&id)?;
    let sessions = lock(&state.sessions);
    if !sessions.contains_key(&session) {
        return Err(ApiError::missing());
    }
    lock(&state.tasks.records)
        .iter()
        .find(|r| r.id == id && r.session == session)
        .map(|r| Json(r.snapshot()))
        .ok_or_else(ApiError::missing)
}

pub(super) async fn cancel(
    State(state): State<Arc<AppState>>,
    Path((session, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let session = parse_id(&session)?;
    let id = parse_id(&id)?;
    let sessions = lock(&state.sessions);
    if !sessions.contains_key(&session) {
        return Err(ApiError::missing());
    }
    let mut records = lock(&state.tasks.records);
    let record = records
        .iter_mut()
        .find(|r| r.id == id && r.session == session)
        .ok_or_else(ApiError::missing)?;
    if !record.terminal() {
        record.cancel.store(true, Ordering::SeqCst);
        if record.status == "queued" {
            record.status = "cancelled";
        }
    }
    Ok(Json(record.snapshot()))
}

async fn run(
    state: Arc<AppState>,
    session: Uuid,
    id: Uuid,
    cancel: Arc<AtomicBool>,
    actions: Vec<Action>,
    expires_at: Instant,
    consent_word: Option<Consent>,
) {
    let timer_flag = Arc::clone(&cancel);
    let timer = tokio::spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(expires_at)).await;
        timer_flag.store(true, Ordering::SeqCst);
    });
    let _permit = state
        .tasks
        .serial
        .acquire()
        .await
        .expect("task semaphore stays open");
    let mut size = 0;
    for action in actions {
        if consent_word.is_some_and(|consent| !consent.matches(&lock(&state.automation))) {
            cancel.store(true, Ordering::SeqCst);
        }
        if cancel.load(Ordering::SeqCst) || !state.has_session(session) {
            break;
        }
        {
            let mut records = lock(&state.tasks.records);
            let Some(record) = records.iter_mut().find(|r| r.id == id) else {
                break;
            };
            if record.cancel.load(Ordering::SeqCst) {
                break;
            }
            record.status = "running";
        }
        let result = execute(Arc::clone(&state), session, Arc::clone(&cancel), action, consent_word).await;
        let mut records = lock(&state.tasks.records);
        let Some(record) = records.iter_mut().find(|r| r.id == id) else {
            break;
        };
        match result {
            Ok(value) => {
                size += value.to_string().len();
                if size > MAX_RESULT_BYTES {
                    record.status = "failed";
                    record.error = Some("task result limit exceeded".into());
                    break;
                }
                record.completed += 1;
                record.results.push(value);
            }
            Err(error) => {
                record.status = if cancel.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "failed"
                };
                record.error = Some(error.message);
                break;
            }
        }
    }
    timer.abort();
    if let Some(record) = lock(&state.tasks.records).iter_mut().find(|r| r.id == id) {
        if !record.terminal() {
            record.status = if cancel.load(Ordering::SeqCst) {
                "cancelled"
            } else {
                "completed"
            };
        }
    }
}

async fn execute(
    state: Arc<AppState>,
    session: Uuid,
    cancel: Arc<AtomicBool>,
    action: Action,
    task_epoch: Option<Consent>,
) -> Result<Value, ApiError> {
    match action {
        Action::View => {
            let response = view(State(Arc::clone(&state)), Path(session.to_string())).await?;
            if task_epoch.is_some_and(|consent| !consent.matches(&lock(&state.automation))) {
                cancel.store(true, Ordering::SeqCst);
                return Err(ApiError::conflict("local consent changed"));
            }
            let bytes = axum::body::to_bytes(response.into_body(), MAX_RESULT_BYTES / 2)
                .await
                .map_err(|_| ApiError::bad("task image exceeds result limit"))?;
            Ok(
                json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
            )
        }
        Action::Mouse(op) => {
            mouse_with_cancel(state, session.to_string(), op, Some(cancel), task_epoch).await?;
            Ok(json!({"type":"text","text":"mouse operation completed"}))
        }
        Action::Chord(body) => {
            type_with_cancel(state, session.to_string(), body, Some(cancel), task_epoch).await?;
            Ok(json!({"type":"text","text":"key chord completed"}))
        }
        Action::Type(text) => {
            let body = serde_json::from_value(json!({"text":text}))
                .map_err(|_| ApiError::bad("invalid type request"))?;
            type_with_cancel(state, session.to_string(), body, Some(cancel), task_epoch).await?;
            Ok(json!({"type":"text","text":"typing completed"}))
        }
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    #[test]
    fn queued_drag_uses_the_existing_mouse_operation_and_validates_all_endpoints() {
        let args = json!({"x":1,"y":2,"to_x":300,"to_y":400});
        let parsed = action(Step {
            tool: "mouse_drag".into(),
            arguments: args.clone(),
        })
        .unwrap_or_else(|_| panic!("valid drag rejected"));
        assert!(matches!(
            parsed,
            Action::Mouse(MouseOp::Drag {
                x: 1,
                y: 2,
                to_x: 300,
                to_y: 400,
                button: Button::Left
            })
        ));
        for key in ["x", "y", "to_x", "to_y"] {
            for bad in [json!(-1), json!(32768), json!(null), json!(1.5)] {
                let mut invalid = args.clone();
                invalid[key] = bad;
                assert!(action(Step {
                    tool: "mouse_drag".into(),
                    arguments: invalid
                })
                .is_err());
            }
        }
        for bad in [json!("Left"), json!("wheel"), json!(null)] {
            let mut invalid = args.clone();
            invalid["button"] = bad;
            assert!(action(Step {
                tool: "mouse_drag".into(),
                arguments: invalid
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    #[test]
    fn absolute_lease_is_short_and_never_wraps_or_extends() {
        assert_eq!(lease_duration(1_015_000, 1_000_000).unwrap_or_else(|_| panic!("valid lease")).as_millis(), 15_000);
        for until in [0, 999_999, 1_000_000, 1_015_001, u64::MAX] {
            assert!(lease_duration(until, 1_000_000).is_err());
        }
    }
}
