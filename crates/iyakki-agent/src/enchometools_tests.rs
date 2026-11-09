//! The encrypted-home chat tools with a fake backend: the free ones ask nothing, every other one asks every time and never rides on a
//! session grant, no tool has a place for a password or a recovery key, and the requests are one word.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::enchometools::*;
use iyakki_enchome::form::Request;

struct Fake {
    guest: bool,
    requests: Mutex<Vec<Request>>,
    audits: Mutex<Vec<String>>,
    locked: Mutex<u32>,
}

impl Backend for Fake {
    fn can_encrypt(&self) -> bool {
        !self.guest
    }
    fn status(&self) -> String {
        "status text".into()
    }
    fn plan(&self) -> Result<String, String> {
        Ok("plan: 3 files".into())
    }
    fn lock(&self) -> Result<String, String> {
        *self.locked.lock().unwrap() += 1;
        Ok("locked".into())
    }
    fn request(&self, r: Request) -> Result<(), String> {
        self.requests.lock().unwrap().push(r);
        Ok(())
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audits.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(guest: bool) -> (EncHomeTools, Arc<Fake>) {
    let f = Arc::new(Fake { guest, requests: Mutex::new(vec![]), audits: Mutex::new(vec![]), locked: Mutex::new(0) });
    (EncHomeTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn status_and_plan_are_free_and_everything_else_asks_every_time() {
    let (t, _) = rig(false);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(STATUS), Some(Risk::Safe));
    assert_eq!(risk(PLAN), Some(Risk::Safe));
    for n in [CREATE, LOCK, UNLOCK, RECOVERY, ADMIN_SLOT, MOVE] {
        assert_eq!(risk(n), Some(Risk::Ask), "{n}");
        assert!(t.grant_blocker(n, &json!({"enable": true})).is_some(), "{n}: no session grant");
        assert!(!t.approved_always(n), "{n}");
    }
    for n in [STATUS, PLAN] {
        assert!(t.grant_blocker(n, &json!({})).is_none() && t.approved_always(n));
    }
}

#[test]
fn no_tool_can_carry_a_password_or_a_recovery_key() {
    let (t, _) = rig(false);
    for d in t.defs() {
        let props = d.spec.schema.get("properties").and_then(|p| p.as_object()).cloned().unwrap_or_default();
        for k in props.keys() {
            let k = k.to_lowercase();
            assert!(!(k.contains("pass") || k.contains("key") || k.contains("secret") || k.contains("recovery")), "{}: argument {k}", d.spec.name);
        }
    }
    assert!(PROMPT_RULE.contains("never receive"));
}

#[test]
fn the_form_tools_leave_a_one_word_request_and_say_where_to_type() {
    let (t, f) = rig(false);
    assert!(t.call(CREATE, &json!({})).unwrap().contains("you never see it"));
    t.call(UNLOCK, &json!({})).unwrap();
    t.call(RECOVERY, &json!({})).unwrap();
    t.call(MOVE, &json!({})).unwrap();
    t.call(ADMIN_SLOT, &json!({"enable": true})).unwrap();
    t.call(ADMIN_SLOT, &json!({"enable": false})).unwrap();
    assert_eq!(*f.requests.lock().unwrap(), vec![Request::Create, Request::Unlock, Request::ChangeRecovery, Request::Migrate, Request::AdminSlotOn, Request::AdminSlotOff]);
    assert!(f.audits.lock().unwrap().iter().all(|a| a.contains("form opened")), "audit lines carry no secret");
    assert!(t.call(ADMIN_SLOT, &json!({})).is_err());
    assert!(t.refusal(ADMIN_SLOT, &json!({})).is_some());
    assert!(t.preview(ADMIN_SLOT, &json!({"enable": true})).unwrap().contains("READ your home"));
    assert!(t.preview(CREATE, &json!({})).unwrap().contains("does not protect a signed-in"));
    assert!(t.preview(MOVE, &json!({})).unwrap().contains("plan: 3 files"));
}

#[test]
fn lock_closes_directly_and_status_and_plan_read() {
    let (t, f) = rig(false);
    assert_eq!(t.call(LOCK, &json!({})).unwrap(), "locked");
    assert_eq!(*f.locked.lock().unwrap(), 1);
    assert_eq!(t.call(STATUS, &json!({})).unwrap(), "status text");
    assert_eq!(t.call(PLAN, &json!({})).unwrap(), "plan: 3 files");
    assert!(t.describe(ADMIN_SLOT, &json!({"enable": true})).contains("administrator password could then open"));
}

#[test]
fn a_guest_has_no_password_so_nothing_is_requested() {
    let (t, f) = rig(true);
    for n in [CREATE, UNLOCK, RECOVERY, MOVE, LOCK, PLAN] {
        assert!(t.call(n, &json!({})).is_err(), "{n}");
    }
    assert!(t.call(ADMIN_SLOT, &json!({"enable": true})).is_err());
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(t.call(STATUS, &json!({})).is_ok());
}

#[test]
fn the_status_words_name_the_dangers_and_what_is_not_protected() {
    use iyakki_enchome::{Meta, SlotInfo, State, Status};
    let st = |state, meta: Meta, slots: Vec<SlotInfo>| Status { state, damaged_reason: None, container: "/x".into(), size_bytes: None, slots, meta, one_copy_bad: false, plain_copy: None };
    assert!(describe_status(&st(State::NotSetUp, Meta::default(), vec![]), false).contains("not encrypted"));
    let open = describe_status(&st(State::Open, Meta { sealed_by_reset: true, ..Meta::default() }, vec![SlotInfo::Password, SlotInfo::Recovery]), true);
    assert!(open.contains("OPEN") && open.contains("not confirmed") && open.contains("reset by an administrator") && open.contains("no administrator slot"), "{open}");
    let locked = describe_status(&st(State::Locked, Meta { recovery_confirmed: true, ..Meta::default() }, vec![SlotInfo::Password, SlotInfo::Recovery, SlotInfo::Admin]), false);
    assert!(locked.contains("LOCKED") && locked.contains("administrator slot") && !locked.contains("WARNING"), "{locked}");
    let (t, _) = rig(false);
    let _ = t;
}
