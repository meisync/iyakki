# Restart grant: keep until I switch it off (alpha.5)

Owner request 2026-11-16: the paired computer stays allowed to restart the device anytime until the administrator switches it off.

## Decisions

1. **One more opt-in choice on the existing standing restart grant, not a new channel.** A computer can only reach the device while
   developer mode is on, so a grant that outlives the automatic switch-off needs the connection to outlive it too. Ticking
   "Keep until I switch it off" therefore clears developer mode's automatic switch-off time (state `until` becomes 0) and the page, the
   card and the chat answer say so in plain words ("developer mode will stay on until you switch it off"). A separate always-on
   restart-only listener was rejected: it would be a second, always-open door with its own pairing and TLS handling, which is more
   attack surface than one developer connection the person can see (the DEV badge stays in the top bar).
2. **It survives restarts.** `restart-grant.json` gets a field `persist` (default false, so old files load as ordinary grants; unknown
   fields of newer releases are kept; schema stays 1 because the field is additive). `state.json` already persists `until`.
   A test reloads both from disk.
3. **Scope unchanged:** restart and switch-off only (`/v1/dev/power/*`), never commands, root or patching. Every use and refusal is in
   the activity log; the restart-loop guard (`awaiting_start`) applies unchanged.
4. **Who and where:** administrators on the device only. Settings, Developer mode shows a second line under the restart box, key `b`
   ("Keep until I switch it off"). Chat: `restart_grant` takes `persist: true` (with `on: true`); it is a hard ask, never covered by a
   session grant; `restart_grant_status` says "Restarts allowed until you switch it off".
5. **It ends only by hand:** unticking the restart box (`r` or `on: false`), going back to the ordinary grant (`b` again), Disconnect /
   Remove this computer, the computer losing the `power` scope (the shell scope implies it), or developer mode switched off by hand
   (which forgets every grant). The automatic switch-off never ends it: if a time is somehow set (for instance the person picks one
   with `h` while it is kept) `DevMode::is_on` skips it and logs `devmode.timer.kept`, and `enable` does not set one.
6. **No open-ended connection is left behind:** when the last kept grant ends while developer mode is still on, the default 4 h
   switch-off time is set again from that moment (`devmode.restart.timer_back`).

## Risk note

A kept grant plus a kept connection means any holder of that computer's pairing key (scope `power`) can restart the device at any
time until the administrator ends it. That is exactly what the owner asked for; it is opt-in, visible (DEV badge, page line, status
tool), audited, and ends with one tap.

## Proof level

Unit tests on a real temporary developer-mode folder (agent, tool and Settings page). Not run on a device: Manual needed.
