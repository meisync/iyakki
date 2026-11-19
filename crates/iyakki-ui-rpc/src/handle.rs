//! The UI's view of the core's services over the socket (F-SPLIT-03 piece 6). Today it is the set of typed accessors the first screens need,
//! each a remote object of one core root; it does not yet implement `ServicesHandle` itself, because the shell still holds the 14 member stores
//! as concrete types (`&Inbox`, `&CalendarStore`, ...) and must move to the traits of `member_stores` first (UI-RPC-DESIGN.md section 1, point 2).
//! `is_real` is true only while the core answers a call, so a lost core is visible and a fake is never taken for it (KI-1010).

use std::sync::Arc;

use iyakki_agent_api::{AuditApi, ChatRuns, InboxApi, UsageLogApi};
use serde_json::{json, Value};

use crate::{RemoteAny, Rpc};

/// The services of the core as one UI program sees them.
#[derive(Clone)]
pub struct RemoteServicesHandle {
    rpc: Rpc,
}

macro_rules! root {
    ($(#[$m:meta])* $name:ident, $variant:ident, $tr:ty) => {
        $(#[$m])*
        pub fn $name(&self) -> Arc<$tr> {
            match self.rpc.root(stringify!($variant)) {
                Some(RemoteAny::$variant(r)) => r,
                _ => unreachable!(concat!(stringify!($variant), " is generated")),
            }
        }
    };
}

impl RemoteServicesHandle {
    pub fn new(rpc: Rpc) -> RemoteServicesHandle {
        RemoteServicesHandle { rpc }
    }

    /// The connection every remote object shares.
    pub fn rpc(&self) -> &Rpc {
        &self.rpc
    }

    /// True only while the core answers: one cheap read on a root it must serve. A closed socket, a core without the `rpc` switch or a
    /// core of another build gives false, and nothing else in this handle makes it true.
    pub fn is_real(&self) -> bool {
        self.rpc.call::<Value>("InboxApi", 0, "list", json!([""])).is_ok()
    }

    root!(/// The core's notification inbox.
        inbox, InboxApi, dyn InboxApi);
    root!(/// The core's usage log.
        usage, UsageLogApi, dyn UsageLogApi);
    root!(/// The audit log (when the core serves it).
        audit, AuditApi, dyn AuditApi);
    root!(/// Chat runs: start, poll events, answer approvals.
        chat_runs, ChatRuns, dyn ChatRuns);
}
