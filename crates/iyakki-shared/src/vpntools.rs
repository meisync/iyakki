//! Types moved out of `iyakki_agent::vpntools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nVPN: vpn_list shows the person's VPN profiles and their state. To add (import a profile FILE by its path), enable, disable or delete one, call the matching vpn_* tool: each asks the person every time. Never ask for or send key text; the person's own WireGuard or OpenVPN file is imported by path and its keys go to their vault. Mode, filters and the kill switch are set by the person in Settings -> Network -> VPN. IYAKKI provides no VPN service and no server list.";
