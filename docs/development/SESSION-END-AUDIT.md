# What ends when a person signs out (audit for D-SESSION-END)

Rule (docs/development/ALPHA-8-DECISIONS.md, D-SESSION-END): sign-out ends the person's interactive session resources; a change of the sound owner ends the previous person's sound; a lock stops capture only; things the person set up to run on their own continue and say so.

Registry: `iyakki_shared::sessionend` (one process-wide registry plus the shell's own for what only the shell holds). `Shell::sign_out` calls it once, before the screen changes, and audits `session.end <name>` per stop and `session.end.count`. Hooks: `crates/iyakki-agent/src/sessionend_hooks.rs`, `crates/iyakki-shell/src/soundowner.rs` (client sound), `crates/iyakki-shell/src/shell_wake.rs` (wake listener). Chat: `session_end_report`. Ratchet: `crates/iyakki-core/tests/session_end_registered.rs` with `docs/development/session-end-resources.txt`.

Reasons a hook can run for (`Why`): sign-out (every hook), sound switch (the `Sound` hooks of the previous person), lock (the `Capture` hooks), suspend (the `Capture` and `Sound` hooks, before the machine sleeps: `Shell::sleep_now`, the lid path, and `boot::sleep` for every other way into a sleep). Kinds: Sound, Capture, Process (a session process: ends on sign-out only; hooks that end sessions of a person report how many and the audit line carries the number, for example `session.end terminals 2`).

Rules in the last column: STOP (ends at sign-out), CONTINUE (on purpose), LOCK-ONLY (ends at lock too), SUSPEND (ends before the machine sleeps).

| Resource | Owner file | Before this lane | Now | Rule |
|---|---|---|---|---|
| Internet radio (one machine-wide player, retry and HLS threads) | agent/radio.rs, radio_recover.rs, hls.rs | kept playing (the owner's report) | stopped by hook `radio` when the person owned the sound | STOP |
| Media player and queue | agent/media.rs | kept | hook `media-player` stops the person's track and drops the queue | STOP |
| Radio-backed builtin audio of the media player | agent/media.rs (uses RadioPlayer) | kept | stopped with `radio` | STOP |
| Story and Learn readers (text-to-speech threads) | agent/storykit.rs (readers made by apps) | kept: the reading threads hold their own handle, dropping the app did not end them | hook `story-readers` (ReaderGroup) | STOP |
| Client session sound routes | shared/clientmedia.rs, shell/soundowner.rs | owner list only | hook `client-sound` closes the person's routes | STOP |
| Wake listening (microphone) | agent/wakelisten.rs, shell/shell_wake.rs | stopped on the next heartbeat | hook `wake-listener` closes it at once | STOP; a lock pauses it (existing) |
| Assistant camera grant and recording | shared/camtools.rs | kept (5-minute grant) | hook `camera-grant` revokes the grant and stops a recording | STOP, LOCK-ONLY |
| Assistant screen capture grant | shared/screencap.rs | kept | hook `screen-grant` | STOP, LOCK-ONLY |
| CAMERA app capture (V4L2) | apps/camera, apps/camera/v4l2.rs, camera/mod.rs (`guarded`) | stops on Drop of the app | hook `camera-device` closes the device from the hook's thread (the tile sees "the camera was switched off") | STOP, LOCK-ONLY, SUSPEND |
| Voice recorder, dictation, voice desk recorders, SDK listen capture (the microphone process `voice::Recorder`) | agent/voice.rs, voicedesk.rs, store/sdkv2_listen_real.rs | stop on Drop of the tile | hook `voice-recorders` kills the recording process; `is_alive` says no | STOP, LOCK-ONLY, SUSPEND |
| Voice players (speech, reading, voice desk: `voice::Player`) | agent/voice.rs, voicedesk.rs | stop on Drop of the tile | hook `voice-players` (the sound owner's) | STOP, SUSPEND |
| Voice assistant speaker thread | agent/voiceassist_ports.rs | thread ended with the tile, not proven | hook `voice-assistant-speaker` (the sound owner's): the sentence is cut while it plays | STOP, SUSPEND |
| Cloud audio stream listener | agent/cloudvoice.rs (`CloudListener::end`), services.rs | open | hook `cloud-audio-listener` closes the port and the thread; a later offer starts a new listener with a new certificate | STOP, LOCK-ONLY, SUSPEND |
| Cloud session's mouth (answer sent to the device) | agent/cloudvoice.rs (`CloudSpeaker`) | open | hook `cloud-speaker` (the sound owner's) | STOP, SUSPEND |
| Cloud answer sound on the device (`AnswerPlayer`) | audiolink/device.rs | open | no hook: a buffer of received frames with no thread and no device; its program (iyakki-connect) runs on the person's own computer, outside this machine's sessions | not per person |
| Browser session camera and microphone (web client) | apps/camera/browsercam.rs, shared/browsersource.rs, browsermedia.rs (`stop_capture_of`) | ended with the browser session (unknown on sign-out) | hook `browser-capture` switches them off and tells the page to stop its devices; a suspend reaches every page | STOP, LOCK-ONLY, SUSPEND |
| Video playback (MEDIA app) | agent/player.rs | decoders stop on Drop | unchanged | STOP |
| Camera recordings the person scheduled | agent/camrec.rs | continue | continue | CONTINUE |
| Scheduled flows and alarms | flows | stopped by `stop_for_logout` | unchanged | existing rule |
| SSH tunnels, VPN | services | stopped by their own logout lines | unchanged | STOP |
| Embedded terminals (command-line sessions) and full terminals (ptys, pod terminals) of the SDK terminal facet; the person's sessions, the count is in the audit | agent/terminalfacet.rs (`Terminals::end_session`), terminalpty.rs (`PtyBook::close_user`) | open until the tile was dropped | hook `terminals` (counted) closes the person's sessions: a live shell is hung up, a question still waiting is dropped | STOP |
| The TERMINAL tile's own shell | apps/terminal | stops on Drop of the tile | unchanged (drop-stops) | STOP |
| K8s port-forwards | core-local/portforward.rs (the one list of the process) | kept | hook `port-forwards` (counted) stops every forward (one person at a time on this machine) | STOP |
| Sandboxed Linux windows | shared/linuxwin.rs (`WINDOW_STOPS`, `Window::enter`) | kept while the tile was alive | hook `linux-windows` (counted) kills the sandbox program; the tile sees it ended | STOP |
| IDE task runs (jobs with an IDE workspace) | agent/jobs.rs (`end_session`), jobtools.rs | survived the session | hook `ide-task-runs` (counted) stops the person's running jobs unless marked to continue | STOP unless marked |
| Lab and chat jobs (jobs with no workspace) | agent/jobs.rs, jobtools.rs | survived the session | hook `lab-jobs` (counted), same rule | STOP unless marked |
| A job the person marked to continue | jobs.rs (`set_keep`), chat tools `job_start` (`keep_after_sign_out`) and `job_keep` (both ask) | n/a | marked in the job's own file; a sign-out leaves it running | CONTINUE |
| Install and download jobs of the system (workspace `iyakki-install`: Linux windows base, firmware, store installs) | agent/quickinstall.rs | continue | unchanged: not the person's sound or capture, and a half installed base is worse than a finished one | CONTINUE |
| Media downloads (`mediadl`), including yt-dlp | agent/mediadl.rs, ytdlp.rs | continue | unchanged: not sound; a half-finished download resumes by hand | CONTINUE |
| Live previews | camera preview, flow-run preview pixmap, Settings look preview | camera preview was the CAMERA tile | the camera preview is the `camera-device` row above; the flow-run preview is a picture in the tile's memory and the Settings preview draws once: neither holds a process or a device | no resource |
| Sound sent to the person's browser page (the web mixer) | shared/browsermedia.rs (`stop_audio_of`) | kept until the page closed | hook `browser-audio` switches the page's sound off | STOP |
| Domain listener (Settings -> Domains) | agent/cloudlisten.rs | continue | unchanged: a machine service the administrator set up that serves their domains | CONTINUE |

Not decided: a Settings switch "keep playing after sign-out" (parked).
