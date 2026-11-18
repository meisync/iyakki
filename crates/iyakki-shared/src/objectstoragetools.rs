//! Types moved out of `iyakki_agent::objectstoragetools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nOBJECT STORAGE: object_storage_status shows the S3-compatible stores of Quick install (SeaweedFS, RustFS): whether installed, the in-cluster endpoint, bucket and the NAMES of the vault entries that hold the keys (you never see a key). object_storage_install and object_storage_remove each ask the person. Never ask for or type an access key or secret key in the chat; the person's vault holds them. The endpoint works inside the cluster only.";
