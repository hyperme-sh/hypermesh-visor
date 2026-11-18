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
pub fn exchange(workspace: &str, control: Option<Value>) -> Result<Value, String> {
    let active = crate::visor::trust_request("/continue/context", None)?;
    let mut identity = None;
    verify_scope(&active, workspace, &mut identity)?;
    let mut body = json!({"workspace":workspace,"session_id":active["session_id"]});
    let path = if let Some(control) = control {
        let object = control.as_object().ok_or("Invalid task control")?;
        if object.len() != 3
            || !["run_id", "revision", "action"]
                .iter()
                .all(|k| object.contains_key(*k))
        {
            return Err("Invalid task control".into());
        }
        for (k, v) in object {
            body[k] = v.clone();
        }
        "/continue/control"
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
    let mut chunks = Vec::new();
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
        let deps = if i == 0 { vec![] } else { vec![i - 1] };
        chunks.push(json!({"prompt":prompt,"completion":criterion,"depends_on":deps}));
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
    body["chunks"] = json!(chunks);
    body["seconds"] = json!(seconds);
    body["tokens"] = json!(tokens);
    body["continue_enabled"] = json!(enabled == 0);
    Ok(Some(body))
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
            &summary(run),
            &[
                "Refresh status".into(),
                "Start a task".into(),
                "Toggle Continue".into(),
                "Pause".into(),
                "Resume".into(),
                "Stop task".into(),
            ],
        )?
        else {
            return Ok(());
        };
        if action == 0 {
            continue;
        }
        if action == 1 {
            if !run.is_null() && run["state"] != "completed" && run["stopped"] != true {
                return Err("Stop the selected task before replacing its scope or limits".into());
            }
            if let Some(body) = start_form(&scope, &mut ui)? {
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
}
