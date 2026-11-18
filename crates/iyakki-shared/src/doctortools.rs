//! Types moved out of `iyakki_agent::doctortools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nDOCTOR: doctor_report shows what is wrong or worth knowing on this computer (memory, disk, clock, network, sound, camera, firewall, the dev cluster, the vault, the VPN, services that failed to start) in plain sentences; it only reads. Offer the fix button in the DOCTOR tile for anything it lists; never claim something is fixed because the report no longer mentions it until it was run again.";
