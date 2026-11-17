//! In-process native form state. No externally reachable route or credential output.
use crate::feedback_grant::{FeedbackGrant, FeedbackGrantClient};
use crate::feedback_intake::{Category, ConfirmedReport, IdentityBinding, IntakeClient, IntakeError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

trait Backend {
    fn consent(&mut self) -> Result<bool, IntakeError>;
    fn enable(&mut self) -> Result<(), IntakeError>;
    fn withdraw(&mut self) -> Result<(), IntakeError>;
    fn prepare(&mut self, category: Category, title: &str, body: &str) -> Result<(), IntakeError>;
    fn submit(&mut self) -> Result<(), IntakeError>;
    fn discard(&mut self);
}
struct Native {
    runtime: tokio::runtime::Runtime,
    grants: FeedbackGrantClient,
    intake: IntakeClient,
    identity: Option<IdentityBinding>,
    revision: u64,
    pending: Option<ConfirmedReport>,
}
impl Native {
    fn configured() -> Result<Self, IntakeError> {
        if std::env::var("HYPERMESH_FEEDBACK_ENABLED").ok().as_deref() != Some("true") { return Err(IntakeError::Refused); }
        let origin = std::env::var("HYPERMESH_FEEDBACK_INTAKE_ORIGIN").map_err(|_| IntakeError::Refused)?;
        let socket = std::env::var_os("HYPERMESH_HOST_BRIDGE_SOCKET").ok_or(IntakeError::Refused)?;
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| IntakeError::Refused)?,
            grants: FeedbackGrantClient::configured(socket.into()).map_err(|_| IntakeError::Refused)?,
            intake: IntakeClient::configured(&origin,"visor")?, identity: None, revision: 0, pending: None,
        })
    }
    fn grant(&mut self) -> Result<FeedbackGrant, IntakeError> {
        let grant = self.runtime.block_on(self.grants.request(true)).map_err(|_| IntakeError::Refused)?;
        match &self.identity {
            Some(identity) => identity.accepts(grant.bearer())?,
            None => self.identity = Some(IdentityBinding::from_grant(grant.bearer())?),
        }
        Ok(grant)
    }
}
impl Backend for Native {
    fn consent(&mut self) -> Result<bool, IntakeError> {
        let grant = self.grant()?;
        let consent = self.intake.read_consent(grant.bearer(),true)?;
        self.revision = consent.revision;
        Ok(consent.enabled)
    }
    fn enable(&mut self) -> Result<(), IntakeError> {
        let grant = self.grant()?;
        self.revision = self.intake.change_consent(grant.bearer(),true,self.revision,true)?.revision;
        Ok(())
    }
    fn withdraw(&mut self) -> Result<(), IntakeError> {
        self.discard();
        let grant = self.grant()?;
        self.intake.change_consent(grant.bearer(),false,self.revision,false)?;
        Ok(())
    }
    fn prepare(&mut self, category: Category, title: &str, body: &str) -> Result<(), IntakeError> {
        self.discard();
        let grant = self.grant()?;
        let consent = self.intake.read_consent(grant.bearer(),true)?;
        if !consent.enabled || consent.revision != self.revision { return Err(IntakeError::Refused); }
        self.pending = Some(self.intake.confirm(grant.bearer(),category,title,body,self.revision,true)?);
        Ok(())
    }
    fn submit(&mut self) -> Result<(), IntakeError> {
        let grant = self.grant()?;
        self.intake.submit(grant.bearer(),self.pending.as_ref().ok_or(IntakeError::Refused)?,true)?;
        self.discard(); Ok(())
    }
    fn discard(&mut self) { self.pending = None; }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    action: String,
    session: Uuid,
    #[serde(default)] opt_in: bool,
    #[serde(default)] category: String,
    #[serde(default)] title: String,
    #[serde(default)] body: String,
}
struct Form<B> { id: Uuid, backend: B, enabled: bool, prepared: bool }
impl<B: Backend> Form<B> {
    fn command(&mut self, request: &Command) -> Result<Value, IntakeError> {
        if request.session != self.id { return Err(IntakeError::Refused); }
        if request.action == "feedback-discard" {
            self.backend.discard(); self.prepared = false; return Ok(json!({}));
        }
        if request.action == "feedback-withdraw" {
            self.prepared = false; self.enabled = false;
            self.backend.withdraw()?; return Ok(json!({"enabled":false}));
        }
        if !request.opt_in { return Err(IntakeError::Refused); }
        match request.action.as_str() {
            "feedback-enable" => { self.backend.enable()?; self.enabled = true; Ok(json!({"enabled":true})) }
            "feedback-prepare" if self.enabled => {
                self.prepared = false; self.backend.discard();
                let category = match request.category.as_str() { "Problem" => Category::Bug, "Idea" => Category::Feature, _ => return Err(IntakeError::Refused) };
                self.backend.prepare(category,&request.title,&request.body)?;
                self.prepared = true; Ok(json!({"prepared":true}))
            }
            "feedback-submit" if self.enabled && self.prepared => {
                match self.backend.submit() {
                    Ok(()) => { self.prepared = false; Ok(json!({"sent":true})) }
                    Err(IntakeError::Unknown) => Err(IntakeError::Unknown),
                    Err(error) => { self.prepared = false; self.backend.discard(); Err(error) }
                }
            }
            _ => Err(IntakeError::Refused),
        }
    }
}
static FORM: OnceLock<Mutex<Option<Form<Native>>>> = OnceLock::new();
pub(crate) fn control(value: Value) -> Value {
    let result = (|| {
        let request: Command = serde_json::from_value(value).map_err(|_| IntakeError::Refused)?;
        if request.session.is_nil() { return Err(IntakeError::Refused); }
        let mut slot = FORM.get_or_init(|| Mutex::new(None)).lock().map_err(|_| IntakeError::Refused)?;
        if request.action == "feedback-close" {
            if slot.as_ref().is_some_and(|form| form.id == request.session) { *slot = None; }
            return Ok(json!({}));
        }
        if request.action == "feedback-open" {
            if !request.opt_in || slot.is_some() { return Err(IntakeError::Refused); }
            let mut backend = Native::configured()?;
            let enabled = backend.consent()?;
            *slot = Some(Form { id: request.session, backend, enabled, prepared: false });
            return Ok(json!({"enabled":enabled}));
        }
        slot.as_mut().ok_or(IntakeError::Refused)?.command(&request)
    })();
    match result {
        Ok(body) => json!({"status":200,"body":body}),
        Err(IntakeError::Unknown) => json!({"status":409,"body":{"unknown":true}}),
        Err(_) => json!({"status":403,"body":null}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake { calls: Vec<&'static str>, failure: Option<IntakeError> }
    impl Backend for Fake {
        fn consent(&mut self) -> Result<bool, IntakeError> { Ok(true) }
        fn enable(&mut self) -> Result<(), IntakeError> { self.calls.push("enable"); Ok(()) }
        fn withdraw(&mut self) -> Result<(), IntakeError> { self.calls.push("withdraw"); Ok(()) }
        fn prepare(&mut self, _: Category, _: &str, _: &str) -> Result<(), IntakeError> { self.calls.push("prepare"); Ok(()) }
        fn submit(&mut self) -> Result<(), IntakeError> { self.calls.push("submit"); self.failure.map_or(Ok(()),Err) }
        fn discard(&mut self) { self.calls.push("discard"); }
    }
    fn form() -> Form<Fake> { Form { id:Uuid::new_v4(),backend:Fake::default(),enabled:true,prepared:false } }
    fn command(form: &Form<Fake>, action: &str) -> Command {
        Command { action:action.into(),session:form.id,opt_in:true,category:"Problem".into(),title:"A title".into(),body:"Manual description".into() }
    }
    #[test] fn no_send_without_preview_or_opt_in() {
        let mut f=form(); assert!(f.command(&command(&f,"feedback-submit")).is_err());
        let mut c=command(&f,"feedback-prepare"); c.opt_in=false;
        assert!(f.command(&c).is_err()); assert!(f.backend.calls.is_empty());
    }
    #[test] fn saved_consent_is_separate_from_local_opt_in() {
        let mut f=form(); f.enabled=false;
        assert!(f.command(&command(&f,"feedback-prepare")).is_err());
        f.command(&command(&f,"feedback-enable")).unwrap();
        f.command(&command(&f,"feedback-prepare")).unwrap(); assert!(f.prepared);
    }
    #[test] fn old_form_cannot_act_on_new_form() {
        let mut f=form(); let mut c=command(&f,"feedback-withdraw");c.session=Uuid::new_v4();
        assert!(f.command(&c).is_err()); assert!(f.backend.calls.is_empty());
    }
    #[test] fn uncertain_submission_keeps_prepared_report_for_explicit_retry() {
        let mut f=form(); f.command(&command(&f,"feedback-prepare")).unwrap();
        f.backend.failure=Some(IntakeError::Unknown);
        assert_eq!(f.command(&command(&f,"feedback-submit")),Err(IntakeError::Unknown));
        assert!(f.prepared); assert_eq!(f.backend.calls.iter().filter(|x| **x=="submit").count(),1);
        f.backend.failure=None; f.command(&command(&f,"feedback-submit")).unwrap(); assert!(!f.prepared);
    }
    #[test] fn refusal_discards_pending_report() {
        let mut f=form(); f.prepared=true; f.backend.failure=Some(IntakeError::Refused);
        assert!(f.command(&command(&f,"feedback-submit")).is_err()); assert!(!f.prepared);
        assert_eq!(f.backend.calls,vec!["submit","discard"]);
    }
    #[test] fn edit_and_withdraw_invalidate_confirmation() {
        let mut f=form(); f.prepared=true;
        f.command(&command(&f,"feedback-discard")).unwrap(); assert!(!f.prepared);
        f.prepared=true; let mut c=command(&f,"feedback-withdraw"); c.opt_in=false;
        f.command(&c).unwrap(); assert!(!f.prepared && !f.enabled);
        assert!(f.command(&command(&f,"feedback-submit")).is_err());
    }
    #[test] fn unknown_category_invalidates_old_report() {
        let mut f=form();f.prepared=true;let mut c=command(&f,"feedback-prepare");c.category="other".into();
        assert!(f.command(&c).is_err());assert!(!f.prepared);assert_eq!(f.backend.calls,vec!["discard"]);
    }
    #[test] fn request_cannot_supply_credentials_or_destinations() {
        let value=json!({"action":"feedback-open","session":Uuid::new_v4(),"opt_in":true,"token":"private"});
        assert!(serde_json::from_value::<Command>(value).is_err());
        assert_eq!(control(json!({"action":"feedback-open","session":Uuid::nil(),"opt_in":true}))["status"],403);
    }
}
