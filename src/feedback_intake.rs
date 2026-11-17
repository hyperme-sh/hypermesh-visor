//! Explicit native intake operations. No discovery, credential entry, persistence or retries.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::Read,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const LIMIT: usize = 24576;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeError {
    Refused,
    Unknown,
}
impl fmt::Display for IntakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Refused => "Feedback request refused",
            Self::Unknown => "Feedback outcome unknown",
        })
    }
}
impl std::error::Error for IntakeError {}

#[derive(Deserialize, PartialEq, Eq)]
struct Scope {
    iss: String,
    sub: String,
    tenant_id: String,
    device_id: String,
}
fn scope(grant: &str) -> Result<Scope, IntakeError> {
    let refuse = IntakeError::Refused;
    if grant.len() > 4000 {
        return Err(refuse);
    }
    let parts: Vec<_> = grant
        .strip_prefix("hm_feedback_")
        .ok_or(refuse)?
        .split('.')
        .collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    {
        return Err(refuse);
    }
    #[derive(Deserialize)]
    struct Claims {
        #[serde(flatten)]
        scope: Scope,
        aud: String,
        token_use: String,
        iat: u64,
        exp: u64,
        jti: Uuid,
    }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| refuse)?);
    let c: Claims = serde_json::from_slice(&bytes).map_err(|_| refuse)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| refuse)?
        .as_secs();
    if c.aud != "hypermesh-feedback-v1"
        || c.token_use != "native-feedback-v1"
        || c.jti.is_nil()
        || c.iat > now
        || c.exp <= now
        || c.exp
            .checked_sub(c.iat)
            .filter(|n| *n > 0 && *n <= 60)
            .is_none()
    {
        return Err(refuse);
    }
    for id in [&c.scope.sub, &c.scope.tenant_id, &c.scope.device_id] {
        let parsed = Uuid::parse_str(id).map_err(|_| refuse)?;
        if parsed.is_nil() || parsed.to_string() != *id {
            return Err(refuse);
        }
    }
    let issuer = url::Url::parse(&c.scope.iss).map_err(|_| refuse)?;
    if issuer.scheme() != "https"
        || issuer.host_str().is_none()
        || !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
    {
        return Err(refuse);
    }
    // These are local binding checks only. Intake verifies the signature.
    Ok(c.scope)
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub enabled: bool,
    pub revision: u64,
    notice: String,
    purpose: String,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Bug,
    Feature,
}

/// Identity captured from an authenticated grant, without retaining its token.
/// The binding may outlive that grant; every replacement grant is validated fresh.
pub struct IdentityBinding(Scope);
impl IdentityBinding {
    pub fn from_grant(grant: &str) -> Result<Self, IntakeError> { Ok(Self(scope(grant)?)) }
    pub fn accepts(&self, grant: &str) -> Result<(), IntakeError> {
        if scope(grant)? == self.0 { Ok(()) } else { Err(IntakeError::Refused) }
    }
}
impl fmt::Debug for IdentityBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdentityBinding([REDACTED])")
    }
}

/// Immutable confirmed bytes; discard this object on edit, withdrawal or account change.
/// Its Debug output excludes identity, destination and report text.
pub struct ConfirmedReport {
    origin: String,
    scope: Scope,
    id: Uuid,
    body: Zeroizing<Vec<u8>>,
}
impl fmt::Debug for ConfirmedReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConfirmedReport([REDACTED])")
    }
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReportState {
    Pending,
    Spam,
    Duplicate,
    Accepted,
    Rejected,
    Publishing,
    Published,
    Closed,
}
#[derive(Debug, Deserialize)]
pub struct Submitted {
    pub id: Uuid,
    pub state: ReportState,
}

/// No destination by default. The caller supplies trusted application configuration,
/// never a URL entered in report text or returned alongside a grant.
#[derive(Default)]
pub struct IntakeClient {
    origin: Option<String>,
    component: String,
}
impl IntakeClient {
    pub fn configured(origin: &str, component: &str) -> Result<Self, IntakeError> {
        let url = url::Url::parse(origin).map_err(|_| IntakeError::Refused)?;
        if origin.len() > 2048
            || origin.chars().any(char::is_whitespace)
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || component.is_empty()
            || component.len() > 40
            || !component.as_bytes()[0].is_ascii_lowercase()
            || !component
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(IntakeError::Refused);
        }
        Ok(Self {
            origin: Some(url.origin().ascii_serialization()),
            component: component.into(),
        })
    }
    /// Blocking operation: desktop callers must use their background worker.
    pub fn read_consent(&self, grant: &str, local_opt_in: bool) -> Result<Consent, IntakeError> {
        self.consent_with(grant, local_opt_in, None, http)
    }
    /// Withdrawal is allowed with local opt-in false; enabling requires true.
    pub fn change_consent(
        &self,
        grant: &str,
        enabled: bool,
        expected_revision: u64,
        local_opt_in: bool,
    ) -> Result<Consent, IntakeError> {
        self.consent_with(
            grant,
            local_opt_in,
            Some((enabled, expected_revision)),
            http,
        )
    }
    fn consent_with(
        &self,
        grant: &str,
        local: bool,
        change: Option<(bool, u64)>,
        send: impl FnOnce(Wire<'_>) -> Result<Vec<u8>, IntakeError>,
    ) -> Result<Consent, IntakeError> {
        if !local && change.map(|v| v.0) != Some(false) {
            return Err(IntakeError::Refused);
        }
        scope(grant)?;
        let origin = self.origin.as_ref().ok_or(IntakeError::Refused)?;
        let payload=change.map(|(enabled,revision)|serde_json::to_vec(&serde_json::json!({"enabled":enabled,"expected_revision":revision,"notice":"human-reports-v1","purpose":"product-feedback"})).unwrap());
        let raw = Zeroizing::new(send(Wire {
            url: format!("{origin}/v1/consent"),
            method: if change.is_some() { "PUT" } else { "GET" },
            grant,
            body: payload.as_deref(),
            id: None,
            status: 200,
        })?);
        let value: Consent = serde_json::from_slice(&raw).map_err(|_| IntakeError::Unknown)?;
        if value.notice != "human-reports-v1"
            || value.purpose != "product-feedback"
            || (value.enabled && value.revision == 0)
            || change.is_some_and(|(enabled, revision)| {
                enabled != value.enabled
                    || (value.revision != revision
                        && Some(value.revision) != revision.checked_add(1))
            })
        {
            return Err(IntakeError::Unknown);
        }
        Ok(value)
    }
    /// Call only for the exact user-reviewed text after a fresh explicit confirmation.
    pub fn confirm(
        &self,
        grant: &str,
        category: Category,
        title: &str,
        body: &str,
        consent_revision: u64,
        confirmed: bool,
    ) -> Result<ConfirmedReport, IntakeError> {
        let origin = self.origin.as_ref().ok_or(IntakeError::Refused)?;
        if !confirmed
            || consent_revision == 0
            || !(4..=160).contains(&title.chars().count())
            || !(10..=4000).contains(&body.chars().count())
            || title.trim().is_empty()
            || body.trim().is_empty()
            || [title, body].iter().any(|text| {
                text.chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            })
        {
            return Err(IntakeError::Refused);
        }
        let payload=Zeroizing::new(serde_json::to_vec(&serde_json::json!({"consent_revision":consent_revision,"category":category,"component":self.component,"title":title,"body":body,"preview_confirmed":true})).map_err(|_|IntakeError::Refused)?);
        if payload.len() > LIMIT {
            return Err(IntakeError::Refused);
        }
        Ok(ConfirmedReport {
            origin: origin.clone(),
            scope: scope(grant)?,
            id: Uuid::new_v4(),
            body: payload,
        })
    }
    /// No automatic retry. On Unknown, retain this same object for an explicit retry
    /// with a fresh grant for the same identity, or discard it on withdrawal/edit.
    pub fn submit(
        &self,
        grant: &str,
        report: &ConfirmedReport,
        local_opt_in: bool,
    ) -> Result<Submitted, IntakeError> {
        self.submit_with(grant, report, local_opt_in, http)
    }
    fn submit_with(
        &self,
        grant: &str,
        report: &ConfirmedReport,
        local: bool,
        send: impl FnOnce(Wire<'_>) -> Result<Vec<u8>, IntakeError>,
    ) -> Result<Submitted, IntakeError> {
        if !local || self.origin.as_ref() != Some(&report.origin) || scope(grant)? != report.scope {
            return Err(IntakeError::Refused);
        }
        let raw = Zeroizing::new(send(Wire {
            url: format!("{}/v1/reports", report.origin),
            method: "POST",
            grant,
            body: Some(&report.body),
            id: Some(report.id),
            status: 201,
        })?);
        let value: Submitted = serde_json::from_slice(&raw).map_err(|_| IntakeError::Unknown)?;
        if value.id.is_nil() {
            return Err(IntakeError::Unknown);
        }
        Ok(value)
    }
}
struct Wire<'a> {
    url: String,
    method: &'static str,
    grant: &'a str,
    body: Option<&'a [u8]>,
    id: Option<Uuid>,
    status: u16,
}
fn http(wire: Wire<'_>) -> Result<Vec<u8>, IntakeError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .redirects(0)
        .try_proxy_from_env(false)
        .build();
    let mut request = agent
        .request(wire.method, &wire.url)
        .set("Authorization", &format!("Bearer {}", wire.grant))
        .set("Accept", "application/json");
    if let Some(id) = wire.id {
        request = request.set("Idempotency-Key", &id.to_string());
    }
    let result = match wire.body {
        Some(body) => request
            .set("Content-Type", "application/json")
            .send_bytes(body),
        None => request.call(),
    };
    let response = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) if (400..500).contains(&code) => {
            return Err(IntakeError::Refused)
        }
        Err(_) => return Err(IntakeError::Unknown),
    };
    if response.status() != wire.status {
        return Err(IntakeError::Unknown);
    }
    let mut raw = Zeroizing::new(Vec::new());
    response
        .into_reader()
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| IntakeError::Unknown)?;
    if raw.len() > LIMIT {
        return Err(IntakeError::Unknown);
    }
    Ok(std::mem::take(&mut *raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn grant(subject: Uuid) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let payload = json!({"iss":"https://plane.test","sub":subject,"tenant_id":"11111111-1111-4111-8111-111111111111","device_id":"22222222-2222-4222-8222-222222222222","aud":"hypermesh-feedback-v1","token_use":"native-feedback-v1","iat":now,"exp":now+60,"jti":Uuid::new_v4()});
        format!(
            "hm_feedback_e30.{}.c2ln",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }
    fn client() -> IntakeClient {
        IntakeClient::configured("https://intake.test/", "visor").unwrap()
    }
    fn consent(enabled: bool, revision: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({"enabled":enabled,"revision":revision,"notice":"human-reports-v1","purpose":"product-feedback"})).unwrap()
    }
    #[test]
    fn identity_binding_accepts_fresh_grants_without_retaining_old_token() {
        let subject = Uuid::new_v4();
        let token = grant(subject);
        let binding = IdentityBinding::from_grant(&token).unwrap();
        drop(token);
        assert!(binding.accepts(&grant(subject)).is_ok());
        assert_eq!(binding.accepts(&grant(Uuid::new_v4())), Err(IntakeError::Refused));
        assert_eq!(format!("{binding:?}"), "IdentityBinding([REDACTED])");
    }

    #[test]
    fn default_and_invalid_configuration_refuse() {
        assert!(IntakeClient::default().origin.is_none());
        for origin in [
            "http://intake.test",
            "https://user@intake.test",
            "https://intake.test/path",
            "https://intake.test?x=1",
            "https://intake.test/#x",
            " https://intake.test",
        ] {
            assert!(IntakeClient::configured(origin, "visor").is_err());
        }
        for component in ["", "Upper", "../other"] {
            assert!(IntakeClient::configured("https://intake.test", component).is_err());
        }
    }
    #[test]
    fn consent_requires_opt_in_but_allows_withdrawal() {
        let client = client();
        let grant = grant(Uuid::new_v4());
        assert_eq!(
            client
                .consent_with(&grant, false, None, |_| panic!("network"))
                .unwrap_err(),
            IntakeError::Refused
        );
        assert_eq!(
            client
                .consent_with(&grant, false, Some((true, 0)), |_| panic!("network"))
                .unwrap_err(),
            IntakeError::Refused
        );
        let result=client.consent_with(&grant,false,Some((false,2)), |w|{
            assert_eq!(w.method,"PUT");assert_eq!(w.url,"https://intake.test/v1/consent");assert!(w.id.is_none());
            let body:serde_json::Value=serde_json::from_slice(w.body.unwrap()).unwrap();
            assert_eq!(body,json!({"enabled":false,"expected_revision":2,"notice":"human-reports-v1","purpose":"product-feedback"}));
            Ok(consent(false,3))
        }).unwrap();
        assert!(!result.enabled);
        assert!(
            client
                .consent_with(&grant, true, None, |w| {
                    assert_eq!(w.method, "GET");
                    assert!(w.body.is_none());
                    Ok(consent(true, 1))
                })
                .unwrap()
                .enabled
        );
        assert_eq!(
            client
                .consent_with(&grant, true, None, |_| Ok(b"private malformed".to_vec()))
                .unwrap_err(),
            IntakeError::Unknown
        );
    }
    #[test]
    fn expired_or_mis_scoped_grants_never_send() {
        let token = grant(Uuid::new_v4());
        let segment = token.split('.').nth(1).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segment).unwrap()).unwrap();
        let now = original["iat"].as_u64().unwrap();
        for (field, value) in [
            ("exp", json!(now - 1)),
            ("iat", json!(now + 60)),
            ("exp", json!(now + 61)),
            ("aud", json!("other-service")),
            ("token_use", json!("machine")),
            ("sub", json!(Uuid::nil())),
            ("tenant_id", json!("invalid")),
            ("device_id", json!(Uuid::nil())),
            ("jti", json!(Uuid::nil())),
            ("iss", json!("http://plane.test")),
        ] {
            let mut payload = original.clone();
            payload[field] = value;
            let invalid = format!(
                "hm_feedback_e30.{}.c2ln",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
            );
            assert_eq!(
                client()
                    .consent_with(&invalid, true, None, |_| panic!("network for {field}"))
                    .unwrap_err(),
                IntakeError::Refused
            );
        }
    }
    #[test]
    fn consent_update_rejects_unrelated_revision() {
        let token = grant(Uuid::new_v4());
        for revision in [0, 1, 5, u64::MAX] {
            assert_eq!(
                client()
                    .consent_with(&token, true, Some((true, 2)), |_| Ok(consent(
                        true, revision
                    )))
                    .unwrap_err(),
                IntakeError::Unknown
            );
        }
        for revision in [2, 3] {
            assert!(client()
                .consent_with(&token, true, Some((true, 2)), |_| Ok(consent(
                    true, revision
                )))
                .is_ok());
        }
    }
    #[test]
    fn confirmation_is_bounded_and_debug_redacts_text() {
        let client = client();
        let grant = grant(Uuid::new_v4());
        assert!(client
            .confirm(&grant, Category::Bug, "Title", "User report text", 1, false)
            .is_err());
        assert!(client
            .confirm(&grant, Category::Bug, "abc", "User report text", 1, true)
            .is_err());
        assert!(client
            .confirm(&grant, Category::Bug, "Title", "User report text", 0, true)
            .is_err());
        assert!(client
            .confirm(&grant, Category::Bug, "Title", &"x".repeat(4001), 1, true)
            .is_err());
        assert!(client
            .confirm(&grant, Category::Bug, "Title", "secret\0payload", 1, true)
            .is_err());
        let report = client
            .confirm(
                &grant,
                Category::Feature,
                "Private title",
                "Private manual report text",
                1,
                true,
            )
            .unwrap();
        assert_eq!(format!("{report:?}"), "ConfirmedReport([REDACTED])");
    }
    #[test]
    fn uncertain_send_keeps_identical_bytes_id_and_scope() {
        let client = client();
        let subject = Uuid::new_v4();
        let token = grant(subject);
        let report = client
            .confirm(
                &token,
                Category::Bug,
                "Title",
                "Manual report text",
                3,
                true,
            )
            .unwrap();
        let mut sent = None;
        assert_eq!(
            client
                .submit_with(&token, &report, true, |w| {
                    assert_eq!(w.url, "https://intake.test/v1/reports");
                    assert_eq!(w.method, "POST");
                    let payload: serde_json::Value =
                        serde_json::from_slice(w.body.unwrap()).unwrap();
                    assert_eq!(payload["preview_confirmed"], true);
                    assert_eq!(payload["consent_revision"], 3);
                    assert_eq!(payload["component"], "visor");
                    assert_eq!(payload.as_object().unwrap().len(), 6);
                    sent = Some((w.id, w.body.unwrap().to_vec()));
                    Err(IntakeError::Unknown)
                })
                .unwrap_err(),
            IntakeError::Unknown
        );
        let fresh = grant(subject);
        let result = client
            .submit_with(&fresh, &report, true, |w| {
                assert_eq!(sent.as_ref().unwrap(), &(w.id, w.body.unwrap().to_vec()));
                Ok(serde_json::to_vec(
                    &json!({"id":Uuid::new_v4(),"state":"pending","body":"server text ignored"}),
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(result.state, ReportState::Pending);
        assert_eq!(
            client
                .submit_with(&grant(Uuid::new_v4()), &report, true, |_| panic!("network"))
                .unwrap_err(),
            IntakeError::Refused
        );
        assert_eq!(
            client
                .submit_with(&fresh, &report, false, |_| panic!("network"))
                .unwrap_err(),
            IntakeError::Refused
        );
        let other = IntakeClient::configured("https://other.test", "visor").unwrap();
        assert_eq!(
            other
                .submit_with(&fresh, &report, true, |_| panic!("network"))
                .unwrap_err(),
            IntakeError::Refused
        );
    }
    #[test]
    fn invalid_credentials_and_success_bodies_are_fixed_errors() {
        let client = client();
        let token = grant(Uuid::new_v4());
        for invalid in ["hm_msi_secret", "hm_feedback_a.b.c\r\n", "hm_feedback_", ""] {
            assert!(scope(invalid).is_err());
        }
        let report = client
            .confirm(
                &token,
                Category::Bug,
                "Title",
                "Manual report text",
                1,
                true,
            )
            .unwrap();
        for raw in [
            b"private malformed".as_slice(),
            b"{}",
            b"{\"id\":\"00000000-0000-0000-0000-000000000000\",\"state\":\"pending\"}",
        ] {
            assert_eq!(
                client
                    .submit_with(&token, &report, true, |_| Ok(raw.to_vec()))
                    .unwrap_err(),
                IntakeError::Unknown
            );
        }
        assert_eq!(IntakeError::Unknown.to_string(), "Feedback outcome unknown");
    }
    #[test]
    fn http_bounds_responses_and_does_not_follow_redirects() {
        use std::{net::TcpListener, thread};
        for (status, body, error) in [
            (302, "redirect".to_owned(), IntakeError::Unknown),
            (401, "secret".to_owned(), IntakeError::Refused),
            (500, "secret".to_owned(), IntakeError::Unknown),
            (200, "x".repeat(LIMIT + 1), IntakeError::Unknown),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let task = thread::spawn(move || {
                use std::io::Write;
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 2048];
                let n = socket.read(&mut request).unwrap();
                let raw = String::from_utf8_lossy(&request[..n]);
                assert!(raw.starts_with("GET /v1/consent "));
                assert!(raw
                    .to_lowercase()
                    .contains("authorization: bearer hm_feedback_test"));
                write!(socket,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/never\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            });
            let result = http(Wire {
                url: format!("http://{address}/v1/consent"),
                method: "GET",
                grant: "hm_feedback_test",
                body: None,
                id: None,
                status: 200,
            });
            assert_eq!(result.unwrap_err(), error);
            task.join().unwrap();
        }
    }
}
