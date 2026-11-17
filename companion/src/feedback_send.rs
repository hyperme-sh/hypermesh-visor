//! Default-off Linux form composition. Called only on the dialog worker.
use super::feedback::{base, decode};
use hypermesh_visor::feedback_grant::{FeedbackGrant, FeedbackGrantClient};
use hypermesh_visor::feedback_intake::{
    Category, ConfirmedReport, IdentityBinding, IntakeClient, IntakeError,
};

const REFUSED: &str = "Feedback unavailable or refused. No automatic retry was made.";

pub(super) trait Backend {
    fn begin(&mut self) -> Result<bool, IntakeError>;
    fn enable(&mut self) -> Result<(), IntakeError>;
    fn withdraw(&mut self) -> Result<(), IntakeError>;
    fn prepare(&mut self, category: Category, title: &str, body: &str) -> Result<(), IntakeError>;
    fn submit(&mut self) -> Result<(), IntakeError>;
}

pub(super) struct Native {
    runtime: tokio::runtime::Runtime,
    grants: FeedbackGrantClient,
    intake: IntakeClient,
    identity: Option<IdentityBinding>,
    revision: u64,
    pending: Option<ConfirmedReport>,
}
impl Native {
    pub fn from_env() -> Result<Option<Self>, String> {
        let enabled = std::env::var("HYPERMESH_FEEDBACK_ENABLED").ok();
        if !enabled_setting(enabled.as_deref())? { return Ok(None); }
        let origin = std::env::var("HYPERMESH_FEEDBACK_INTAKE_ORIGIN").map_err(|_| REFUSED)?;
        let socket = std::env::var_os("HYPERMESH_HOST_BRIDGE_SOCKET").ok_or(REFUSED)?;
        Ok(Some(Self {
            runtime: tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| REFUSED)?,
            grants: FeedbackGrantClient::configured(socket.into()).map_err(|_| REFUSED)?,
            intake: IntakeClient::configured(&origin, "visor").map_err(|_| REFUSED)?,
            identity: None, revision: 0, pending: None,
        }))
    }
    fn grant(&mut self) -> Result<FeedbackGrant, IntakeError> {
        let grant = self.runtime.block_on(self.grants.request(true)).map_err(|_| IntakeError::Refused)?;
        match &self.identity {
            Some(binding) => binding.accepts(grant.bearer())?,
            None => self.identity = Some(IdentityBinding::from_grant(grant.bearer())?),
        }
        Ok(grant)
    }
}
fn enabled_setting(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(REFUSED.into()),
    }
}
impl Backend for Native {
    fn begin(&mut self) -> Result<bool, IntakeError> {
        let grant = self.grant()?;
        let consent = self.intake.read_consent(grant.bearer(), true)?;
        self.revision = consent.revision;
        Ok(consent.enabled)
    }
    fn enable(&mut self) -> Result<(), IntakeError> {
        let grant = self.grant()?;
        let consent = self.intake.change_consent(grant.bearer(), true, self.revision, true)?;
        self.revision = consent.revision;
        Ok(())
    }
    fn withdraw(&mut self) -> Result<(), IntakeError> {
        self.pending = None;
        // Acquire under the existing local opt-in, then withdraw server consent.
        let grant = self.grant()?;
        self.intake.change_consent(grant.bearer(), false, self.revision, false)?;
        Ok(())
    }
    fn prepare(&mut self, category: Category, title: &str, body: &str) -> Result<(), IntakeError> {
        self.pending = None;
        let grant = self.grant()?;
        let consent = self.intake.read_consent(grant.bearer(), true)?;
        if !consent.enabled || consent.revision != self.revision { return Err(IntakeError::Refused); }
        self.pending = Some(self.intake.confirm(grant.bearer(), category, title, body, self.revision, true)?);
        Ok(())
    }
    fn submit(&mut self) -> Result<(), IntakeError> {
        let grant = self.grant()?;
        self.intake.submit(grant.bearer(), self.pending.as_ref().ok_or(IntakeError::Refused)?, true)?;
        self.pending = None;
        Ok(())
    }
}

type DialogResult = Result<Option<String>, String>;
fn question(ui: &mut impl FnMut(&[String], Option<&str>) -> DialogResult,
    title: &str, text: &str, button: &str) -> Result<bool, String> {
    let mut args = base("question", title);
    args.extend(["--no-markup".into(), format!("--text={text}"),
        format!("--ok-label={button}"), "--cancel-label=Cancel and discard".into()]);
    Ok(ui(&args, None)?.is_some())
}
fn choose(ui: &mut impl FnMut(&[String], Option<&str>) -> DialogResult,
    title: &str, choices: &[&str]) -> DialogResult {
    let mut args = base("list", title);
    args.extend(["--no-markup".into(), "--column=Action".into(), "--".into()]);
    args.extend(choices.iter().map(|s| s.to_string()));
    match ui(&args, None)? {
        Some(value) if choices.contains(&value.trim()) => Ok(Some(value.trim().into())),
        Some(_) => Err(REFUSED.into()),
        None => Ok(None),
    }
}
fn editor(ui: &mut impl FnMut(&[String], Option<&str>) -> DialogResult,
    title: &str, value: &str) -> DialogResult {
    let mut args = base("text-info", title);
    args.extend(["--editable".into(), "--ok-label=Continue".into(),
        "--cancel-label=Discard draft".into()]);
    ui(&args, Some(value))?.map(|v| decode(v.into_bytes())).transpose()
}
fn valid_text(title: &str, body: &str) -> bool {
    (4..=160).contains(&title.chars().count()) && !title.trim().is_empty()
        && (10..=4000).contains(&body.chars().count()) && !body.trim().is_empty()
}

pub(super) fn screen(
    mut ui: impl FnMut(&[String], Option<&str>) -> DialogResult,
    backend: &mut impl Backend,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if !question(&mut ui, "Human feedback",
        "Opt in to human reports and check your saved consent? Only text you type will be sent after preview and explicit confirmation. Do not include prompts, documents, credentials or personal information. No attachments or automatic diagnostics. Closing clears local drafts; saved consent is changed only by an explicit choice.",
        "Opt in")? { return Ok(()); }
    checkpoint()?;
    let enabled = backend.begin().map_err(|_| REFUSED)?;
    let choices = if enabled { vec!["Continue", "Withdraw saved consent"] }
        else { vec!["Enable saved consent", "Withdraw saved consent"] };
    match choose(&mut ui, "Human reports: product-feedback consent", &choices)?.as_deref() {
        None => return Ok(()),
        Some("Withdraw saved consent") => {
            checkpoint()?;
            return backend.withdraw().map_err(|_| "Consent withdrawal could not be confirmed. No report was sent.".into());
        }
        Some("Enable saved consent") => {
            if !question(&mut ui, "Enable human reports",
                "Enable human-reports-v1 consent for product-feedback on this enrolled device? Reports are filtered and reviewed by a human before publication. You can withdraw saved consent from this form.",
                "Enable consent")? { return Ok(()); }
            checkpoint()?;
            backend.enable().map_err(|_| "Consent change could not be confirmed. No report was sent.")?;
        }
        Some("Continue") => {},
        _ => return Err(REFUSED.into()),
    }
    let Some(kind) = choose(&mut ui, "Feedback category", &["Problem", "Idea"])? else { return Ok(()); };
    let category = if kind == "Problem" { Category::Bug } else { Category::Feature };
    let mut title = String::new();
    let mut body = String::new();
    loop {
        let Some(next) = editor(&mut ui, "Report title: 4–160 characters", &title)? else { return Ok(()); };
        title = next;
        let Some(next) = editor(&mut ui, "Report text: 10–4000 characters, at most 4096 UTF-8 bytes", &body)? else { return Ok(()); };
        body = next;
        if !valid_text(&title, &body) { return Err("Report title or text is outside the displayed limits.".into()); }
        let preview = format!("{kind}\n\n{title}\n\n{body}");
        let mut args = base("text-info", "Review exact report text");
        args.extend(["--ok-label=Choose next action".into(), "--cancel-label=Discard draft".into()]);
        if ui(&args, Some(&preview))?.is_none() { return Ok(()); }
        match choose(&mut ui, "Confirm the report you just reviewed",
            &["Send this report", "Edit draft", "Withdraw consent and discard"])?.as_deref() {
            Some("Edit draft") => continue,
            Some("Withdraw consent and discard") => {
                title.clear(); body.clear();
                checkpoint()?;
                return backend.withdraw().map_err(|_| "Consent withdrawal could not be confirmed. No report was sent.".into());
            }
            Some("Send this report") => {},
            None => return Ok(()),
            _ => return Err(REFUSED.into()),
        }
        checkpoint()?;
        backend.prepare(category, &title, &body).map_err(|_| REFUSED)?;
        title.clear(); body.clear();
        loop {
            match backend.submit() {
                Ok(()) => {
                    let mut args = base("info", "Report received");
                    args.extend(["--no-markup".into(), "--text=Intake received the report. This does not mean it was approved or published.".into()]);
                    ui(&args, None)?;
                    return Ok(());
                }
                Err(IntakeError::Refused) => return Err("Submission could not be confirmed. An earlier attempt may have arrived; no automatic retry was made.".into()),
                Err(IntakeError::Unknown) => {
                    if !question(&mut ui, "Report outcome unknown",
                        "The report may already have arrived. Retry the same confirmed report with the same request identity? Cancel discards the local copy and cannot undo a request already sent.",
                        "Retry same report")? { return Ok(()); }
                    checkpoint()?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[derive(Default)]
    struct Fake {
        enabled: Option<bool>,
        events: Vec<String>,
        results: VecDeque<Result<(), IntakeError>>,
        confirmed: Vec<(String, String)>,
    }
    impl Backend for Fake {
        fn begin(&mut self) -> Result<bool, IntakeError> { self.events.push("begin".into()); Ok(self.enabled.unwrap_or(true)) }
        fn enable(&mut self) -> Result<(), IntakeError> { self.events.push("enable".into()); Ok(()) }
        fn withdraw(&mut self) -> Result<(), IntakeError> { self.events.push("withdraw".into()); Ok(()) }
        fn prepare(&mut self, _: Category, title: &str, body: &str) -> Result<(), IntakeError> {
            self.events.push("prepare".into()); self.confirmed.push((title.into(), body.into())); Ok(())
        }
        fn submit(&mut self) -> Result<(), IntakeError> {
            self.events.push("submit".into()); self.results.pop_front().unwrap_or(Ok(()))
        }
    }
    fn replies() -> Vec<Option<&'static str>> {
        vec![Some(""), Some("Continue"), Some("Problem"), Some("Private title"),
            Some("Private manual report"), Some(""), Some("Send this report"), None]
    }
    fn run(values: Vec<Option<&str>>, fake: &mut Fake) {
        let mut values: VecDeque<_> = values.into();
        screen(|args, input| {
            assert!(args.iter().all(|s| !s.contains("Private") && s != "--html" && s != "--markdown"));
            if args.iter().any(|s| s == "--title=Review exact report text") {
                assert!(input.unwrap().contains("Private"));
            }
            Ok(values.pop_front().expect("unexpected dialog").map(str::to_owned))
        }, fake, || Ok(())).unwrap();
        assert!(values.is_empty());
    }
    #[test]
    fn disabled_setting_is_explicit() {
        for value in [None, Some("false")] { assert!(!enabled_setting(value).unwrap()); }
        assert!(enabled_setting(Some("true")).unwrap());
        for value in ["", "1", "TRUE", " true"] { assert!(enabled_setting(Some(value)).is_err()); }
    }
    #[test]
    fn cancellation_never_confirms_or_sends() {
        for cancel in 0..7 {
            let mut values = replies();
            values.truncate(cancel + 1); values[cancel] = None;
            let mut fake = Fake::default();
            run(values, &mut fake);
            assert!(!fake.events.iter().any(|e| e == "prepare" || e == "submit"));
            if cancel == 0 { assert!(fake.events.is_empty()); }
        }
    }
    #[test]
    fn send_follows_exact_plain_preview() {
        let mut fake = Fake::default();
        run(replies(), &mut fake);
        assert_eq!(fake.events, ["begin", "prepare", "submit"]);
        assert_eq!(fake.confirmed, [("Private title".into(), "Private manual report".into())]);
    }
    #[test]
    fn disabled_server_consent_requires_its_own_confirmation() {
        let mut values = replies();
        values[1] = Some("Enable saved consent");
        values.insert(2, Some(""));
        let mut fake = Fake { enabled: Some(false), ..Fake::default() };
        run(values, &mut fake);
        assert_eq!(fake.events, ["begin", "enable", "prepare", "submit"]);
        let mut fake = Fake { enabled: Some(false), ..Fake::default() };
        run(vec![Some(""), Some("Enable saved consent"), None], &mut fake);
        assert_eq!(fake.events, ["begin"]);
    }
    #[test]
    fn editing_requires_new_preview_and_confirmation() {
        let mut values = replies(); values.truncate(6);
        values.extend([Some("Edit draft"), Some("Private changed title"),
            Some("Private changed report"), Some(""), Some("Send this report"), None]);
        let mut fake = Fake::default(); run(values, &mut fake);
        assert_eq!(fake.confirmed, [("Private changed title".into(), "Private changed report".into())]);
        assert_eq!(fake.events, ["begin", "prepare", "submit"]);
    }
    #[test]
    fn explicit_retry_never_reconfirms_or_changes_report() {
        let mut values = replies(); values.pop(); values.extend([Some(""), None]);
        let mut fake = Fake { results: [Err(IntakeError::Unknown), Ok(())].into(), ..Fake::default() };
        run(values, &mut fake);
        assert_eq!(fake.events, ["begin", "prepare", "submit", "submit"]);
        assert_eq!(fake.confirmed.len(), 1);
        let mut fake = Fake { results: [Err(IntakeError::Unknown)].into(), ..Fake::default() };
        run(replies(), &mut fake);
        assert_eq!(fake.events, ["begin", "prepare", "submit"]);
    }
    #[test]
    fn withdrawal_discards_without_preparing_or_sending() {
        let mut fake = Fake::default();
        run(vec![Some(""), Some("Withdraw saved consent")], &mut fake);
        assert_eq!(fake.events, ["begin", "withdraw"]);
        let mut values = replies(); values.truncate(7); values[6] = Some("Withdraw consent and discard");
        let mut fake = Fake::default(); run(values, &mut fake);
        assert_eq!(fake.events, ["begin", "withdraw"]);
    }
    #[test]
    fn failed_observer_checkpoint_never_contacts_backend() {
        let mut fake = Fake::default();
        assert!(screen(|_, _| Ok(Some("".into())), &mut fake, || Err("unavailable".into())).is_err());
        assert!(fake.events.is_empty());
    }
    #[test]
    fn text_bounds_match_intake() {
        assert!(valid_text("Title", "A manual report"));
        for (title, body) in [("abc", "A manual report"), ("Title", "short"), ("    ", "A manual report")] {
            assert!(!valid_text(title, body));
        }
        assert!(!valid_text(&"x".repeat(161), "A manual report"));
        assert!(!valid_text("Title", &"x".repeat(4001)));
    }
}
