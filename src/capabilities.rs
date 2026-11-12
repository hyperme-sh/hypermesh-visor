//! Implementation discovery only. This does not probe devices or grant access.
use serde_json::{json, Value};

pub(crate) fn describe(backend: &str) -> Value {
    let capture = match backend {
        "x11" | "wayland" | "macos-embedded" => "conditional",
        "macos" => "disabled",
        "fixture" => "fixture",
        _ => "unavailable",
    };
    let input = match backend {
        "x11" | "macos-embedded" => "conditional",
        "macos" => "disabled",
        "fixture" => "fixture",
        "wayland" => "disabled",
        _ => "unavailable",
    };
    json!({
        "schema_version": 1,
        "build_mode": if cfg!(feature = "desktop") { "desktop" } else { "headless" },
        "backend": backend,
        "readiness_probed": false,
        "authorization_granted": false,
        "desktop": {
            "view": capture, "watch": capture,
            "listen": if backend.starts_with("macos") { "unavailable" } else { capture },
            "mouse": input, "type": input,
            "input_disabled_reason": match backend {
                "wayland" => Some(crate::desktop::WAYLAND_AUTOMATION),
                "macos" => Some("native automation disabled pending authenticated daemon integration"),
                _ => None
            },
            "capture_disabled_reason": if backend == "macos" {
                Some("native automation disabled pending authenticated daemon integration")
            } else { None },
            "mouse_actions": ["move", "click", "drag", "scroll"],
            "mouse_buttons": ["left", "right", "middle"],
            "type_fields": ["text", "keys", "secret"],
            "key_actions": ["tap", "down", "up"],
            "unsupported": ["clipboard", "display_enumeration", "accessibility_tree", "audio_playback"],
            "coordinates": {
                "space": match backend {
                    "x11" => "root_window_pixels",
                    "macos-embedded" => "primary_display_logical_pixels",
                    "macos" => "unspecified",
                    _ => "unspecified"
                },
                "display_selection": false,
                "dpi_transform": false
            },
            "audio_format": crate::desktop::AUDIO_FORMAT
        },
        "limits": {
            "type_max_keystrokes": crate::input::TYPE_MAX_KEYSTROKES,
            "stream_max_bytes": crate::stream::MAX_STREAM_BYTES,
            "scroll_max_total_ticks": crate::input::SCROLL_MAX_TICKS
        },
        "requirements": {
            "desktop_verbs_require_session": true,
            "input_requires_automation_gate": true,
            "tcp_requires_bearer": true
        },
        "mcp": {
            "discovery_sources": "/mcp/sources",
            "persistent_trust": true,
            "profile_editor": "/mcp/profiles",
            "server_editor": "/mcp/servers",
            "discovery": "/mcp/discovery",
            "trust": "/mcp/trust",
            "trust_requires_companion_socket": true,
            "attached_tools": "/session/{id}/mcp",
            "refresh_tools": "/session/{id}/mcp/refresh",
            "refresh_actions": ["refresh", "reconnect"],
            "resources": "/session/{id}/mcp/resources",
            "resource_actions": ["list", "templates", "read"],
            "inverse_tools": if cfg!(feature = "desktop") { json!(["capabilities", "view", "mouse_move", "mouse_click", "mouse_scroll", "mouse_drag", "type_text", "key_chord", "task_submit", "task_status", "task_cancel"]) } else { json!([]) }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_native_capabilities_remain_conditional_and_do_not_grant_access() {
        let value = describe("macos-embedded");
        assert_eq!(value["desktop"]["view"], "conditional");
        assert_eq!(value["desktop"]["mouse"], "conditional");
        assert_eq!(value["desktop"]["listen"], "unavailable");
        assert_eq!(
            value["desktop"]["coordinates"]["space"],
            "primary_display_logical_pixels"
        );
        assert_eq!(value["authorization_granted"], false);
        assert_eq!(value["readiness_probed"], false);
        assert_eq!(describe("macos")["desktop"]["mouse"], "disabled");
    }

    #[test]
    fn discovery_never_advertises_disabled_or_unknown_input_as_ready() {
        for (backend, status) in [
            ("x11", "conditional"),
            ("macos", "disabled"),
            ("wayland", "disabled"),
            ("fixture", "fixture"),
            ("unknown", "unavailable"),
            ("future", "unavailable"),
        ] {
            let value = describe(backend);
            assert_eq!(value["desktop"]["mouse"], status);
            assert_eq!(value["desktop"]["type"], status);
            assert_eq!(value["readiness_probed"], false);
            assert_eq!(value["authorization_granted"], false);
        }
        assert!(describe("wayland")["desktop"]["input_disabled_reason"]
            .as_str()
            .unwrap()
            .contains("#17"));
    }
    #[test]
    fn macos_discovery_preserves_native_bridge_refusal() {
        let value = describe("macos");
        assert_eq!(value["desktop"]["view"], "disabled");
        assert_eq!(value["desktop"]["watch"], "disabled");
        assert_eq!(value["desktop"]["listen"], "unavailable");
        assert!(value["desktop"]["capture_disabled_reason"]
            .as_str()
            .unwrap()
            .contains("authenticated daemon"));
        assert!(value["desktop"]["input_disabled_reason"]
            .as_str()
            .unwrap()
            .contains("authenticated daemon"));
        assert_eq!(value["desktop"]["coordinates"]["space"], "unspecified");
        assert_eq!(value["desktop"]["coordinates"]["display_selection"], false);
        assert_eq!(value["authorization_granted"], false);
    }
}
