//! Types moved out of `iyakki_agent::settingstools_linux` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nLINUX APPS (Store, Setup, Linux apps): linux_sandboxes lists the person's Linux sandboxes and how each one reaches the network (free). linux_network_set changes one sandbox's network and asks first: off, outbound_only (reaches the internet, never this computer or the person's own network; with an allow_list of host:port or address/prefix it reaches only those) or host_shared (uses the computer's own network and can reach its services; only for accounts allowed every host). Accounts that may not reach every host can only use outbound_only with a list of hosts they are allowed. linux_devices_set (asks first) gives one sandbox sound output (playback only) and/or the graphics card's render nodes; both are off unless set, and linux_sandboxes says which are on.";

/// What the account may do: the command-line permission and the hosts it may reach.
#[derive(Clone)]
pub struct Access {
    pub allowed: bool,
    pub net_hosts: Vec<String>,
}
