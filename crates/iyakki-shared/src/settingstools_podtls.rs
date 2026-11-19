//! Types moved out of `iyakki_agent::settingstools_podtls` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nPOD TLS (administrators): pod_tls_status (free) shows, for the TLS link between the domain listener and the pod of a route, which secrets are in the tenant vault and how long the certificates last; pod_tls_create makes the certificate authority and the two certificates for a route name; pod_tls_rotate gives new certificates (leaves), a new authority with the old one still trusted (ca) or stops trusting the old one (drop-old). Creating and rotating ask every time; they never show a certificate or a key.";
