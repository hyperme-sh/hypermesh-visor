//! Explicit authenticated Host grant acquisition; no report sending or automatic configuration.
use crate::host_bridge::Bridge;
use serde::Deserialize;
use std::{fmt, path::PathBuf};
use zeroize::Zeroizing;

/// Disabled unless a caller explicitly supplies its configured Host socket.
#[derive(Default)]
pub struct FeedbackGrantClient {
    bridge: Option<Bridge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantUnavailable;
impl fmt::Display for GrantUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Feedback grant unavailable")
    }
}
impl std::error::Error for GrantUnavailable {}

impl FeedbackGrantClient {
    pub fn configured(socket: PathBuf) -> Result<Self, GrantUnavailable> {
        #[cfg(windows)]
        if socket != std::path::Path::new(hypermesh_local_transport::WINDOWS_FEEDBACK_PIPE) { return Err(GrantUnavailable); }
        Ok(Self { bridge: Some(Bridge::new(socket).map_err(|_| GrantUnavailable)?) })
    }

    /// The assertion represents a local opt-in, not server consent or a send action.
    /// No retry is performed. Disabled clients never connect to Host.
    pub async fn request(&self, human_reports_opt_in: bool) -> Result<FeedbackGrant, GrantUnavailable> {
        self.bridge.as_ref().ok_or(GrantUnavailable)?
            .feedback_grant(human_reports_opt_in).await.map_err(|_| GrantUnavailable)
    }
}

/// A credential from the authenticated root Host peer. Intake verifies its signature.
/// Use immediately; do not cache, persist, log or send it to any other destination.
pub struct FeedbackGrant {
    bearer: Zeroizing<String>,
}
impl fmt::Debug for FeedbackGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FeedbackGrant([REDACTED])")
    }
}
impl FeedbackGrant {
    pub fn bearer(&self) -> &str { &self.bearer }

    pub(crate) fn from_response(value: serde_json::Value) -> Result<Self, ()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reply { version: u32, grant: String, expires_in: u32 }
        let reply: Reply = serde_json::from_value(value).map_err(|_| ())?;
        let token = Zeroizing::new(reply.grant);
        if reply.version != 1 || reply.expires_in != 60 || token.len() > 4000 {
            return Err(());
        }
        let compact = token.strip_prefix("hm_feedback_").ok_or(())?;
        let parts: Vec<_> = compact.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()
            || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')) {
            return Err(());
        }
        Ok(Self { bearer: token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn default_and_no_opt_in_refuse_without_connecting() {
        assert_eq!(FeedbackGrantClient::default().request(true).await.unwrap_err(), GrantUnavailable);
        #[cfg(unix)]
        let path = "/nonexistent/host.sock";
        #[cfg(windows)]
        let path = hypermesh_local_transport::WINDOWS_FEEDBACK_PIPE;
        let client = FeedbackGrantClient::configured(path.into()).unwrap();
        assert_eq!(client.request(false).await.unwrap_err(), GrantUnavailable);
        assert!(FeedbackGrantClient::configured("relative.sock".into()).is_err());
        #[cfg(windows)]
        assert!(FeedbackGrantClient::configured(hypermesh_local_transport::WINDOWS_CRASH_PIPE.into()).is_err());
    }
    #[test]
    fn exact_envelope_scoped_token_and_redaction() {
        let good = json!({"version":1,"expires_in":60,"grant":"hm_feedback_header.payload.signature"});
        let grant = FeedbackGrant::from_response(good.clone()).unwrap();
        assert_eq!(grant.bearer(), "hm_feedback_header.payload.signature");
        assert!(!format!("{grant:?}").contains("payload"));
        for key in ["version", "expires_in", "grant"] {
            let mut value = good.clone();
            value.as_object_mut().unwrap().remove(key);
            assert!(FeedbackGrant::from_response(value).is_err());
        }
        for token in ["hm_msi_secret", "hm_feedback_", "hm_feedback_a.b", "hm_feedback_a.b.c.d",
            "hm_feedback_a..c", "hm_feedback_a.b.c\n", "hm_feedback_a.b.c="] {
            let mut value = good.clone();
            value["grant"] = json!(token);
            assert!(FeedbackGrant::from_response(value).is_err());
        }
        for (key, replacement) in [("version", json!(2)), ("expires_in", json!(61)),
            ("extra", json!("private")), ("grant", json!("a".repeat(4001)))] {
            let mut value = good.clone();
            value[key] = replacement;
            assert!(FeedbackGrant::from_response(value).is_err());
        }
    }
}
