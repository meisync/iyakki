//! The forward-looking core contract (docs/development/CORE-CONTRACT.md): the cross-cutting types every domain shares and the
//! first three domains (settings, vault, flows). Nothing here does IO and no tile uses it yet; it is the shape a cloud or web UI
//! can rely on, over any of the three transports, against a machine of a different version.
//!
//! Rules enforced by the types: a remote caller never answers a consent prompt ([`consent::LocalSeat`]), no vault call returns a
//! secret value ([`vault::Vault`]), every error has a stable code ([`error::ErrorCode`]), every event has a sequence number
//! ([`event::EventLog`]).

pub mod catalog;
pub mod consent;
pub mod context;
pub mod error;
pub mod event;
pub mod flows;
pub mod hello;
pub mod settings;
pub mod stream;
pub mod vault;

pub use consent::{ConsentAnswer, ConsentKind, ConsentRequest, Consents, LocalSeat};
pub use context::{guard, CallContext, Risk, Scope, Transport};
pub use error::{CallError, CallResult, ErrorCode, IdempotencyKey, RateLimit, RateLimiter};
pub use event::{Event, EventBatch, EventLog, Resume};
pub use flows::{Approval, FlowEvent, FlowInfo, Flows, RunId, RunState, RunStatus};
pub use hello::{negotiate, ClientHello, ContractVersion, DomainSupport, Hello, Negotiated, CONTRACT_VERSION};
pub use settings::{SettingChange, SettingEvent, SettingKind, SettingSpec, SettingValue, Settings};
pub use stream::{Chunk, StreamHandle, StreamId, StreamKind, Streams};
pub use vault::{ActionOutcome, NamedAction, Vault, VaultStatus};
