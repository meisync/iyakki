//! The tool policy: which calls ask even in Auto mode (F-SPLIT-03, moved out of iyakki-agent so the `Tools` trait and the session grants can leave the agent).

use serde_json::Value;

/// Why a call keeps asking even in Auto mode (`None`: Auto may run it). This is the one place that list lives. Policy
/// refusals (devcluster guard, manifest linter, production read-only, file scope, credguard) are not prompts and apply
/// in every mode; browser consent tiers and the vault form are their own prompts and are not `Risk::Ask`.
pub fn hard_ask(name: &str, args: &Value) -> Option<&'static str> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_lowercase();
    let argv: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_lowercase).collect()).unwrap_or_default();
    // reading the browser's graphics mode is free (the `browser_` prefix below is for site interaction)
    if name == "browser_graphics_get" {
        return None;
    }
    match name {
        "vault_request" | "screen_capture" => return Some("secrets and the screen are never released automatically"),
        "camera_photo" | "camera_record_start" | "camera_passport" => return Some("the camera is private: it always asks (Allow for 5 minutes at most)"),
        "camera_recording_set" if text("mode") != "off" && text("mode") != "" => return Some("turning a camera's recording on always asks, with what is stored, where and for how long on the card"),
        "camera_export" => return Some("exporting recorded pictures (the copies are not encrypted) always asks"),
        "camera_motion_set" if text("mode") != "off" && text("mode") != "" => return Some("turning on motion watching of a camera always asks, with what is analysed, what is stored and that a red sign shows"),
        "disks_unlock" | "disks_lock" => return Some("unlocking and locking encrypted drives always asks; the password is typed on the screen, never given to the assistant"),
        "ssh_tunnel_add" | "ssh_tunnel_edit" | "ssh_tunnel_enable" | "ssh_tunnel_disable" | "ssh_tunnel_delete" => return Some("changing SSH tunnels (who can reach what through this device) always asks"),
        "ssh_login" => return Some("opening a login to another computer always asks, with the connection on the card; the person types in the session, the assistant cannot"),
        "domain_route_add" | "domain_route_remove" | "domains_listener_set" => return Some("changing domain names, the listener and its firewall opening decides who can reach services on this device, so it always asks"),
        "updates_settings_set" => return Some("how often this computer contacts update servers is a privacy choice, so changing it always asks"),
        "updates_bundle_open" => return Some("opening a system update file unpacks software that is meant to be installed on this computer, so it always asks"),
        "updates_bundle_fetch" => return Some("downloading a system update contacts the address given and unpacks software that is meant to be installed on this computer, so it always asks"),
        "updates_check_now" => return Some("looking for updates contacts other computers (the store sources and the system update list), so it always asks"),
        "share_create" | "share_start" | "share_stop" | "share_delete" => return Some("sharing a folder makes files reachable from the network, so changing shares always asks, with who can open it on the card"),
        "browser_graphics_set" => return Some("changing how the browser draws can freeze the computer when it starts, so it always asks"),
        "dictation_wording_set" => return Some("switching on correcting your dictated words with the assistant can send what you say to the chat model, so it always asks"),
        "router_set" | "router_run" | "router_strategy_set" | "router_strategy_add" | "router_strategy_remove" => return Some("the router decides which model sees your work and how much it may spend, so changing it, starting a run, its strategy or a strategy script always asks, and Auto mode never approves it"),
        "fleet_pair" | "fleet_leave" | "fleet_assign" | "fleet_receive" | "fleet_work" => return Some("pairing computers, leaving a group, sending work to another computer and accepting work from one decide who can reach this computer and what runs on it, so they always ask, and Auto mode never approves them"),
        "app_money_limit_set" => return Some("changing how much money an app may spend decides what can be paid for without you, so it always asks, and a spoken yes never approves it"),
        // switching these ON widens who can reach this computer or what a program may do with the account; text found in a page, a file or a
        // message must never be able to do that in Auto mode (switching them off stays easy)
        "devmode_api_scope_set" | "core_contract_switch" | "core_contract_scope_set" | "relay_switch" | "relay_approve_computer" | "ui_socket_set" | "media_server_set" | "connect_client_enable" | "firewall_vpn_local_set" if args.get("on").and_then(Value::as_bool) != Some(false) => return Some("granting a program's access, opening the media server, letting a client connect and counting a VPN as the local network widen who can reach this computer, so they always ask"),
        "trusted_computer" | "patch_grant" if args.get("on").and_then(Value::as_bool) != Some(false) => return Some("letting a paired computer act on this device without asking (every approval answered yes by itself, or patching without asking) is the person's security choice, so switching it on always asks on the device, and Auto mode never approves it"),
        "linuxapp_image" | "image_build" | "linuxapp_pod_start" | "linuxapp_pod_stop" => return Some("building an image, or starting a pod, from files the assistant has not seen runs their commands on this computer, so it always asks, with what goes in on the card"),
        "linux_sandbox_create" | "linux_sandbox_delete" => return Some("making a sandbox downloads from the internet and deleting one removes its files for good, so both always ask"),
        "linux_network_set" => return Some("changing how a Linux sandbox reaches the network always asks, with the new rules on the card"),
        "install_shrink" => return Some("shrinking a partition of another system changes its file system and its partition table and a fault or a power cut can damage it, so it always asks, with the plan on the card, and Auto mode never approves it"),
        "startup_disk_register" | "startup_disk_set" => return Some("this writes the firmware's boot variables (IYAKKI's own entry, the boot order or the one-time next start), which decide what the computer starts, so it always asks"),
        "pod_tls_create" | "pod_tls_rotate" => return Some("the certificates between the domain listener and a pod decide who may talk to that pod, so creating and rotating them always asks"),
        "cloud_mode_set" if text("mode") != "off" => return Some("cloud mode decides how sign-in and the domain listener behave, so turning it on or to automatic always asks"),
        "backup_restore" | "backup_create" => return Some("restoring replaces your data from a file (it can carry skills and settings), and an unencrypted backup copies everything out in plain text, so both always ask, with the file and the areas on the card"),
        "voice_session_settings_set" => return Some("letting a spoken yes approve actions changes who can make this computer act (a TV or a passer-by could say it), so it always asks, and a spoken yes never approves it"),
        "voice_cloud_stream_set" if args.get("on").and_then(Value::as_bool) != Some(false) => return Some("sending your microphone sound to a cloud machine lets your voice leave this device, so it always asks, and a spoken yes never approves it"),
        "voice_assistant_set" => return Some("turning My IYAKKI on or changing its microphone settings decides when this computer listens, so it always asks, and a spoken yes never approves it"),
        "this_computer_set_sharing" | "this_computer_copy_from" | "this_computer_reset" => return Some("changing which settings belong to this computer, copying another computer's settings here, or resetting them always asks"),
        "lemonade_compute_set" => return Some("moving Lemonade between the GPU and the CPU restarts it, so it always asks"),
        "hardware_firmware_install" => return Some("installing hardware firmware needs the person's own acceptance of the vendors' licences, so it always asks, with the files and licences on the card"),
        "assistant_memory_forget" => return Some("forgetting what the assistant remembers of earlier work cannot be undone, so it always asks, with what goes on the card"),
        "history_forget" => return Some("forgetting what the assistant remembers of earlier work cannot be undone, so it always asks, with what goes on the card"),
        "quick_uninstall" => return Some("removing a Quick install item (it can delete data) always asks, with what goes and what stays on the card"),
        "gemini_set" | "gemini_consent_revoke" => return Some("Google Gemini sends audio and text to Google when it is used, so changing it and taking the consent back always ask, with what is sent on the card; the assistant can never give the consent"),
        "gemini_voice_set" => return Some("the Gemini voice assistant (EXPERIMENTAL) sends the open microphone to Google during a session, so turning it on and changing its key and limits always ask, and a spoken yes never approves it; the assistant can never give the consent or type the key"),
        "model_options_set" | "provider_models_fetch" | "models_check" => return Some("changing a model's request options, contacting a provider with the person's key and spending the person's rate limit on test requests always ask"),
        "object_storage_install" | "object_storage_remove" => return Some("installing or removing object storage (it pulls an image, and removing can delete data) always asks"),
        "deploy_team_host" => return Some("publishing a team's model host as a cluster Service follows the team's deploy rules and always asks, with the Service on the card"),
        "ollama_install" => return Some("installing Ollama in the cluster, or moving the host Ollama into it (it pulls an image and copies models), always asks"),
        "rag_rerank_set" => return Some("choosing how found passages are ordered always asks"),
        "rag_sources_add_folder" => return Some("adding a folder to the documents reads many files into the index, so it always asks, with the folder and the file count on the card"),
        "message_send" => return Some("sending a message to another person or service always asks, with the text on the card"),
        "message_mirror_set" => return Some("copying messages from your rooms into the INBOX changes what this computer keeps about you, so it always asks"),
        "shared_vault_provider_set" => return Some("choosing which cryptography keeps the shared vault always asks"),
        "git_clone_repo" | "ide_workspace_add" => return Some("cloning a repository and adding an AI IDE workspace always ask"),
        "bluetooth_forget" => return Some("forgetting a Bluetooth device removes its keys, so it must be paired again: it always asks"),
        "bluetooth_pair" | "bluetooth_pair_answer" => return Some("pairing a Bluetooth device needs the person at the device, so it always asks"),
        "vpn_add" | "vpn_enable" | "vpn_disable" | "vpn_delete" => return Some("changing VPN profiles (which traffic leaves this device, and where) always asks"),
        "store_install" => return Some("installing anything from the store always asks"),
        "store_add_licence" => return Some("adding a licence unlocks a paid app on this device, so it always asks"),
        "store_update" | "store_remove" | "store_rollback" | "store_data_restore" => return Some("updating, removing, rolling back and putting back the data of installed apps always asks"),
        "tile_install_dev" => return Some("adding an unsigned app to the launcher always asks"),
        "app_bump_version" => return Some("changing a version number and the change log always asks"),
        "store_publish" => return Some("publishing to a store shares your work with everyone who uses it, so it always asks"),
        "platform_deploy" | "platform_upgrade" | "platform_remove" => return Some("deploying, upgrading and removing platform services in a cluster always asks"),
        "driver_install" => return Some("changing drivers and firmware changes the whole machine, so it always asks"),
        "k8s_delete" | "k8s_teardown" | "k8s_registry_cleanup" | "calendar_delete" | "alarm_delete" | "study_plan_delete" | "study_plan_calendar_remove" | "quiz_delete" | "remove_cli" | "cluster_stop" => return Some("destructive actions always ask"),
        "flow_publish" | "flow_rollback" | "flow_apply_to_runs" => return Some("publishing a flow, rolling it back and changing running tasks always ask"),
        "radio_trust_certificate" => return Some("trusting a server certificate always asks, with the host and fingerprint on the card"),
        "media_playlist_delete" => return Some("deleting a playlist always asks"),
        "radio_remove" | "radio_forget_certificate" => return Some("removing a station or forgetting a trusted certificate changes the person's own list, so it always asks"),
        "flow_delete" | "flow_drop_version" => return Some("deleting a flow or dropping a version always asks"),
        "webhook_public" => return Some("letting a webhook take requests from outside the local network always asks"),
        "webhook_enable" => return Some("switching a webhook on opens a way for programs on the network to start a flow, so it always asks"),
        "flow_prune_history" => return Some("removing old runs from the history always asks, with the list shown first"),
        "flow_set_heal_policy" if text("mode") != "off" => return Some("giving the agent more say over fixes always asks"),
        "http_request" if text("method") == "delete" => return Some("destructive actions always ask"),
        "job_start" => {
            // the same rules as run_command, applied to every command of the line
            return match args.get("command").and_then(Value::as_str) {
                Some(line) => hard_ask_line(line),
                None => hard_ask_argv(&args.get("argv").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default()),
            };
        }
        _ => {}
    }
    if name.starts_with("mcp__") || name.starts_with("utcp__") || name.starts_with("pkg_") {
        return Some("outside tools always ask");
    }
    if name.contains("vault") || name.contains("credential") || name.contains("secret") || name.starts_with("browser_") {
        return Some("credentials and site interaction always ask");
    }
    if name == "run_command" {
        let prog = text("program");
        let prog = prog.rsplit('/').next().unwrap_or("");
        if matches!(prog, "ssh" | "scp" | "sftp") {
            return Some("connecting to another machine always asks");
        }
        let verbs = ["delete", "rm", "rmi", "prune", "kill", "uninstall", "destroy", "purge", "drain"];
        let destructive = matches!(prog, "rm" | "rmdir" | "shred" | "dd" | "mkfs" | "chmod" | "chown" | "mv" | "sudo" | "su") || argv.iter().any(|a| verbs.contains(&a.as_str()) || a == "--all" || a == "--force" || a == "-rf" || a == "-fr");
        if destructive {
            return Some("destructive commands always ask");
        }
        if argv.iter().any(|a| a.starts_with('/') || a.starts_with('~') || a.split('/').any(|p| p == "..") || a.contains("=/") || a.contains("=~")) {
            return Some("anything outside the workspace always asks");
        }
    }
    None
}

/// Why starting this shell line always asks, even in Auto: the same list `run_command` keeps (destructive programs and verbs,
/// anything outside the workspace), applied to every command of the line (split at `;`, `&`, `|`, newlines and `$(`).
pub fn hard_ask_line(line: &str) -> Option<&'static str> {
    let spaced = line.replace("$(", " ; ").replace('`', " ; ");
    for part in spaced.split([';', '&', '|', '\n', '(', ')']) {
        let words = split_command(part);
        let mut it = words.iter().skip_while(|w| w.contains('=') && !w.starts_with('/') && !w.starts_with('-'));
        let Some(prog) = it.next() else { continue };
        let args: Vec<String> = it.cloned().collect();
        if let Some(why) = hard_ask("run_command", &serde_json::json!({"program": prog, "args": args})) {
            return Some(why);
        }
    }
    None
}

/// The same check for a program with arguments.
pub fn hard_ask_argv(argv: &[String]) -> Option<&'static str> {
    let prog = argv.first()?;
    hard_ask("run_command", &serde_json::json!({"program": prog, "args": argv.iter().skip(1).collect::<Vec<_>>()}))
}

fn split_command(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for c in s.chars() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                q = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}
