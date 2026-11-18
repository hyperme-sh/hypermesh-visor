//! Native controls for the single owner-selected Continue task.
use crate::mcp_dialog::{args, choose, dialog};
use serde_json::{json, Value};

pub fn run(workspace: &str) -> Result<(), String> {
    let mut identity = None;
    screen(
        workspace,
        |path, body| {
            let active = crate::visor::trust_request("/continue/context", None)?;
            verify_scope(&active, workspace, &mut identity)?;
            let result = crate::visor::trust_request(path, body)?;
            let fresh = crate::visor::trust_request("/continue/context", None)?;
            verify_scope(&fresh, workspace, &mut identity)?;
            Ok(result)
        },
        dialog,
    )
}
pub fn advice_exchange(workspace: &str) -> Result<Value, String> {
    let active = crate::visor::trust_request("/continue/context", None)?;
    let mut identity = None;
    verify_scope(&active, workspace, &mut identity)?;
    let mut result = crate::visor::trust_request(
        "/continue/advice",
        Some(json!({"workspace":workspace,"session_id":active["session_id"]})),
    )?;
    let fresh = crate::visor::trust_request("/continue/context", None)?;
    verify_scope(&fresh, workspace, &mut identity)?;
    result["workspace"] = json!(workspace);
    Ok(result)
}
pub fn exchange(workspace: &str, control: Option<Value>) -> Result<Value, String> {
    let active = crate::visor::trust_request("/continue/context", None)?;
    let mut identity = None;
    verify_scope(&active, workspace, &mut identity)?;
    let mut body = json!({"workspace":workspace,"session_id":active["session_id"]});
    let path = if let Some(control) = control {
        let object = control.as_object().ok_or("Invalid task control")?;
        let handoff = matches!(
            control["action"].as_str(),
            Some("restart" | "reassign" | "retire")
        );
        if object.len() != if handoff { 4 } else { 3 }
            || (handoff && !object.contains_key("request_id"))
            || !["run_id", "revision", "action"]
                .iter()
                .all(|k| object.contains_key(*k))
        {
            return Err("Invalid task control".into());
        }
        for (k, v) in object {
            body[k] = v.clone();
        }
        if handoff {
            "/continue/handoff"
        } else {
            "/continue/control"
        }
    } else {
        "/continue/view"
    };
    let result = crate::visor::trust_request(path, Some(body))?;
    let fresh = crate::visor::trust_request("/continue/context", None)?;
    verify_scope(&fresh, workspace, &mut identity)?;
    Ok(result)
}

fn verify_scope(
    context: &Value,
    workspace: &str,
    identity: &mut Option<Value>,
) -> Result<(), String> {
    if !std::path::Path::new(workspace).is_absolute()
        || context["workspace"] != workspace
        || !context["identity"].is_object()
    {
        return Err("Select the currently assigned local workspace".into());
    }
    let current = context["identity"].clone();
    if identity.as_ref().is_some_and(|old| *old != current) {
        return Err("Assignment changed. Reopen tasks to refresh.".into());
    }
    *identity = Some(current);
    Ok(())
}
fn entry(
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    label: &str,
    default: &str,
) -> Result<Option<String>, String> {
    let mut a = args("entry", "Continue task", label);
    a.push(format!("--entry-text={default}"));
    ui(&a, None)
}
fn start_form(
    scope: &Value,
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    request: &mut impl FnMut(&str, Option<Value>) -> Result<Value, String>,
) -> Result<Option<Value>, String> {
    let Some(goal) = entry(ui, "Goal", "")? else {
        return Ok(None);
    };
    let Some(links) = entry(
        ui,
        "Central issue, PR or check URLs (comma-separated GitHub links)",
        "",
    )?
    else {
        return Ok(None);
    };
    let tracking: Vec<String> = links
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let Some(completion) = entry(ui, "Completion criteria", "")? else {
        return Ok(None);
    };
    let Some(count) = entry(ui, "Number of declared chunks (1–16)", "2")? else {
        return Ok(None);
    };
    let count: usize = count
        .parse()
        .map_err(|_| "Enter a chunk count from 1 to 16")?;
    if !(1..=16).contains(&count) {
        return Err("Enter a chunk count from 1 to 16".into());
    }
    let Some(settings) = project_settings(ui, None)? else {
        return Ok(None);
    };
    let mut units = Vec::new();
    for i in 0..count {
        let Some(prompt) = entry(ui, &format!("Chunk {}: bounded coding prompt", i + 1), "")?
        else {
            return Ok(None);
        };
        let Some(criterion) = entry(
            ui,
            &format!("Chunk {}: completion description", i + 1),
            "One complete text response",
        )?
        else {
            return Ok(None);
        };
        let Some(issue) = entry(
            ui,
            "Existing central issue URL for this acceptance unit",
            tracking.first().map(String::as_str).unwrap_or(""),
        )?
        else {
            return Ok(None);
        };
        let Some(outcome) = entry(ui, "One outcome (up to 256 characters)", &criterion)? else {
            return Ok(None);
        };
        let Some(bound) = entry(
            ui,
            "Bounded component/file scope (up to 512 characters)",
            "",
        )?
        else {
            return Ok(None);
        };
        let Some(deps) = entry(
            ui,
            "Dependency unit numbers, comma separated; empty means independent",
            if i == 0 { "" } else { "1" },
        )?
        else {
            return Ok(None);
        };
        let deps: Vec<String> = deps
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.parse::<usize>().map(|n| format!("unit-{n}")))
            .collect::<Result<_, _>>()
            .map_err(|_| "Invalid dependency number")?;
        units.push(json!({"id":format!("unit-{}",i+1),"issue":issue,"outcome":outcome,"scope":bound,"prompt":prompt,"acceptance":criterion,"depends_on":deps}));
    }
    let Some(seconds) = entry(ui, "Time limit in seconds (1–3600)", "300")? else {
        return Ok(None);
    };
    let Some(tokens) = entry(
        ui,
        "Token budget, including reserved output (1–1048576)",
        "16384",
    )?
    else {
        return Ok(None);
    };
    let seconds: u64 = seconds.parse().map_err(|_| "Enter a numeric time limit")?;
    let tokens: u64 = tokens.parse().map_err(|_| "Enter a numeric token limit")?;
    let Some(enabled)=choose(ui,"Continue","Continue is on by default for this task only. Every chunk still uses current policy. The first task contract checks complete, nonempty model responses; it does not execute generated commands.",&["On (default)".into(),"Off — start paused".into()])? else{return Ok(None)};
    let mut body = scope.clone();
    body["run_id"] = json!(uuid::Uuid::new_v4());
    body["goal"] = json!(goal);
    body["tracking"] = json!(tracking);
    body["completion"] = json!(completion);
    let mut preview_body = scope.clone();
    preview_body["plan"] = json!({"settings":settings,"units":units});
    let preview = request("/continue/plan", Some(preview_body))?;
    if !preview["needs_input"]
        .as_array()
        .ok_or("Invalid plan preview")?
        .is_empty()
    {
        return Err(format!(
            "Revise acceptance units: {}",
            preview["needs_input"]
        ));
    }
    let shown=format!("{}\nModel/context: {}\nSettings: {}\nIssue mapping: {}\nAcceptance units: {}\nExecution remains serial within the WIP ceiling. No issues are created.",preview["guide"].as_str().unwrap_or(""),preview["budget"],preview["settings"],preview["issues"],preview["chunks"].as_array().ok_or("Missing plan chunks")?.iter().map(|u|json!({"id":u["id"],"outcome":u["outcome"],"acceptance":u["acceptance"],"scope":u["scope"],"depends_on":u["depends_on"],"estimated_input":u["estimated_input"]})).collect::<Vec<_>>().iter().map(Value::to_string).collect::<Vec<_>>().join("\n"));
    if choose(
        ui,
        "Preview project plan",
        &shown,
        &["Dispatch declared plan".into(), "Cancel".into()],
    )? != Some(0)
    {
        return Ok(None);
    }
    let (chunks, project) = planned_chunks(&preview)?;
    body["chunks"] = json!(chunks);
    body["project"] = project;
    body["tracking"] = json!(merge_tracking(&tracking, &preview["issues"])?);
    body["seconds"] = json!(seconds);
    body["tokens"] = json!(tokens);
    body["continue_enabled"] = json!(enabled == 0);
    Ok(Some(body))
}

fn observation_form(
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
) -> Result<Option<Value>, String> {
    let mut value = json!({"observed_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|"Clock unavailable")?.as_secs()});
    for (key, label) in [
        ("corrections", "Correction count"),
        ("stale_assumptions", "Stale assumption count"),
        ("tool_failures", "Recent tool failure count"),
        ("scope_drift", "Scope drift count"),
        ("context_used", "Estimated context tokens used"),
        ("context_capacity", "Context token capacity"),
        ("cost_micros", "Reported cost in micro-units"),
    ] {
        let Some(input) = entry(ui, &format!("{label}; blank means unknown"), "")? else {
            return Ok(None);
        };
        value[key] = if input.is_empty() {
            Value::Null
        } else {
            json!(input
                .parse::<u64>()
                .map_err(|_| "Enter a nonnegative number or leave unknown")?)
        };
    }
    for (key, label) in [
        (
            "failed_approaches",
            "Failed approach IDs in attempt order; comma separated",
        ),
        ("files", "Changed file paths; comma separated"),
        ("blockers", "Current blockers; comma separated"),
    ] {
        let Some(input) = entry(ui, label, "")? else {
            return Ok(None);
        };
        value[key] = json!(input
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>());
    }
    let Some(next) = entry(ui, "Proposed next step", "")? else {
        return Ok(None);
    };
    value["next_step"] = json!(next);
    let Some(owner) = choose(
        ui,
        "Task ownership",
        "Does this worker still own the intended task?",
        &["Unknown".into(), "Yes".into(), "No".into()],
    )?
    else {
        return Ok(None);
    };
    value["owns_task"] = match owner {
        1 => json!(true),
        2 => json!(false),
        _ => Value::Null,
    };
    Ok(Some(value))
}
fn merge_tracking(original: &[String], issues: &Value) -> Result<Vec<String>, String> {
    let mut links = original.to_vec();
    for issue in issues.as_array().ok_or("Invalid issue links")? {
        let value = issue.as_str().ok_or("Invalid issue link")?.to_owned();
        if !links.contains(&value) {
            links.push(value)
        }
    }
    if links.len() > 12 {
        return Err("Use at most twelve unique tracking links".into());
    }
    Ok(links)
}
fn project_settings(
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    old: Option<&Value>,
) -> Result<Option<Value>, String> {
    let styles = ["kanban", "vertical-slices", "milestones", "spike"];
    let Some(index)=choose(ui,"Project style","Use the existing tracker. Defaults for a running plan affect future tasks only; completed chunks, active prompts, limits and scope remain unchanged.",&styles.iter().map(|s|s.to_string()).collect::<Vec<_>>())? else{return Ok(None)};
    let style = styles.get(index).ok_or("Invalid preset")?;
    let Some(wip) = entry(
        ui,
        "WIP ceiling (1–16); this worker executes one chunk at a time",
        old.and_then(|v| v["wip"].as_u64())
            .map(|n| n.to_string())
            .as_deref()
            .unwrap_or("1"),
    )?
    else {
        return Ok(None);
    };
    let default = match *style {
        "kanban" => "Selected issue acceptance checked",
        "vertical-slices" => "One user-visible outcome checked",
        "milestones" => "Declared milestone acceptance checked",
        _ => "Answer the declared question and record the output",
    };
    let Some(completion) = entry(ui, "Completion default", default)? else {
        return Ok(None);
    };
    let Some(stop) = entry(
        ui,
        "Stop default",
        if *style == "spike" {
            "Stop at the declared output or time limit; do not start implementation"
        } else {
            "Stop on failed check, unknown result, scope change or budget"
        },
    )?
    else {
        return Ok(None);
    };
    Ok(Some(
        json!({"style":style,"wip":wip.parse::<u64>().map_err(|_|"Invalid WIP")?,"completion":completion,"stop":stop}),
    ))
}
fn planned_chunks(preview: &Value) -> Result<(Vec<Value>, Value), String> {
    let units = preview["chunks"].as_array().ok_or("Missing plan chunks")?;
    let mut chunks = Vec::new();
    let mut mappings = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let mut deps = Vec::new();
        for dep in u["depends_on"].as_array().ok_or("Missing dependencies")? {
            let index = units[..i]
                .iter()
                .position(|v| v["id"] == *dep)
                .ok_or("Dependency is not earlier in the plan")?;
            deps.push(index);
        }
        chunks.push(json!({"prompt":u["prompt"],"completion":u["acceptance"],"depends_on":deps}));
        mappings.push(json!({"chunk_id":"assigned-at-start","issue":u["issue"],"outcome":u["outcome"],"scope":u["scope"],"assignment_id":u["assignment_id"]}));
    }
    Ok((
        chunks,
        json!({"settings":preview["settings"],"budget":preview["budget"],"units":mappings}),
    ))
}

fn summary(run: &Value) -> String {
    if run.is_null() {
        return "No selected task. Start a scoped task to enable Continue.".into();
    }
    let next = run["chunks"]
        .as_array()
        .and_then(|v| v.iter().find(|c| c["state"] == "pending"))
        .map(|c| c["id"].as_str().unwrap_or("—"))
        .unwrap_or("—");
    format!("Task: {}\nState: {} ({})\nContinue: {}\nCurrent: {}\nNext declared chunk: {}\nChunks: {} / {}\nReserved tokens: {} / {}\nDeadline: {}\nPause/stop requests cancellation; an in-flight operation may remain uncertain. Scope and limits are fixed for this task. Stop it before starting a replacement.",run["id"].as_str().unwrap_or("—"),run["state"].as_str().unwrap_or("unknown"),run["reason"].as_str().unwrap_or("unknown"),if run["continue_enabled"]==true{"on"}else{"off"},run["current"].as_str().unwrap_or("—"),next,run["used_chunks"],run["budgets"]["chunks"],run["used_tokens"],run["budgets"]["tokens"],run["deadline"])
}
fn screen(
    workspace: &str,
    mut request: impl FnMut(&str, Option<Value>) -> Result<Value, String>,
    mut ui: impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
) -> Result<(), String> {
    let context = request("/continue/context", None)?;
    let mut identity = None;
    verify_scope(&context, workspace, &mut identity)?;
    let scope = json!({"workspace":workspace,"session_id":context["session_id"]});
    loop {
        let fresh = request("/continue/context", None)?;
        verify_scope(&fresh, workspace, &mut identity)?;
        let snapshot = request("/continue/view", Some(scope.clone()))?;
        let run = &snapshot["run"];
        let Some(action) = choose(
            &mut ui,
            "Continue task",
            &format!(
                "{}\nWorker: {} (generation {})\nLast handoff: {}\nLocal execution: {}",
                summary(run),
                run["worker_id"],
                run["worker_generation"],
                run["last_handoff"].get("action").unwrap_or(&Value::Null),
                if fresh["local_running"] == true {
                    "running"
                } else {
                    "paused; use Resume local execution when ready"
                }
            ),
            &[
                "Refresh status".into(),
                "Start a task".into(),
                "Toggle Continue".into(),
                "Pause".into(),
                "Resume".into(),
                "Stop task".into(),
                "Resume local execution for this session".into(),
                "Project style and defaults".into(),
                "Agent recommendation and handoff".into(),
                "Update work observations".into(),
                "Restart task worker".into(),
                "Reassign unfinished task".into(),
                "Retire task worker".into(),
            ],
        )?
        else {
            return Ok(());
        };
        if (10..=12).contains(&action) {
            if run.is_null() {
                continue;
            }
            let command = ["restart", "reassign", "retire"][action - 10];
            let text=format!("{command} the task worker within the current machine/session/workspace? A checkpoint preserves completed work, limits and pause/Continue. Queued or uncertain work must be resolved first. Retirement keeps unfinished work for explicit reassignment. No daemon or host is restarted.");
            if choose(
                &mut ui,
                "Confirm worker handoff",
                &text,
                &["Apply selected action".into(), "Cancel".into()],
            )? != Some(0)
            {
                continue;
            }
            let mut body = scope.clone();
            body["run_id"] = run["id"].clone();
            body["revision"] = run["revision"].clone();
            body["request_id"] = json!(uuid::Uuid::new_v4());
            body["action"] = json!(command);
            let result = request("/continue/handoff", Some(body))?;
            choose(
                &mut ui,
                "Worker handoff applied",
                &format!(
                    "Old worker: {}\nNew worker: {}\nGeneration: {}\nUnfinished task retained: {}",
                    result["handoff"]["old_worker"],
                    result["handoff"]["new_worker"],
                    result["handoff"]["generation"],
                    result["handoff"]["unfinished"]
                ),
                &["Back".into()],
            )?;
            continue;
        }
        if action == 8 {
            let result = request("/continue/advice", Some(scope.clone()))?;
            let text=format!("Recommendation: {}\nReasons: {}\nContributing observations: {}\nConfidence: {}\nUnknown: {}\nNext action: {}\nProposed handoff: {}\nThresholds: {}\nViewing advice never restarts or retires a worker.",result["action"],result["reasons"],result["contributing"],result["confidence"],result["unknown"],result["next_action"],result["handoff"],result["thresholds"]);
            choose(&mut ui, "Agent advice", &text, &["Back".into()])?;
            continue;
        }
        if action == 9 {
            if run.is_null() {
                continue;
            }
            if let Some(observations) = observation_form(&mut ui)? {
                let mut body = scope.clone();
                body["run_id"] = run["id"].clone();
                body["revision"] = run["revision"].clone();
                body["observations"] = observations;
                request("/continue/observations", Some(body))?;
            }
            continue;
        }
        if action == 7 {
            if run["project"].is_null() {
                return Err("Start a task with a project preset first".into());
            }
            if let Some(settings) = project_settings(&mut ui, Some(&run["project_settings"]))? {
                let mut body = scope.clone();
                body["run_id"] = run["id"].clone();
                body["revision"] = run["revision"].clone();
                body["settings"] = settings;
                request("/continue/project", Some(body))?;
            }
            continue;
        }
        if action == 6 {
            let latest = request("/continue/context", None)?;
            verify_scope(&latest, workspace, &mut identity)?;
            let mut body = scope.clone();
            body["expected_epoch"] = latest["local_epoch"].clone();
            request("/continue/local-resume", Some(body))?;
            return Ok(());
        }
        if action == 0 {
            continue;
        }
        if action == 1 {
            if !run.is_null() && run["state"] != "completed" && run["stopped"] != true {
                return Err("Stop the selected task before replacing its scope or limits".into());
            }
            if let Some(body) = start_form(&scope, &mut ui, &mut request)? {
                request("/continue/start", Some(body))?;
            }
        } else {
            if run.is_null() {
                continue;
            }
            let command = match action {
                2 => {
                    if run["continue_enabled"] == true {
                        "continue-off"
                    } else {
                        "continue-on"
                    }
                }
                3 => "pause",
                4 => "resume",
                5 => "stop",
                _ => return Err("Unknown task control".into()),
            };
            let mut body = scope.clone();
            body["run_id"] = run["id"].clone();
            body["revision"] = run["revision"].clone();
            body["action"] = json!(command);
            // Show no optimistic state: the next view comes from the acknowledged owner.
            request("/continue/control", Some(body))?;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_or_assignment_change_refuses() {
        let mut i = None;
        let v = json!({"workspace":"/project","identity":{"session_id":"first"}});
        verify_scope(&v, "/project", &mut i).unwrap();
        assert!(verify_scope(&v, "/other", &mut i).is_err());
        let w = json!({"workspace":"/project","identity":{"session_id":"second"}});
        assert!(verify_scope(&w, "/project", &mut i).is_err());
    }
    #[test]
    fn controls_use_current_revision_and_owner_acknowledgement() {
        let mut calls = Vec::new();
        let mut ui_count = 0;
        screen("/project",|path,body|{calls.push((path.to_owned(),body.clone()));match path{"/continue/context"=>Ok(json!({"workspace":"/project","session_id":"s","identity":{"session_id":"s"}})),"/continue/view"=>Ok(json!({"run":{"id":"r","revision":7,"state":"running","continue_enabled":true}})),"/continue/control"=>Err("stale task revision".into()),_=>panic!("unexpected path")}},|_,_|{ui_count+=1;Ok(Some("3".into()))}).unwrap_err();
        assert_eq!(ui_count, 1);
        let body = calls.last().unwrap().1.as_ref().unwrap();
        assert_eq!(body["revision"], 7);
        assert_eq!(body["action"], "pause");
    }
    #[test]
    fn reconnect_reads_server_state_without_starting_a_task() {
        let mut starts = 0;
        screen("/project",|path,_|{if path=="/continue/context"{Ok(json!({"workspace":"/project","session_id":"s","identity":{"session_id":"s"}}))}else{if path=="/continue/start"{starts+=1}Ok(json!({"run":{"state":"paused","continue_enabled":false}}))}},|_,_|Ok(None)).unwrap();
        assert_eq!(starts, 0);
    }
    #[test]
    fn planned_units_keep_issue_scope_and_dependency_boundaries() {
        let preview = json!({"settings":{"style":"spike"},"budget":{"source":"unknown-conservative"},"chunks":[{"id":"a","prompt":"one","acceptance":"first checked","issue":"https://github.com/a/b/issues/1","scope":"one file","outcome":"answer","assignment_id":"assigned","depends_on":[]},{"id":"b","prompt":"two","acceptance":"second checked","issue":"https://github.com/a/b/issues/1","scope":"one file","outcome":"answer","assignment_id":"assigned","depends_on":["a"]}]});
        let (chunks, project) = planned_chunks(&preview).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1]["depends_on"], json!([0]));
        assert_eq!(project["units"][0]["assignment_id"], "assigned");
        let mut invalid = preview;
        invalid["chunks"][0]["depends_on"] = json!(["b"]);
        assert!(planned_chunks(&invalid).is_err());
    }
    #[test]
    fn project_tracking_preserves_prs_checks_and_extra_issues() {
        let links = vec![
            "https://github.com/a/b/pull/2".into(),
            "https://github.com/a/b/actions/runs/3".into(),
            "https://github.com/a/b/issues/4".into(),
        ];
        let merged = merge_tracking(
            &links,
            &json!([
                "https://github.com/a/b/issues/1",
                "https://github.com/a/b/issues/4"
            ]),
        )
        .unwrap();
        assert_eq!(&merged[..3], &links);
        assert_eq!(merged.len(), 4);
    }
    #[test]
    fn advice_display_never_sends_worker_controls() {
        let mut paths = Vec::new();
        let mut choices = vec![Some("8".to_owned()), Some("0".to_owned()), None].into_iter();
        screen("/project",|path,_|{paths.push(path.to_owned());match path{"/continue/context"=>Ok(json!({"workspace":"/project","session_id":"s","identity":{"session_id":"s"}})),"/continue/advice"=>Ok(json!({"action":"restart","handoff":{"goal":"goal"}})),_=>Ok(json!({"run":{"id":"r","state":"paused"}}))}},|_,_|Ok(choices.next().unwrap())).unwrap();
        assert!(paths.iter().any(|p| p == "/continue/advice"));
        assert!(!paths
            .iter()
            .any(|p| p == "/continue/control" || p == "/continue/local-resume"));
    }
    #[test]
    fn worker_change_requires_confirmation_and_current_revision() {
        for confirmed in [false, true] {
            let mut calls = Vec::new();
            let mut choices = if confirmed {
                vec![
                    Some("10".to_owned()),
                    Some("0".to_owned()),
                    Some("0".to_owned()),
                    None,
                ]
            } else {
                vec![Some("10".to_owned()), Some("1".to_owned()), None]
            }
            .into_iter();
            screen("/project",|path,body|{if path=="/continue/handoff"{calls.push(body.unwrap())}match path{"/continue/context"=>Ok(json!({"workspace":"/project","session_id":"s","identity":{"session_id":"s"}})),_=>Ok(json!({"run":{"id":"r","revision":7,"state":"paused"},"handoff":{"generation":2}}))}},|_,_|Ok(choices.next().unwrap())).unwrap();
            assert_eq!(calls.len(), usize::from(confirmed));
            if confirmed {
                assert_eq!(calls[0]["revision"], 7);
                assert_eq!(calls[0]["action"], "restart");
                assert!(uuid::Uuid::parse_str(calls[0]["request_id"].as_str().unwrap()).is_ok())
            }
        }
    }
}
