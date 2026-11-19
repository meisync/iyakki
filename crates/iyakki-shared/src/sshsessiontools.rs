//! Types moved out of `iyakki_agent::sshsessiontools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSSH LOGIN: ssh_connections lists the person's saved SSH hosts (name, user@host:port, key name, whether the host key is trusted). ssh_login opens one of them as an interactive terminal in the CLI tile after the person approves a card; the person types in that session, you cannot type in it, read it or see it. For a command whose output you need, use run_command with ssh instead. You cannot trust a new host key: if ssh_login says it is not trusted, tell the person to press T on the Connections tab of the CLI tile. Passwords are never used (keys only).";
