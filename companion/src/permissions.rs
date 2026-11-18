//! Native permission templates, current reviews and bounded local reads.
use crate::mcp_dialog::{args, choose, dialog};
use serde_json::{json, Value};

pub fn run() -> Result<(), String> { run_workspace(None) }
pub fn run_workspace(workspace:Option<&str>) -> Result<(), String> {
    hypermesh_session::refresh_session(
        &hypermesh_session::KeyringStore,
        &hypermesh_session::Endpoints::panopticon(),
    )
    .map_err(|_| "Sign in again to open Permissions")?;
    screen_for(
        |path, body| {
            if path == "/permissions/policy" {
                crate::access_policy::request(body.ok_or("Policy scope required")?)
            } else {
                crate::visor::trust_request(path, body)
            }
        },
        dialog,
        workspace,
    )
}
fn entry(
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    label: &str,
    initial: &str,
) -> Result<Option<String>, String> {
    let mut a = args("entry", "Permissions", label);
    a.push(format!("--entry-text={initial}"));
    ui(&a, None)
}
fn screen(request:impl FnMut(&str,Option<Value>)->Result<Value,String>,ui:impl FnMut(&[String],Option<&str>)->Result<Option<String>,String>)->Result<(),String>{screen_for(request,ui,None)}
fn screen_for(
    mut request: impl FnMut(&str, Option<Value>) -> Result<Value, String>,
    mut ui: impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    workspace:Option<&str>,
) -> Result<(), String> {
    let context = request("/permissions/context", None)?;
    let organization = context["binding"]["organization_id"]
        .as_str()
        .ok_or("Assigned organization unavailable")?;
    let device = context["device_id"]
        .as_str()
        .ok_or("Assigned device unavailable")?;
    let Some(project) = (if let Some(path)=workspace {
        if !std::path::Path::new(path).is_absolute() || path.chars().any(char::is_control) || path.len()>4096 {return Err("Select a canonical local workspace".into())}
        Some(path.to_owned())
    }else{entry(&mut ui, "Absolute project directory", "")?}) else {
        return Ok(());
    };
    let scope = json!({"organization":organization,"device":device,"project":project});
    loop {
        let fresh=request("/permissions/context",None)?;
        if fresh["binding"]!=context["binding"] || fresh["device_id"]!=context["device_id"] {return Err("Assigned scope changed. Reopen Permissions to refresh.".into())}
        if workspace.is_some(){
            let active=request("/acp/context",None)?;
            if active["workspace"]!=project || active["identity"]["organization_id"]!=organization || active["identity"]["device_id"]!=device {return Err("Workspace differs from the active assignment. Reconnect and reopen Permissions.".into())}
        }
        let snapshot = request("/permissions/templates", None)?;
        let selections = snapshot["selections"]
            .as_array()
            .ok_or("Permission templates unavailable")?;
        let current = selections.iter().find(|s| s["scope"] == scope).cloned();
        let description=format!("Project: {project}\nOrganization: {organization}\nDevice: {device}\nTemplate: {}\nTemplates are preferences. Current user and organization policy still applies. Previews do not execute actions or remember grants.",current.as_ref().and_then(|s|s["preset"].as_str()).unwrap_or("not selected"));
        let Some(action) = choose(
            &mut ui,
            "Permissions",
            &description,
            &[
                "Select or edit template".into(),
                "Preview a concrete action".into(),
                "Read a verified local file".into(),
                "View learned allows".into(),
                "Run a verified typed action".into(),
                "Review or change access policy".into(),
            ],
        )?
        else {
            return Ok(());
        };
        if action == 5 {
            policy_screen(&mut request, &mut ui, &context, organization)?;
        } else if action == 4 {
            let Some(selection) = current else { continue };
            let machine = request("/acp/context", None)?;
            if machine["workspace"] != project {
                return Err("Selected project differs from the active assignment".into());
            }
            let rules: Vec<Value> = selection["rules"]
                .as_array()
                .ok_or("Template unavailable")?
                .iter()
                .filter(|r| {
                    matches!(
                        r["operation"].as_str(),
                        Some("https" | "write" | "service_restart")
                    )
                })
                .cloned()
                .collect();
            if rules.is_empty() {
                return Err(
                    "Add an HTTPS, write or named service rule to the template first".into(),
                );
            }
            let labels: Vec<String> = rules
                .iter()
                .map(|r| {
                    format!(
                        "{}: {}",
                        r["operation"].as_str().unwrap_or(""),
                        r["target"].as_str().unwrap_or("")
                    )
                })
                .collect();
            let Some(index)=choose(&mut ui,"Verified action","Choose an exact configured scope. Writes create a new file directly inside the project; existing files are never replaced.",&labels)? else{continue};
            let rule = &rules[index];
            let operation = rule["operation"].as_str().ok_or("Operation unavailable")?;
            let Some(target) = entry(
                &mut ui,
                "Exact action target",
                rule["target"].as_str().unwrap_or(""),
            )?
            else {
                continue;
            };
            let mut body = json!({"request_id":uuid::Uuid::new_v4().to_string(),"session_id":machine["session_id"],"selection":selection,"operation":operation,"target":target,"max_bytes":rule["max_bytes"].as_u64().unwrap_or(0).min(1048576),"timeout_seconds":rule["timeout_seconds"].as_u64().unwrap_or(0).min(30)});
            if operation == "https" {
                let Some(method) = choose(
                    &mut ui,
                    "HTTPS method",
                    "Requests use verified HTTPS, with no redirects or forwarded credentials.",
                    &["GET".into(), "POST".into()],
                )?
                else {
                    continue;
                };
                body["method"] = json!(if method == 0 { "GET" } else { "POST" });
            }
            if operation == "write" || body["method"] == "POST" {
                let Some(content) = entry(&mut ui, "UTF-8 content to send or create", "")? else {
                    continue;
                };
                use base64::Engine;
                body["content_base64"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(content.as_bytes()));
            }
            let result = request("/permissions/actions", Some(body))?;
            if result["completed"] != true {
                return Err("Action did not confirm completion; outcome may be unknown. Do not automatically retry.".into());
            }
            crate::mcp_dialog::text(
                &mut ui,
                "Verified action completed",
                &format!(
                    "Target: {target}\nLearned: {} · Reused: {} · Pending review: {}",
                    result["learned"], result["reused"], result["review_pending"]
                ),
                false,
            )?;
        } else if action == 3 {
            let store = request("/permissions/allows", None)?;
            let mut history = request("/permissions/dream", None)?;
            history["history"]=json!(history_for_project(&history,&project));
            let rows:Vec<String>=store["allows"].as_array().ok_or("Learned allows unavailable")?.iter()
                .filter(|a|a["project"]==project&&a["identity"]["organization_id"]==organization&&a["identity"]["device_id"]==device)
                .map(|a|format!("{}\nScope: {}\nOrigin: {}\nSession: {}\nPolicy: {}\nLast use: {} (Unix seconds)\nExpires: {} (Unix seconds)\nPending dreaming review: {}",a["action"]["target"].as_str().unwrap_or(""),a["action"],a["origin"].as_str().unwrap_or(""),a["identity"]["session_id"].as_str().unwrap_or(""),a["policy_revision"].as_str().unwrap_or(""),a["last_use"],a["expires_at"],a["review_pending"])).collect();
            crate::mcp_dialog::text(
                &mut ui,
                "Learned allows — current policy is checked on every use",
                &if rows.is_empty() {
                    format!("No learned allows in this scope\nDreaming reviews: {}",history["history"])
                } else {
                    format!("{}\n\nDreaming reviews:\n{}",rows.join("\n\n"),history["history"].as_array().map(|items|items.iter().map(|h|format!("{}: {} — {}\nBefore: {}\nAfter: {}\nSource: {}",h["kind"],h["status"],h["reason"],h["before"],h["after"],h["source_review"])).collect::<Vec<_>>().join("\n\n")).unwrap_or_default())
                },
                false,
            )?;
        } else if action == 2 {
            let Some(selection) = current else { continue };
            let machine = request("/acp/context", None)?;
            if machine["workspace"] != project {
                return Err("Selected project differs from the active assignment".into());
            }
            let Some(target) = entry(&mut ui, "Exact local file to read", "")? else {
                continue;
            };
            let rule = selection["rules"]
                .as_array()
                .and_then(|rs| {
                    rs.iter()
                        .find(|r| r["operation"] == "read" && r["target"] == target)
                })
                .ok_or("Select an exact read target in the template first")?;
            let result = request(
                "/permissions/reads",
                Some(
                    json!({"request_id":uuid::Uuid::new_v4().to_string(),"session_id":machine["session_id"],"selection":selection,"target":target,"max_bytes":rule["max_bytes"].as_u64().unwrap_or(0).min(1048576),"timeout_seconds":rule["timeout_seconds"].as_u64().unwrap_or(0).min(30)}),
                ),
            )?;
            if !result["error"].is_null() {
                return Err("Read refused or verification unavailable; not retried".into());
            }
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(
                    result["content_base64"]
                        .as_str()
                        .ok_or("Read unavailable")?,
                )
                .map_err(|_| "Read unavailable")?;
            let message = format!(
                "Learned: {} · Reused: {} · Pending review: {}\n\n{}",
                result["learned"],
                result["reused"],
                result["review_pending"],
                String::from_utf8(bytes).unwrap_or_else(|_| {
                    "Binary file read successfully; contents are not displayed".into()
                })
            );
            crate::mcp_dialog::text(&mut ui, "Verified file read", &message, false)?;
        } else if action == 0 {
            let Some(preset) = choose(
                &mut ui,
                "Permission template",
                "Choose a scoped starting point. Resources and limits remain editable.",
                &[
                    "Local inspection".into(),
                    "Project development".into(),
                    "Named network destinations".into(),
                    "Scoped administration".into(),
                ],
            )?
            else {
                continue;
            };
            let (name, operations, label, initial) = match preset {
                0 => (
                    "local_inspection",
                    vec!["read"],
                    "Exact local file or directory",
                    project.as_str(),
                ),
                1 => (
                    "project_development",
                    vec!["read", "write"],
                    "Exact project file or directory",
                    project.as_str(),
                ),
                2 => (
                    "named_network",
                    vec!["https"],
                    "Exact HTTPS destination (no credentials, query or fragment)",
                    "",
                ),
                _ => (
                    "scoped_administration",
                    vec!["service_inspect", "service_restart"],
                    "Exact service name (no shell command)",
                    "",
                ),
            };
            let existing = current.as_ref().filter(|s| s["preset"] == name);
            let initial = existing
                .and_then(|s| s["rules"].as_array())
                .map(|rules| {
                    let mut targets = Vec::new();
                    for rule in rules {
                        if let Some(target) = rule["target"].as_str() {
                            if !targets.contains(&target) {
                                targets.push(target);
                            }
                        }
                    }
                    targets.join("\n")
                })
                .unwrap_or_else(|| initial.to_owned());
            let Some(targets) = crate::mcp_dialog::text(
                &mut ui,
                &format!("{label} — one per line"),
                &initial,
                true,
            )?
            else {
                continue;
            };
            let targets: Vec<String> = targets
                .lines()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            if targets.is_empty() || targets.len() > 16 {
                return Err("Enter between one and sixteen explicit targets".into());
            }
            let default_bytes = existing
                .and_then(|s| s["rules"][0]["max_bytes"].as_u64())
                .unwrap_or(1048576)
                .to_string();
            let Some(bytes) = entry(&mut ui, "Maximum bytes (1–67108864)", &default_bytes)?
            else {
                continue;
            };
            let bytes = bytes
                .parse::<u64>()
                .map_err(|_| "Enter a whole-number byte limit")?;
            let default_seconds = existing
                .and_then(|s| s["rules"][0]["timeout_seconds"].as_u64())
                .unwrap_or(30)
                .to_string();
            let Some(seconds) = entry(&mut ui, "Timeout in seconds (1–300)", &default_seconds)?
            else {
                continue;
            };
            let seconds = seconds
                .parse::<u32>()
                .map_err(|_| "Enter a whole-number timeout")?;
            let rules:Vec<Value>=targets.iter().flat_map(|target| operations.iter().map(move |operation| json!({"operation":operation,"target":target,"max_bytes":bytes,"timeout_seconds":seconds}))).collect();
            let selection = json!({"version":1,"scope":scope,"preset":name,"rules":rules});
            request(
                "/permissions/templates",
                Some(json!({"expected":snapshot,"selection":selection})),
            )?;
            ui(&args("info","Template saved","Saved for this project, organization and device. No execution permission or learned grant changed."),None)?;
        } else {
            let Some(selection) = current else {
                ui(
                    &args(
                        "info",
                        "Select a template",
                        "Select a permission template before previewing an action.",
                    ),
                    None,
                )?;
                continue;
            };
            let operations = [
                "read",
                "write",
                "https",
                "service_inspect",
                "service_restart",
            ];
            let Some(index)=choose(&mut ui,"Preview action","Choose an operation. You can inspect both permitted and refused cases. This preview has no additional action parameters.",&["Read a local resource".into(),"Write a local resource".into(),"Access an HTTPS destination".into(),"Inspect a service".into(),"Restart a service".into()])? else{continue};
            let initial = selection["rules"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["operation"] == operations[index]))
                .and_then(|r| r["target"].as_str())
                .unwrap_or("");
            let Some(target) = entry(&mut ui, "Exact target to preview", initial)? else {
                continue;
            };
            let limits = &selection["rules"][0];
            let request_body = json!({"selection":selection,"operation":operations[index],"target":target,"max_bytes":limits["max_bytes"],"timeout_seconds":limits["timeout_seconds"]});
            match request("/permissions/preview", Some(request_body)) {
                Ok(result) => {
                    ui(&args("info", "Permission preview", &render(&result)?), None)?;
                }
                Err(_) => {
                    ui(&args("info","Permission preview","Verification unavailable. No action was run and no allow was learned. Check the assigned host and policy service, then try again."),None)?;
                }
            }
        }
    }
}
fn render(result: &Value) -> Result<String, String> {
    if result["preview_only"] != true {
        return Err("Invalid permission preview".into());
    }
    let label = match result["decision"].as_str() {
        Some("allowed") => "Allowed (preview only)",
        Some("advisory") => "Permitted with advisory (preview only)",
        Some("prohibited") => "Prohibited by current policy or template",
        Some("extreme") => "Blocked by an extreme policy concern",
        Some("unavailable") => "Verification unavailable",
        _ => return Err("Invalid permission decision".into()),
    };
    Ok(format!("{label}\n{}\nTarget: {}\nEffective limits: {} bytes, {} seconds\nEligible for learning: {}\nPolicy revision: {}\nNo action was run and no allow was learned.",
        result["reason"].as_str().unwrap_or(""),result["action"]["target"].as_str().unwrap_or(""),result["limits"]["max_bytes"],result["limits"]["timeout_seconds"],result["learning_eligible"]==true,result["policy_revision"].as_str().unwrap_or("")))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_read_uses_saved_scope_and_displays_learning() {
        let mut answers = vec![
            Some("/project"),
            Some("2"),
            Some("/project/file"),
            None,
            None,
        ]
        .into_iter();
        let selection = json!({"version":1,"scope":{"organization":"org","device":"device","project":"/project"},"preset":"local_inspection","rules":[{"operation":"read","target":"/project/file","max_bytes":1024,"timeout_seconds":10}]});
        let mut reads = 0;
        screen(|path,body|Ok(match path {
            "/permissions/context"=>json!({"device_id":"device","binding":{"organization_id":"org"}}),
            "/permissions/templates"=>json!({"version":1,"selections":[selection.clone()]}),
            "/acp/context"=>json!({"workspace":"/project","session_id":"session"}),
            "/permissions/reads"=>{reads+=1;let body=body.unwrap();assert_eq!(body["selection"],selection);assert_eq!(body["target"],"/project/file");assert!(uuid::Uuid::parse_str(body["request_id"].as_str().unwrap()).is_ok());json!({"content_base64":"aGVsbG8=","learned":true,"reused":false,"review_pending":true})},
            _=>panic!("unexpected route")
        }),|_,text|{if let Some(text)=text {assert!(text.contains("hello"));assert!(text.contains("Learned: true"));}Ok(answers.next().unwrap().map(str::to_owned))}).unwrap();
        assert_eq!(reads, 1);
    }
    #[test]
    fn typed_write_uses_saved_scope_and_does_not_retry_unknown_outcome() {
        let selection = json!({"version":1,"scope":{"organization":"org","device":"device","project":"/project"},"preset":"project_development","rules":[{"operation":"write","target":"/project/new","max_bytes":1024,"timeout_seconds":10}]});
        let mut answers = vec![
            Some("/project"),
            Some("4"),
            Some("0"),
            Some("/project/new"),
            Some("hello"),
        ]
        .into_iter();
        let mut calls = 0;
        let result = screen(
            |path, body| {
                Ok(match path {
                    "/permissions/context" => {
                        json!({"device_id":"device","binding":{"organization_id":"org"}})
                    }
                    "/permissions/templates" => {
                        json!({"version":1,"selections":[selection.clone()]})
                    }
                    "/acp/context" => json!({"workspace":"/project","session_id":"session"}),
                    "/permissions/actions" => {
                        calls += 1;
                        let body = body.unwrap();
                        assert_eq!(body["selection"], selection);
                        assert_eq!(body["operation"], "write");
                        assert_eq!(body["content_base64"], "aGVsbG8=");
                        assert!(body.get("credential").is_none());
                        json!({"error":"unknown"})
                    }
                    _ => panic!("unexpected route"),
                })
            },
            |_, _| Ok(answers.next().unwrap().map(str::to_owned)),
        );
        assert!(result.unwrap_err().contains("outcome may be unknown"));
        assert_eq!(calls, 1);
    }
    #[test]
    fn advisory_is_not_rendered_as_a_block() {
        let text=render(&json!({"preview_only":true,"decision":"advisory","reason":"Review later","learning_eligible":false})).unwrap();
        assert!(text.contains("Permitted with advisory"));
        assert!(!text.contains("Blocked"));
        assert!(text.contains("learning: false"));
    }
    #[test]
    fn cancelling_an_edit_does_not_save() {
        let mut answers = vec![Some("/project"), Some("0"), Some("0"), None, None].into_iter();
        screen(
            |path, body| {
                assert!(body.is_none());
                Ok(if path == "/permissions/context" {
                    json!({"device_id":"device","binding":{"organization_id":"org"}})
                } else {
                    json!({"version":1,"selections":[]})
                })
            },
            |_, _| Ok(answers.next().unwrap().map(str::to_owned)),
        )
        .unwrap();
    }
    #[test]
    fn editing_saves_scope_and_limits_without_authority() {
        let mut answers = vec![
            Some("/project"),
            Some("0"),
            Some("2"),
            Some("https://example.test/"),
            Some("1024"),
            Some("10"),
            Some(""),
            None,
        ]
        .into_iter();
        let mut writes = vec![];
        screen(
            |path, body| {
                if let Some(body) = body {
                    writes.push(body)
                };
                Ok(if path == "/permissions/context" {
                    json!({"device_id":"device","binding":{"organization_id":"org"}})
                } else {
                    json!({"version":1,"selections":[]})
                })
            },
            |_, _| Ok(answers.next().unwrap().map(str::to_owned)),
        )
        .unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["selection"]["rules"][0]["max_bytes"], 1024);
        assert_eq!(writes[0]["selection"]["scope"]["project"], "/project");
        assert!(writes[0].get("allow").is_none());
    }
}

fn policy_screen(
    request: &mut impl FnMut(&str, Option<Value>) -> Result<Value, String>,
    ui: &mut impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
    context: &Value,
    organization: &str,
) -> Result<(), String> {
    let tenant = context["tenant_id"]
        .as_str()
        .ok_or("Assigned tenant unavailable")?;
    let scope = json!({"tenant_id":tenant,"organization_id":organization});
    let policy = request("/permissions/policy", Some(scope.clone()))?;
    let mut choices = vec!["View current policy and restrictions".into()];
    if policy["can_edit_personal"] == true {
        choices.push("Narrow my permissions".into());
    }
    if policy["can_edit_organization"] == true {
        choices.push("Change organization restrictions".into());
    }
    let Some(choice)=choose(ui,"Access policy","Personal settings cannot relax organization restrictions. Per-use means fresh automatic verification, not another prompt.",&choices)? else{return Ok(())};
    if choice == 0 {
        crate::mcp_dialog::text(ui, "Current access policy", &policy_text(&policy), false)?;
        return Ok(());
    }
    let organization_edit = choices[choice] == "Change organization restrictions";
    let rules = policy["rules"]
        .as_array()
        .ok_or("Policy rules unavailable")?;
    if rules.is_empty() {
        return Err("No configured rules in this organization".into());
    }
    let labels: Vec<String> = rules
        .iter()
        .map(|r| {
            format!(
                "{}: {}",
                r["operation"].as_str().unwrap_or(""),
                r["target"].as_str().unwrap_or("")
            )
        })
        .collect();
    let Some(index)=choose(ui,"Policy rule","Select the exact operation and resource. Restrictions cover all typed arguments for this target.",&labels)? else{return Ok(())};
    let rule = &rules[index];
    let Some(mode) = choose(
        ui,
        "Rule restriction",
        "Removing your restriction still leaves organization policy in force.",
        &[
            "Prohibit this action".into(),
            "Verify each use; do not remember".into(),
            "Reduce byte/time limits".into(),
            "Remove this scope's restriction".into(),
        ],
    )?
    else {
        return Ok(());
    };
    let Some(reason) = entry(ui, "Reason for this change", "")? else {
        return Ok(());
    };
    if reason.is_empty() || reason.len() > 256 {
        return Err("Enter a reason of 1–256 bytes".into());
    }
    let mode_name = ["deny", "per_use", "limited", "inherited"][mode];
    let mut restriction = json!({"operation":rule["operation"],"target":rule["target"],"mode":mode_name,"reason":reason});
    if mode == 2 {
        let Some(bytes) = entry(
            ui,
            "Maximum bytes",
            &rule["limits"]["max_bytes"].to_string(),
        )?
        else {
            return Ok(());
        };
        let Some(seconds) = entry(
            ui,
            "Timeout in seconds",
            &rule["limits"]["timeout_seconds"].to_string(),
        )?
        else {
            return Ok(());
        };
        restriction["limits"] = json!({"max_bytes":bytes.parse::<u64>().map_err(|_|"Whole-number limit required")?,"timeout_seconds":seconds.parse::<u32>().map_err(|_|"Whole-number timeout required")?});
    }
    let mut update = scope;
    update["mutation"] = json!({"version":1,"organization_id":organization,"expected_revision":policy["revision"],"scope":if organization_edit{"organization"}else{"personal"},"restriction":restriction});
    request("/permissions/policy", Some(update))?;
    let cleared = request(
        "/permissions/cache",
        Some(json!({"organization_id":organization})),
    )
    .is_ok();
    ui(
        &args(
            "info",
            "Policy updated",
            if cleared {
                "Remembered allows were cleared. New actions use the updated policy. Already admitted actions cannot be undone."
            } else {
                "The policy changed. Local cache cleanup was unavailable; every new action still checks current policy. Already admitted actions cannot be undone."
            },
        ),
        None,
    )?;
    Ok(())
}

fn policy_text(policy: &Value) -> String {
    let mut lines = vec![format!(
        "Signed-in user: {}",
        policy["subject"].as_str().unwrap_or("")
    )];
    if let Some(rules) = policy["rules"].as_array() {
        for r in rules {
            lines.push(format!(
                "{} — {}\n{} · {} bytes · {} seconds\n{}",
                r["operation"].as_str().unwrap_or(""),
                r["target"].as_str().unwrap_or(""),
                r["effect"].as_str().unwrap_or(""),
                r["limits"]["max_bytes"],
                r["limits"]["timeout_seconds"],
                r["reason"].as_str().unwrap_or("")
            ));
        }
    }
    if let Some(rules) = policy["restrictions"].as_array() {
        for r in rules {
            lines.push(format!(
                "{} restriction: {} — {}\n{} · {}",
                if r["subject"] == "" {
                    "Organization"
                } else {
                    "Personal"
                },
                r["operation"].as_str().unwrap_or(""),
                r["target"].as_str().unwrap_or(""),
                r["mode"].as_str().unwrap_or(""),
                r["reason"].as_str().unwrap_or("")
            ));
        }
    }
    lines.join("\n\n")
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn personal_edit_uses_expected_revision_and_clears_local_cache() {
        let mut answers = vec![
            Some("1"),
            Some("0"),
            Some("1"),
            Some("Verify each action"),
            None,
        ]
        .into_iter();
        let policy = json!({"version":1,"revision":"revision","can_edit_personal":true,"can_edit_organization":false,"rules":[{"operation":"read","target":"/project/file","limits":{"max_bytes":1024,"timeout_seconds":10}}]});
        let mut updates = 0;
        let mut clears = 0;
        policy_screen(
            &mut |path, body| {
                if path == "/permissions/cache" {
                    clears += 1;
                    return Ok(json!({"invalidated":true}));
                }
                let body = body.unwrap();
                assert_eq!(body["tenant_id"], "tenant");
                assert_eq!(body["organization_id"], "org");
                if !body["mutation"].is_null() {
                    updates += 1;
                    assert_eq!(body["mutation"]["scope"], "personal");
                    assert_eq!(body["mutation"]["expected_revision"], "revision");
                    assert_eq!(body["mutation"]["restriction"]["mode"], "per_use");
                }
                Ok(policy.clone())
            },
            &mut |args, _| {
                assert!(!args.iter().any(|s| s == "Change organization restrictions"));
                Ok(answers.next().unwrap().map(str::to_owned))
            },
            &json!({"tenant_id":"tenant"}),
            "org",
        )
        .unwrap();
        assert_eq!((updates, clears), (1, 1));
    }
}

fn history_for_project(history:&Value,project:&str)->Vec<Value>{ history["history"].as_array().map(|items|items.iter().filter(|h|h["project"]==project).cloned().collect()).unwrap_or_default() }

#[cfg(test)]
mod history_tests { use super::*; #[test] fn selected_project_never_displays_active_other_project(){ let history=json!({"history":[{"project":"/active","reason":"private"}]});assert!(history_for_project(&history,"/selected").is_empty());assert_eq!(history_for_project(&history,"/active").len(),1); } }
#[cfg(test)]
mod workspace_tests {
 use super::*;
 #[test]
 fn workspace_mismatch_never_loads_permissions(){
  let context=json!({"device_id":"device","binding":{"organization_id":"org"}});
  let result=screen_for(|path,_| match path{
   "/permissions/context"=>Ok(context.clone()),
   "/acp/context"=>Ok(json!({"workspace":"/other","identity":{"device_id":"device","organization_id":"org"}})),
   _=>panic!("loaded another workspace")},|_,_|panic!("displayed another workspace"),Some("/project"));
  assert!(result.unwrap_err().contains("Workspace differs"));
 }
 #[test]
 fn changed_org_refuses_stale_ui_before_loading_rules(){
  let mut calls=0;
  let result=screen_for(|path,_|{assert_eq!(path,"/permissions/context");calls+=1;Ok(json!({"device_id":"device","binding":{"organization_id":if calls==1{"old"}else{"new"}}}))},|_,_|panic!("displayed stale organization"),Some("/project"));
  assert!(result.unwrap_err().contains("scope changed"));
 }
}
