//! Bounded, resumable metadata-only review of existing local allows.
use crate::local_reads as reads;
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub pending: Option<Value>,
    pub history: Vec<Value>,
}
fn hash(a: &reads::Learned) -> Result<String, &'static str> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(a).map_err(|_| reads::ERROR)?)
    ))
}
pub fn load(dir: &Path) -> Result<Journal, &'static str> {
    let f = match storage::open(
        &dir.join("permission-reviews.json"),
        storage::Open::Read { private: true },
    ) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Journal::default()),
        Err(_) => return Err(reads::ERROR),
    };
    let mut raw = Vec::new();
    f.take(2097153)
        .read_to_end(&mut raw)
        .map_err(|_| reads::ERROR)?;
    if raw.len() > 2097152 {
        return Err(reads::ERROR);
    }
    let j: Journal = serde_json::from_slice(&raw).map_err(|_| reads::ERROR)?;
    if j.history.len() > 128 {
        return Err(reads::ERROR);
    }
    Ok(j)
}
fn save(dir: &Path, j: &Journal) -> Result<(), &'static str> {
    let mut bounded = j.clone();
    let raw = loop {
        let raw = serde_json::to_vec(&bounded).map_err(|_| reads::ERROR)?;
        if raw.len() <= 2097152 {
            break raw;
        }
        if bounded.history.is_empty() {
            return Err(reads::ERROR);
        }
        bounded.history.remove(0);
    };
    let path = dir.join(format!(".dream-{}", uuid::Uuid::new_v4()));
    let mut f = storage::open(&path, storage::Open::New).map_err(|_| reads::ERROR)?;
    f.write_all(&raw)
        .and_then(|_| f.sync_all())
        .map_err(|_| reads::ERROR)?;
    storage::replace(&path, &dir.join("permission-reviews.json")).map_err(|_| reads::ERROR)?;
    storage::sync_dir(dir).map_err(|_| reads::ERROR)
}
pub fn perform(
    dir: &Path,
    identity: &Value,
    project: &str,
    review: impl FnOnce(Value) -> Result<Value, &'static str>,
) -> Result<Value, &'static str> {
    let guard = storage::open(&dir.join("lock"), storage::Open::Lock).map_err(|_| reads::ERROR)?;
    storage::lock(&guard).map_err(|_| reads::ERROR)?;
    let _unlock = storage::UnlockOnDrop(&guard);
    let mut store = reads::load(dir)?;
    let mut journal = load(dir)?;
    if journal
        .pending
        .as_ref()
        .is_some_and(|p| p["identity"] != *identity || p["request"]["project"] != project)
    {
        journal.history.push(json!({"status":"superseded","reason":"Active identity or workspace changed","source_review":journal.pending.as_ref().unwrap()["request"]["request_id"]}));
        journal.pending = None;
    }
    if journal.pending.is_none() {
        let mut records = Vec::new();
        for a in &store.allows {
            if a.identity != *identity || a.project != project {
                continue;
            }
            let key = hash(a)?;
            if journal
                .history
                .iter()
                .any(|h| h["rule_hash"] == key && h["status"] == "applied" && h["kind"] == "retain")
                && a.expires_at > reads::now()
            {
                continue;
            }
            records.push(json!({"rule_hash":key,"policy_revision":a.policy_revision,"action":a.action,"last_use":a.last_use,"expires_at":a.expires_at,"review_pending":a.review_pending}));
            if serde_json::to_vec(&records)
                .map_err(|_| reads::ERROR)?
                .len()
                > 100000
            {
                records.pop();
                break;
            }
            if records.len() == 8 {
                break;
            }
        }
        if records.is_empty() {
            while journal.history.len() > 128 {
                journal.history.remove(0);
            }
            save(dir, &journal)?;
            return Ok(json!({"version":1,"reviewed":0}));
        }
        journal.pending = Some(
            json!({"identity":identity,"request":{"version":1,"request_id":uuid::Uuid::new_v4(),"project":project,"records":records}}),
        );
        while journal.history.len() > 128 {
            journal.history.remove(0);
        }
        save(dir, &journal)?;
    }
    let request = journal.pending.as_ref().ok_or(reads::ERROR)?["request"].clone();
    if serde_json::to_vec(&request)
        .map_err(|_| reads::ERROR)?
        .len()
        > 131072
    {
        return Err(reads::ERROR);
    }
    let result = review(request.clone())?;
    let outcomes = result["outcomes"].as_array().ok_or(reads::ERROR)?;
    let records = request["records"].as_array().ok_or(reads::ERROR)?;
    if result["version"] != 1
        || result["request_id"] != request["request_id"]
        || result["identity"] != *identity
        || result["project"] != project
        || outcomes.len() != records.len()
    {
        return Err(reads::ERROR);
    }
    for (out, record) in outcomes.iter().zip(records) {
        if out["rule_hash"] != record["rule_hash"]
            || out["before"] != record["action"]
            || !matches!(
                out["kind"].as_str(),
                Some("retain" | "narrow" | "expire" | "revoke")
            )
            || !matches!(
                out["status"].as_str(),
                Some("applied" | "superseded" | "failed")
            )
        {
            return Err(reads::ERROR);
        }
        if !out["after"].is_null() && out["after"] != record["action"] {
            return Err(reads::ERROR);
        }
    }
    for out in outcomes {
        let position = store
            .allows
            .iter()
            .position(|a| hash(a).ok().as_deref() == out["rule_hash"].as_str());
        let mut entry = out.clone();
        entry["source_review"] = request["request_id"].clone();
        entry["identity"] = identity.clone();
        entry["project"] = json!(project);
        entry["policy_revision"] = result["policy_revision"].clone();
        if let Some(index) = position {
            if out["status"] == "applied" && out["kind"] != "retain" {
                store.allows.remove(index);
            }
        } else {
            entry["status"] = json!("superseded");
            entry["reason"] = json!("Local rule changed or was removed");
        }
        journal.history.push(entry);
    }
    // Persist removals first. A crash may repeat history application, never restore
    // a removed rule; the same persisted batch ID is retried at the owner.
    reads::save(dir, &store)?;
    journal.pending = None;
    while journal.history.len() > 128 {
        journal.history.remove(0);
    }
    save(dir, &journal)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (std::path::PathBuf, Value) {
        let dir = std::env::temp_dir().join(format!("dream-{}", uuid::Uuid::new_v4()));
        storage::create_dir(&dir).unwrap();
        let identity = json!({"principal":"owner","organization_id":"org"});
        let allow = reads::Learned {
            identity: identity.clone(),
            project: "/project".into(),
            action: json!({"operation":"read","target":"/project/file"}),
            policy_revision: uuid::Uuid::new_v4().to_string(),
            origin: "learned".into(),
            last_use: 1.0,
            expires_at: 9999999999.0,
            review_pending: true,
        };
        reads::save(
            &dir,
            &reads::Store {
                allows: vec![allow],
                requests: vec![],
            },
        )
        .unwrap();
        (dir, identity)
    }
    #[test]
    fn interrupted_review_resumes_same_id_and_removes_once() {
        let (dir, id) = fixture();
        assert!(perform(&dir, &id, "/project", |_| Err("offline")).is_err());
        let pending = load(&dir).unwrap().pending.unwrap();
        perform(&dir,&id,"/project",|request|{
   assert_eq!(request,pending["request"]);
   let r=&request["records"][0];
   Ok(json!({"version":1,"request_id":request["request_id"],"identity":id,"project":"/project","policy_revision":"new","outcomes":[{"rule_hash":r["rule_hash"],"kind":"narrow","status":"applied","reason":"per use","before":r["action"],"after":null}]}))
  }).unwrap();
        assert!(reads::load(&dir).unwrap().allows.is_empty());
        assert!(load(&dir).unwrap().pending.is_none());
        assert_eq!(load(&dir).unwrap().history.len(), 1);
        perform(&dir, &id, "/project", |_| panic!("empty batch dispatched")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn changed_rule_is_not_removed_by_old_result() {
        let (dir, id) = fixture();
        assert!(perform(&dir, &id, "/project", |_| Err("offline")).is_err());
        let mut store = reads::load(&dir).unwrap();
        store.allows[0].last_use = 2.0;
        reads::save(&dir, &store).unwrap();
        perform(&dir,&id,"/project",|request|{let r=&request["records"][0];Ok(json!({"version":1,"request_id":request["request_id"],"identity":id,"project":"/project","outcomes":[{"rule_hash":r["rule_hash"],"kind":"revoke","status":"applied","before":r["action"],"after":null}]}))}).unwrap();
        assert_eq!(reads::load(&dir).unwrap().allows.len(), 1);
        assert_eq!(load(&dir).unwrap().history[0]["status"], "superseded");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
