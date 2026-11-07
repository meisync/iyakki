//! The generic SANDBOX layer for cloud mode (plan: `docs/development/CLOUD-SANDBOX-PLAN.md`).
//!
//! A tenant member, a flow node or an AI agent asks to run an arbitrary command, script or browser session. Nothing runs on the tenant's
//! own pod. The request becomes a [`spec::SandboxSpec`] (an argv array, never a shell string), passes [`admit::admit`] (policy, image,
//! limits, network, approval) and only then exists as an [`admit::Admitted`], the one input every [`Executor`] accepts. Three isolation
//! tiers ([`spec::Tier`]): `wasm` (iyakki-wasm: fuel and memory), `namespace` (in-pod user/mount/pid namespaces, an explicit opt-in), `job`
//! (one hardened Kubernetes Job and NetworkPolicy per run, the cloud default) and, optional, `browser` (a Job with the web engine helper).
//!
//! | file | what |
//! |---|---|
//! | [`spec`] | the request, the result, the vocabulary |
//! | [`policy`] | `sandbox.policy` of the tenant configuration, read into one struct (fail closed) |
//! | [`admit`] | `choose_tier`, `admit`: every refusal with its reason; approval patterns |
//! | [`job`] | pure manifest builders: Job, NetworkPolicy, Secret reference, browser Job (golden tests) |
//! | [`kube`] | the cluster door behind a trait (kubectl for real, a fake in tests) |
//! | [`exec`] | `Executor` and its three implementations |
//! | [`limiter`] | per-person and per-tenant concurrency with a queue (`tenantconfig::fairshare`) |
//! | [`output`] | caps and credential redaction of what a run printed |
//! | [`engine`] | the facade: admit, queue, run, redact, audit |
//! | [`wiring`] | `run_command` in cloud mode and the flows `cli`/`browser` node adapter |

pub mod admit;
pub mod engine;
pub mod exec;
pub mod job;
pub mod kube;
pub mod limiter;
pub mod output;
pub mod policy;
pub mod spec;
pub mod wiring;

#[cfg(test)]
mod tests;
