use crate::mcp_dialog::{args, choose, dialog};

pub fn run() -> Result<(), String> {
    let snapshot = crate::visor::session_selection_request(None)?;
    let entries = snapshot["sessions"].as_array().ok_or("Session list unavailable")?;
    let candidates: Vec<_> = entries.iter().take(128).filter_map(|entry| {
        let id = crate::destinations::canonical_session(entry["id"].as_str()?)?;
        let purpose: String = entry["purpose"].as_str().unwrap_or("").chars().take(160).flat_map(char::escape_default).collect();
        Some((id, purpose))
    }).collect();
    if candidates.is_empty() {
        dialog(&args("info", "Choose session", "No open sessions. Open a session from a client first."), None)?;
        return Ok(());
    }
    let rows: Vec<_> = candidates.iter().map(|(id, purpose)| format!("{purpose} ({id})")).collect();
    if let Some(index) = choose(&mut dialog, "Choose session", "Selection pauses desktop input. Resume explicitly after checking the selected task. Only the first 128 open sessions are shown.", &rows)? {
        crate::visor::session_selection_request(Some(&candidates[index].0))?;
        dialog(&args("info", "Session selected", "Desktop input is paused. Check the selected task in the companion before resuming."), None)?;
    }
    Ok(())
}
