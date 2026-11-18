//! Types moved out of `iyakki_agent::sftptools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSFTP SERVER: sftp_status shows whether the SFTP server is on, its port, who may log in, the keys (fingerprints) and the host key fingerprint. sftp_set changes it and asks the person on a card every time: action enable or disable; allow_user or deny_user (an account name); add_key (a PUBLIC key line `ssh-ed25519 AAAA…`, never a private key) or remove_key (a fingerprint); set_port; set_expiry (1 hour, 1 day, 7 days, never); password_login (on or off). It is off by default, only the local network can reach it, and each account sees only what its own file rules allow. Never ask for or type a password or private key in the chat.";
