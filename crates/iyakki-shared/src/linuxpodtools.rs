//! Types moved out of `iyakki_agent::linuxpodtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nLINUX APP PODS: linuxapp_pods lists the pods of the person's Linux apps in the dev cluster (free). linuxapp_pod_start turns the archived root of a sandbox (a .tar.gz the person made inside it with `tar -C / -czf root.tar.gz ...`, inside their own home) into an image, imports it into the dev cluster and starts a locked-down pod (no token, no privilege); it is for a computer whose kernel cannot run a sandbox itself, administrators only, and it asks every time. linuxapp_pod_stop removes the pod (asks). Tell the person the exec command the answer gives; never invent an archive path.";
