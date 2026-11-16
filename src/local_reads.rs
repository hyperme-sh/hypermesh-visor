//! Scoped personal read metadata and user-owned, no-follow file access.
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs::File, io::{Read, Write}, path::Path};
use uuid::Uuid;
use base64::Engine;

pub const ERROR: &str = "Local read unavailable or refused";
pub fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0) }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Learned {
    pub identity: Value,
    pub project: String,
    pub action: Value,
    pub policy_revision: String,
    pub origin: String,
    pub last_use: f64,
    pub expires_at: f64,
    pub review_pending: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store { pub allows: Vec<Learned>, pub requests: Vec<Uuid> }
pub fn load(dir: &Path) -> Result<Store, &'static str> {
    storage::check_dir(dir).map_err(|_|ERROR)?;
    let file=match storage::open(&dir.join("learned-reads.json"),storage::Open::Read{private:true}) {
        Ok(f)=>f, Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(Store::default()),Err(_)=>return Err(ERROR)
    };
    let mut bytes=Vec::new();file.take(524289).read_to_end(&mut bytes).map_err(|_|ERROR)?;
    if bytes.len()>524288 {return Err(ERROR)}
    let s:Store=serde_json::from_slice(&bytes).map_err(|_|ERROR)?;
    if s.allows.len()>256 || s.requests.len()>1024 {return Err(ERROR)}
    Ok(s)
}
pub fn save(dir: &Path, store: &Store) -> Result<(), &'static str> {
    let bytes=serde_json::to_vec(store).map_err(|_|ERROR)?;
    if bytes.len()>524288 || store.allows.len()>256 || store.requests.len()>1024 {return Err(ERROR)}
    let temp=dir.join(format!(".read-{}",Uuid::new_v4()));
    let result=(||{
        let mut f=storage::open(&temp,storage::Open::New).map_err(|_|ERROR)?;
        f.write_all(&bytes).and_then(|_|f.sync_all()).map_err(|_|ERROR)?;
        storage::replace(&temp,&dir.join("learned-reads.json")).map_err(|_|ERROR)?;
        storage::sync_dir(dir).map_err(|_|ERROR)
    })();
    if result.is_err(){let _=std::fs::remove_file(temp);}result
}

#[cfg(unix)]
fn fingerprint(file: &File) -> Result<Value, &'static str> {
    use std::os::unix::fs::MetadataExt;
    let m=file.metadata().map_err(|_|ERROR)?;
    if !m.is_file(){return Err(ERROR)}
    Ok(json!([m.dev(),m.ino(),m.mode(),m.uid(),m.gid(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()]))
}
#[cfg(unix)]
pub struct Opened { file: File, fingerprint: Value, project:String, target:String }
#[cfg(unix)]
impl Opened {
    pub fn open(project:&str,target:&str)->Result<Self,&'static str>{
        use std::{ffi::CString, os::fd::{AsRawFd, FromRawFd}};
        if project=="/" || !project.starts_with('/') || !target.starts_with(&format!("{project}/")) || [project,target].iter().any(|p|p.chars().any(char::is_control)||p.contains(['*','?','\\'])||p.split('/').skip(1).any(|c|c.is_empty()||c=="."||c=="..")){return Err(ERROR)}
        let mut dir=File::open("/").map_err(|_|ERROR)?;
        let parts:Vec<_>=target.split('/').skip(1).collect();
        for (i,part) in parts.iter().enumerate(){
            let name=CString::new(*part).map_err(|_|ERROR)?;
            let flags=libc::O_RDONLY|libc::O_CLOEXEC|libc::O_NOFOLLOW|libc::O_NONBLOCK|if i+1<parts.len(){libc::O_DIRECTORY}else{0};
            // Each component is relative to an already-open directory; links
            // are refused for the project ancestors and the final object.
            let fd=unsafe{libc::openat(dir.as_raw_fd(),name.as_ptr(),flags)};
            if fd<0{return Err(ERROR)}
            dir=unsafe{File::from_raw_fd(fd)};
        }
        let mark=fingerprint(&dir)?;
        Ok(Self{file:dir,fingerprint:mark,project:project.into(),target:target.into()})
    }
    pub fn action(&self,bytes:u64,seconds:u32)->Value{
        json!({"operation":"read","target":self.target,"arguments_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&self.fingerprint).unwrap())),"limits":{"max_bytes":bytes,"timeout_seconds":seconds}})
    }
    pub fn read(mut self,max:u64,seconds:u32,valid:impl Fn()->bool)->Result<Vec<u8>,&'static str>{
        let started=std::time::Instant::now();let mut out=Vec::new();let mut chunk=[0;4096];
        if self.file.metadata().map_err(|_|ERROR)?.len()>max{return Err(ERROR)}
        loop{
            if !valid() || started.elapsed().as_secs_f64()>=seconds as f64{return Err(ERROR)}
            let n=self.file.read(&mut chunk).map_err(|_|ERROR)?;
            if n==0{break}out.extend_from_slice(&chunk[..n]);if out.len() as u64>max{return Err(ERROR)}
        }
        let current=Self::open(&self.project,&self.target)?;
        if !valid() || started.elapsed().as_secs_f64()>=seconds as f64 || fingerprint(&self.file)?!=self.fingerprint || current.fingerprint!=self.fingerprint{return Err(ERROR)}
        Ok(out)
    }
}

pub fn verify(result:&Value, request:&Value, identity:&Value)->Result<(), &'static str>{
    if result["version"]!=1 || result["preview_only"]!=false || result["request_id"]!=request["request_id"] || result["project"]!=request["project"] || result["action"]!=request["action"] || result["limits"]!=request["action"]["limits"] || result["permitted"]!=true || !matches!(result["decision"].as_str(),Some("allowed"|"advisory")) {return Err(ERROR)}
    for key in ["tenant_id","owner_id","principal","organization_id","device_id","bot_id","assignment_id","session_id"] {if identity[key].as_str().is_none() || result["identity"][key]!=identity[key]{return Err(ERROR)}}
    let expiry=result["expires_at"].as_f64().ok_or(ERROR)?;
    if expiry<=now() || expiry>now()+11.0 || Uuid::parse_str(result["policy_revision"].as_str().ok_or(ERROR)?).map_err(|_|ERROR)?.is_nil(){return Err(ERROR)}
    if !request["expected_policy_revision"].is_null() && result["policy_revision"]!=request["expected_policy_revision"]{return Err(ERROR)}
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub selection: crate::permission_templates::Selection,
    pub target: String,
    pub max_bytes: u64,
    pub timeout_seconds: u32,
}
#[cfg(unix)]
pub fn perform(dir:&Path,body:ReadRequest,identity:&Value,workspace:&str,mut reviewer:impl FnMut(Value)->Result<Value,&'static str>,valid:impl Fn()->bool)->Result<Value,&'static str>{

    body.selection.validate()?;
    if body.request_id.is_nil()||body.session_id.is_nil()||body.max_bytes==0||body.max_bytes>1048576||body.timeout_seconds==0||body.timeout_seconds>30||!valid(){return Err(ERROR)}
    // The same lock as template edits prevents a saved preference from changing
    // midway through dispatch. The bounded private store serializes retries.
    let guard=storage::open(&dir.join("lock"),storage::Open::Lock).map_err(|_|ERROR)?;
    storage::lock(&guard).map_err(|_|ERROR)?;let _unlock=storage::UnlockOnDrop(&guard);
    if !crate::permission_templates::load(dir)?.selections.contains(&body.selection){return Err(ERROR)}
    let mut store=load(dir)?;
    if store.requests.contains(&body.request_id)||store.requests.len()>=1024{return Err(ERROR)}
    if workspace!=body.selection.scope.project || identity["session_id"]!=body.session_id.to_string() || identity["organization_id"]!=body.selection.scope.organization.to_string() || identity["device_id"]!=body.selection.scope.device.to_string() || !valid(){return Err(ERROR)}
    let file=Opened::open(&body.selection.scope.project,&body.target)?;
    let action=file.action(body.max_bytes,body.timeout_seconds);
    let previous=store.allows.iter().find(|a|a.identity==*identity&&a.project==body.selection.scope.project&&a.action==action&&a.expires_at>now()).cloned();
    let rules:Vec<Value>=body.selection.rules.iter().map(|r|json!({"operation":r.operation,"target":r.target,"limits":{"max_bytes":r.max_bytes,"timeout_seconds":r.timeout_seconds}})).collect();
    let mut request=json!({"request_id":body.request_id,"project":body.selection.scope.project,"template_version":1,"template":rules,"action":action});
    if let Some(ref prior)=previous{request["expected_policy_revision"]=json!(prior.policy_revision);}
    let review=reviewer(request.clone())?;
    verify(&review,&request,identity)?;
    if !valid(){return Err(ERROR)}
    // Consume the ID before OS reading. A disconnected or restarted caller may
    // start a new request but cannot replay this ID or retrieve cached contents.
    store.requests.push(body.request_id);save(dir,&store)?;
    let expiry=review["expires_at"].as_f64().ok_or(ERROR)?;
    let bytes=file.read(body.max_bytes,body.timeout_seconds,||valid()&&now()<expiry)?;
    request["expected_policy_revision"]=review["policy_revision"].clone();
    let final_review=reviewer(request.clone())?;
    verify(&final_review,&request,identity)?;
    if !valid(){return Err(ERROR)}
    let learn=final_review["learning_eligible"]==true;
    let reused=previous.is_some()&&learn;
    store.allows.retain(|a|!(a.identity==*identity&&a.project==body.selection.scope.project&&a.action==action));
    if learn {
        store.allows.retain(|a|a.expires_at>now());
        if store.allows.len()>=256{return Err(ERROR)}
        store.allows.push(Learned{identity:identity.clone(),project:body.selection.scope.project,action,
            policy_revision:final_review["policy_revision"].as_str().ok_or(ERROR)?.into(),origin:"conductor_aiuditor_verified".into(),last_use:now(),expires_at:now()+86400.0,review_pending:final_review["decision"]=="advisory"||previous.is_some_and(|a|a.review_pending)});
    }
    save(dir,&store)?;
    if !valid() || now()>=final_review["expires_at"].as_f64().ok_or(ERROR)?{return Err(ERROR)}
    Ok(json!({"request_id":body.request_id,"content_base64":base64::engine::general_purpose::STANDARD.encode(bytes),"learned":learn,"reused":reused,"review_pending":final_review["decision"]=="advisory"}))
}

#[cfg(all(test,unix))]
mod tests {
    use super::*;
    use crate::permission_templates::{Preferences,Selection,Scope,Preset};
    struct Fixture { root:std::path::PathBuf, dir:std::path::PathBuf, project:String,target:String,selection:Selection,identity:Value,revision:Uuid }
    impl Fixture {
        fn new()->Self{
            let root=std::env::temp_dir().join(format!("visor-read-{}",Uuid::new_v4()));
            storage::create_dir(&root).unwrap();let dir=root.join("store");storage::create_dir(&dir).unwrap();
            let project=root.join("project");std::fs::create_dir(&project).unwrap();let target=project.join("file");std::fs::write(&target,b"private file text").unwrap();
            let mut identity=json!({"principal":"owner"});for k in ["tenant_id","owner_id","organization_id","device_id","bot_id","assignment_id","session_id"]{identity[k]=json!(Uuid::new_v4());}
            let selection=Selection::new(Scope{organization:Uuid::parse_str(identity["organization_id"].as_str().unwrap()).unwrap(),device:Uuid::parse_str(identity["device_id"].as_str().unwrap()).unwrap(),project:project.to_str().unwrap().into()},Preset::LocalInspection,target.to_str().unwrap().into()).unwrap();
            let mut prefs=Preferences::empty();prefs.select(selection.clone()).unwrap();crate::permission_templates::save(&dir,&Preferences::empty(),&prefs).unwrap();
            Self{root,dir,project:project.to_str().unwrap().into(),target:target.to_str().unwrap().into(),selection,identity,revision:Uuid::new_v4()}
        }
        fn body(&self,id:Uuid)->ReadRequest{ReadRequest{request_id:id,session_id:Uuid::parse_str(self.identity["session_id"].as_str().unwrap()).unwrap(),selection:self.selection.clone(),target:self.target.clone(),max_bytes:1024,timeout_seconds:10}}
        fn review(&self,request:&Value)->Value{json!({"version":1,"request_id":request["request_id"],"project":request["project"],"action":request["action"],"limits":request["action"]["limits"],"preview_only":false,"identity":self.identity,"permitted":true,"decision":"advisory","learning_eligible":true,"policy_revision":self.revision,"expires_at":now()+9.0})}
        fn run(&self,id:Uuid)->Result<Value,&'static str>{perform(&self.dir,self.body(id),&self.identity,&self.project,|r|Ok(self.review(&r)),||true)}
    }
    impl Drop for Fixture{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.root);}}
    #[test]
    fn first_read_learns_restart_reuses_and_revision_change_refuses(){
        let mut f=Fixture::new();let first=Uuid::new_v4();let result=f.run(first).unwrap();assert_eq!(result["learned"],true);assert_eq!(result["reused"],false);
        assert!(f.run(first).is_err());
        // Each call reloads the private store as a restarted process would.
        assert_eq!(f.run(Uuid::new_v4()).unwrap()["reused"],true);
        let stored=load(&f.dir).unwrap();assert_eq!(stored.allows.len(),1);assert!(stored.allows[0].review_pending);
        let raw=std::fs::read_to_string(f.dir.join("learned-reads.json")).unwrap();assert!(!raw.contains("private file text"));assert!(!raw.contains("content_base64"));
        f.revision=Uuid::new_v4();assert!(f.run(Uuid::new_v4()).is_err());
    }
    #[test]
    fn owner_policy_revocation_and_missing_verification_cannot_read_or_learn(){
        let f=Fixture::new();
        assert!(perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|_|Err(ERROR),||true).is_err());
        assert!(load(&f.dir).unwrap().allows.is_empty());
        f.run(Uuid::new_v4()).unwrap();
        for field in ["permitted","preview_only"]{
            assert!(perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|r|{let mut v=f.review(&r);v[field]=json!(field=="preview_only");Ok(v)},||true).is_err());
        }
        let result=perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|r|{let mut v=f.review(&r);v["learning_eligible"]=json!(false);Ok(v)},||true).unwrap();
        assert_eq!(result["reused"],false);assert!(load(&f.dir).unwrap().allows.is_empty());
    }
    #[test]
    fn links_escape_replacement_changed_arguments_and_cancel_refuse(){
        use std::os::unix::fs::symlink;
        let f=Fixture::new();let opened=Opened::open(&f.project,&f.target).unwrap();
        std::fs::remove_file(&f.target).unwrap();std::fs::write(&f.target,b"replacement").unwrap();
        assert!(opened.read(1024,10,||true).is_err());
        assert!(Opened::open(&f.project,&format!("{}/../file",f.project)).is_err());
        let link=format!("{}/link",f.project);symlink(&f.target,&link).unwrap();assert!(Opened::open(&f.project,&link).is_err());
        assert!(perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|r|{let mut v=f.review(&r);v["action"]["arguments_sha256"]=json!("0".repeat(64));Ok(v)},||true).is_err());
        assert!(perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|r|Ok(f.review(&r)),||false).is_err());
        assert!(load(&f.dir).unwrap().allows.is_empty());
    }
    #[test]
    fn cross_session_and_organization_verification_refused(){
        let f=Fixture::new();for key in ["session_id","organization_id"]{
            assert!(perform(&f.dir,f.body(Uuid::new_v4()),&f.identity,&f.project,|r|{let mut v=f.review(&r);v["identity"][key]=json!(Uuid::new_v4());Ok(v)},||true).is_err());
        }
    }
}
