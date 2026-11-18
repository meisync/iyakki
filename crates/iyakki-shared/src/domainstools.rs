//! Types moved out of `iyakki_agent::domainstools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nDOMAIN NAMES: domains_list shows the person's domain routes (a host name such as store.local mapped to a port or cluster service on this device), the listener's state and the certificates. To add or remove a route or to switch the listener (ports, firewall opening) call domain_route_add, domain_route_remove or domains_listener_set: each asks the person every time. Switching the listener on and opening the firewall to the local network are two separate calls. A host ending in .local is also announced on the local network. Never ask for or send certificate or key text: use a vault certificate NAME or `trial`.";
