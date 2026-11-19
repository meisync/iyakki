//! Types moved out of `iyakki_agent::sshtunneltools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSSH TUNNELS: ssh_tunnel_list shows the person's tunnels (pages behind an SSH host, reachable on 127.0.0.1). To add, edit, enable, disable or delete one, call the matching ssh_tunnel_* tool: each asks the person every time. Refer to the key by its NAME only; if there is none, ask the person to add it in Settings -> Git and SSH keys. You cannot trust a new host key: show the fingerprint and tell the person to confirm it in Settings -> Network -> SSH tunnels.";
