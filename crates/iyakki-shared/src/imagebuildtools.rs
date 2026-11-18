//! Types moved out of `iyakki_agent::imagebuildtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nIMAGE BUILD: image_build turns a folder of the person's own files (with a Containerfile or Dockerfile) into a container image app by building it in the dev cluster; it asks first, may take minutes and pulls base images from the internet. Say what it needs: a licence for the app, the port it listens on, and an administrator account. Never invent a licence.";
