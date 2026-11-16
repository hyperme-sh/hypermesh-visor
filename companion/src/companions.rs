//! Native controls for the independent Linux desktop mode.
use crate::mcp_dialog::{args, choose, dialog};
use serde_json::{json, Value};

fn scoped(entry: &Value, action: &str) -> Result<Value, String> {
    let mut value = json!({"action":action});
    for key in ["companion", "session", "generation"] {
        value[key] = crate::destinations::canonical_session(
            entry[key].as_str().ok_or("Missing companion scope")?,
        )
        .ok_or("Invalid companion scope")?
        .into();
    }
    Ok(value)
}
pub fn run(confirmation: crate::visor::Confirmation) -> Result<(), String> {
    loop {
        let snapshot = crate::visor::manager_request(json!({"action":"list"}))?;
        let companions = snapshot["companions"]
            .as_array()
            .ok_or("Companion list unavailable")?;
        let retained = snapshot["retained"]
            .as_array()
            .ok_or("Retained homes unavailable")?;
        if retained.len() > 128 {
            return Err("Invalid retained home count".into());
        }
        if companions.len() > 4 {
            return Err("Invalid companion count".into());
        }
        let mut rows = vec!["+ Add a private Linux desktop (2 GiB, 1 CPU)".into()];
        rows.extend(companions.iter().map(|c| {
            format!(
                "{} — {} — {}",
                if c["native"] == true {
                    "Native desktop"
                } else {
                    "Linux container desktop"
                },
                c["companion"].as_str().unwrap_or("unknown"),
                if c["native"] == true {
                    "state shown in native controls"
                } else {
                    c["phase"].as_str().unwrap_or("unknown")
                }
            )
        }));
        rows.extend(retained.iter().map(|c| {
            format!(
                "Retained home — {}",
                c["companion"].as_str().unwrap_or("unknown")
            )
        }));
        let Some(index) = choose(&mut dialog, "Companions", "One native desktop; up to three private Linux desktops. Physical host input pauses all managed desktops. Containers have no network or shared host files.", &rows)? else { return Ok(()); };
        if index == 0 {
            if dialog(&args("question", "Add companion", "Create a paused Linux desktop with its own home and session? The installed TEST image is required."), None)?.is_some() {
                crate::visor::manager_request(json!({"action":"add"}))?;
            }
            continue;
        }
        if index > companions.len() {
            let entry = &retained[index - companions.len() - 1];
            let Some(action) = choose(&mut dialog, "Stopped companion", "The saved home stays isolated. Restoring configuration creates an empty, paused desktop with a fresh identity; it does not recover files or credentials.", &["Restore configuration to a new desktop".into(), "Delete retained home".into()])? else { continue; };
            let (title, message, operation) = if action == 0 {
                ("Restore companion configuration", "Create a fresh paused desktop using the saved resource limits? No files, secrets, assignments or grants are copied.", "restore")
            } else {
                (
                    "Delete retained home",
                    "Permanently delete this stopped companion's private home?",
                    "delete_home",
                )
            };
            if dialog(&args("question", title, message), None)?.is_some() {
                crate::visor::manager_request(scoped(entry, operation)?)?;
            }
            continue;
        }
        let entry = &companions[index - 1];
        if entry["native"] == true {
            dialog(&args("info", "Native desktop", "Use Choose session and the existing native pause/resume controls for this desktop."), None)?;
            continue;
        }
        let Some(action) = choose(
            &mut dialog,
            "Linux companion",
            "Choose an operation for this exact desktop and session.",
            &[
                "View desktop snapshot".into(),
                "Resume".into(),
                "Stop and retain private home".into(),
                "Stop and delete private home".into(),
                "Clone configuration to a new desktop".into(),
            ],
        )?
        else {
            continue;
        };
        if action == 0 {
            let frame = crate::visor::manager_request(scoped(entry, "view")?)?;
            snapshot_window(
                &frame,
                entry["companion"].as_str().ok_or("Invalid companion")?,
            )?;
        } else if action == 1 {
            if dialog(&args("question", "Resume companion", &format!("Allow desktop input for companion {} and session {}? Host input will pause it again.", entry["companion"], entry["session"])), None)?.is_none() { continue; }
            // Request a fresh pause-bound offer after the dialog's own physical
            // click. Any later host activity still invalidates it server-side.
            confirmation.wait()?;
            let offer = crate::visor::manager_request(scoped(entry, "offer")?)?;
            crate::visor::manager_request(json!({"action":"resume","offer":offer["offer"]}))?;
        } else if action == 4 {
            if dialog(&args("question", "Clone companion configuration", "Create an empty, paused desktop with the same resource limits and a fresh identity? No files, credentials, assignments or grants are copied."), None)?.is_some() {
                crate::visor::manager_request(scoped(entry, "clone")?)?;
            }
        } else {
            if dialog(&args("question", "Stop companion", if action == 3 { "Stop this desktop and permanently delete its private home?" } else { "Stop this desktop and retain its private home? Retained homes are never automatically attached to another identity." }), None)?.is_none() { continue; }
            let mut request = scoped(entry, "stop")?;
            request["retain_home"] = (action == 2).into();
            crate::visor::manager_request(request)?;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_preserves_full_scope_and_rejects_incomplete_routes() {
        let entry = json!({"companion":"11111111-1111-4111-8111-111111111111", "session":"22222222-2222-4222-8222-222222222222", "generation":"33333333-3333-4333-8333-333333333333"});
        let request = scoped(&entry, "stop").unwrap();
        for key in ["companion", "session", "generation"] {
            assert_eq!(entry[key], request[key]);
        }
        assert!(scoped(&json!({"session":entry["session"]}), "stop").is_err());
    }
}

fn snapshot_window(frame: &Value, companion: &str) -> Result<(), String> {
    use base64::Engine;
    use x11rb::{
        connection::Connection,
        protocol::{
            xproto::{
                AtomEnum, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, ImageFormat,
                ImageOrder, PropMode, WindowClass,
            },
            Event,
        },
        wrapper::ConnectionExt as _,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(frame["png"].as_str().ok_or("Missing desktop frame")?)
        .map_err(|_| "Invalid desktop frame")?;
    if bytes.len() > 700000 {
        return Err("Desktop frame exceeds the view limit".into());
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| "Invalid desktop PNG")?;
    if reader.info().width > 1280 || reader.info().height > 800 {
        return Err("Desktop dimensions exceed the view limit".into());
    }
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| "Invalid desktop PNG")?;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return Err("Unsupported desktop image format".into()),
    };
    let (conn, screen_number) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let screen = &conn.setup().roots[screen_number];
    let format = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == screen.root_depth)
        .ok_or("Unsupported display format")?;
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == screen.root_visual)
        .ok_or("Unsupported display visual")?;
    if format.bits_per_pixel != 32
        || visual.red_mask != 0xff0000
        || visual.green_mask != 0xff00
        || visual.blue_mask != 0xff
    {
        return Err("Unsupported display pixel layout".into());
    }
    let lsb = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
    let mut data = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for p in pixels[..info.buffer_size()].chunks_exact(channels) {
        let value = (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
        data.extend_from_slice(&if lsb {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    let window = conn.generate_id().map_err(|e| e.to_string())?;
    let gc = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_window(
        screen.root_depth,
        window,
        screen.root,
        0,
        0,
        info.width as u16,
        info.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS),
    )
    .map_err(|e| e.to_string())?;
    conn.create_gc(gc, window, &CreateGCAux::new())
        .map_err(|e| e.to_string())?;
    let title = format!("Companion {companion} — desktop snapshot (view only)");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    let protocols = conn
        .intern_atom(false, b"WM_PROTOCOLS")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    let close = conn
        .intern_atom(false, b"WM_DELETE_WINDOW")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        protocols,
        AtomEnum::ATOM,
        &[close],
    )
    .map_err(|e| e.to_string())?;
    conn.map_window(window).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;
    loop {
        match conn.wait_for_event().map_err(|e| e.to_string())? {
            Event::Expose(_) => {
                let stride = info.width as usize * 4;
                for (row, pixels) in data.chunks(stride * 8).enumerate() {
                    conn.put_image(
                        ImageFormat::Z_PIXMAP,
                        window,
                        gc,
                        info.width as u16,
                        (pixels.len() / stride) as u16,
                        0,
                        (row * 8) as i16,
                        0,
                        screen.root_depth,
                        pixels,
                    )
                    .map_err(|e| e.to_string())?;
                }
                conn.flush().map_err(|e| e.to_string())?;
            }
            Event::KeyPress(_) | Event::DestroyNotify(_) => break,
            Event::ClientMessage(message) if message.data.as_data32()[0] == close => break,
            _ => {}
        }
    }
    Ok(())
}
