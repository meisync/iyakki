# Events: hear what happens, online and offline

An app can ask to hear about things that happen on the machine: the network went away, the battery is low, a file in a folder changed, a paired computer came online. The core delivers these events while the app runs, and keeps the ones that happen while it does not run, so the app can catch up at its next start before it draws its first frame. A handler can act: keep state, leave a note, start a flow, call a chat tool.

The events are a layer over what the core already knows (its pushed events, the notification bus, the file watcher, the settings store). There is no second bus: a service tells the core, the core decides who hears it.

## Four lines of what an app does

```rust
let mut router = EventRouter::new()
    .on("connectivity.changed", |e, acts| { if !e.late { acts.notify("The network went away"); } })
    .on_missed(|_, _| reload_what_i_mirror());
router.register_all(port.as_mut());      // refusals come back as sentences
router.begin(port.as_mut(), acts.as_mut()); // what waited is handled now, before the first frame
// on every tick: router.pump(port.as_mut(), acts.as_mut());
```

The full app is `docs/sdk/recipes/events_react.rs` (compiled and driven by a test).

## What the app is handed

* `EventsPort` (`register`, `unregister`, `registrations`, `start`, `stop`, `poll`, `replay`, `publish`): the core's side is `iyakki_core_api::AppEvents`; `MemEvents` is the in-memory port with the same rules, for tests and the browser preview.
* `EventRouter`: registers the app's interests, hands each event to its handler in order, never twice, and reports events nobody handled.
* `EventActions` (`notify`, `start_flow`, `call_tool`): what a handler may do, each through the core and each needing a capability the app holds (`notify`, `flows.start`, `tools.request`). `notify` leaves a note in the inbox under the app's name; `start_flow` starts a published flow of the person by its name (a flow that changes something asks inside as always); `call_tool` never runs anything by itself: it puts a request in the person's waiting list, and the tool runs only if the person approves the card of `app_actions_decide`, which names the app and the tool. `RecordingActions` is the fake for tests.

## What the core guarantees

* **Only what the app registered, and may hear.** Each event has a scope (a capability such as `events.network`) that the app must hold; the person sees it on the install card and in Settings. An event the app has no scope for is refused with a sentence at registration and is never delivered. Taking the scope away stops delivery at once.
* **Order.** Every delivered event has a sequence number that only grows (per app, also across restarts) and a time. A handler never sees a number twice.
* **Online.** While the app runs, events arrive on `poll` in order. An app counts as running only while it polls: one that has not polled for 3 seconds is away (a shown app polls every frame; one that goes Home just stops), and an event that waited longer than that before it was handed over is `late`.
* **Offline.** While the app does not run (or the machine slept), events wait in one bounded queue per app: at most 100, none older than 7 days. The oldest is dropped and counted; the count arrives first, at the next start, as one `missed` event (`e.is_missed()`, `e.missed_count()`). State events keep only the latest (the network status, the battery, a display change per kind), so a flapping network is one event, not forty. What waited arrives with `late = true`.
* **Not a flood.** An event that can come in bursts has a per-minute limit for each app; what the limit drops is counted in `missed`.
* **No secrets.** A payload is a few typed fields (words, numbers, short text). A settings event carries the key, never the value. Custom events may not use a field named like a secret (`password`, `token`, `key`, ...). The audit log names events and numbers, never values.
* **The person decides.** Settings > App events lists every registration (app, event, what it lets the app hear, how many wait) with a switch for each. An event switched off is neither queued nor delivered. The chat tools `app_events_list`, `app_events_catalogue`, `app_events_register`, `app_events_unregister`, `app_events_switch` and `app_events_replay_missed` do the same (the changes ask first).
* **Replay.** The last 50 delivered events are kept: after a crash the app can ask for them again (`router.recover`), or the person can have them handed over again (`app_events_replay_missed`; marked `late`). Dropped events cannot come back: that is what `missed` is for.

## Late and missed: what the app must do

An event that arrives `late` happened while the app was away. Update the screen, but do not ring a bell for a call that ended an hour ago. A `missed` count means the app's picture of the world may be wrong: read the state again. `app-check` fails an app that registers for events and handles neither (`events-missed`, `events-late`).

## Custom events

An app that holds `events.publish` may publish `custom.<its own name>.<event>` with up to 8 plain fields; apps that hold `events.custom` may register for it. This is how apps tell each other things (an app-to-app message is a custom event). The core checks the publisher's name, so an app cannot speak as another.

## What the app may do: `app.permissions`

An app must always learn what it may do, so one event is different from the rest: **`app.permissions`**. It is told to the app it is about, and to no other app or person's copy. It needs no registration and no scope, and the person cannot switch it off (they can take the permissions themselves away instead). It follows the other queue rules: it waits while the app is closed and arrives at the next start marked `late`, even after a restart of the machine; several changes before the app listens become **one** event with the latest effective set and the changes added up (a grant taken away again before the app heard is no change at all). Coalescing is its rate limit: a permission change is never dropped for being too frequent.

The payload is names, words and counts only: `granted` and `revoked` (names of capabilities and scopes such as `camera.view`, `files.read`, `net.connect`, `events.network`, `tools.request`), `dropped` (events the app had registered for that ended because the scope they needed was taken away), `effective` (what it holds now, cut with `+N` when long) and `held` (how many), `by` (`person`, `administrator`, `policy`, `store` or `expiry`) and `restart` (the app must restart or register again to use what changed).

```rust
let router = EventRouter::new()
    .on("connectivity.changed", handler)
    .on_permissions(|change, e, acts| {
        // stop using what was taken away; read what is held now instead of keeping a copy
        refresh(port.capabilities(), change.revoked.clone());
    });
```

The core tells the app wherever it changes what an app may do: Settings > Apps access (a permission taken away), the install approval (given at install), a store update that asks for more or fewer capabilities (the ask-again flow), and the removal of an app. Taking an `events.*` scope away also ends the registrations that needed it, and `dropped` says which. Not built yet: tenant or company policy updates, a change of the folders the file policy lets an app read, and a time-limited grant that ends (the `expiry` word exists, nothing calls it). `app-check` fails an app that uses events and never handles it (rule `events-permissions`). The chat tool `app_permissions_status` (free) lists what an app holds and the last change told to it, never a payload value. The person can take a permission away from an app (Settings > App events, key `p` or the button "Take its permission away", or the chat tool `app_permissions_set`, which asks) and give back one they took away; the app is told with `by` set to `person`, and what it registered for with that scope ends.

## The catalogue (version 1)

| Event | Scope | What the app hears | Fields | Waiting and limits |
|---|---|---|---|---|
| `connectivity.changed` | `events.network` | the network went online, offline, metered or needs a sign-in page | state | latest only |
| `vpn.changed` | `events.network` | a VPN came up or went down | name, state | latest per name |
| `app.permissions` | none | what this app may do changed: granted, taken away, dropped registrations, the effective set, who changed it, restart needed (always told to the app it is about; cannot be switched off) | app, granted, revoked, dropped, effective, held, by, restart | one event: the latest effective set with the changes added up |
| `peers.changed` | `events.network` | a paired computer was paired, forgotten, came online or went offline | device, change | every event kept, 30 a minute |
| `peers.battery` | `events.network` | the battery of a paired device changed | device, percent | latest per device |
| `power.source` | `events.power` | the machine moved to battery or to mains | source | latest only |
| `power.battery_low` | `events.power` | the battery is low | percent | latest only |
| `power.sleep` | `events.power` | the machine goes to sleep or woke up | phase | every event kept, 20 a minute |
| `power.shutdown_pending` | `events.power` | a shutdown or restart is coming | in_seconds | latest only |
| `thermal.changed` | `events.power` | the machine is slowing down because it is hot, or stopped | state | latest only |
| `memory.pressure` | `events.power` | memory is getting short | level | latest only |
| `quiet_hours.changed` | `events.power` | quiet hours (do not disturb) went on or off | state | latest only |
| `session.changed` | `events.session` | a person signed in or out, or the screen was locked or unlocked | state | every event kept, 30 a minute |
| `vault.changed` | `events.session` | the vault was locked or unlocked | state | latest only |
| `security.alert` | `events.security` | a security alert: a failed sign-in, a new paired device, an unlock from a new place (a count, never who or where) | kind, count | latest per kind |
| `cert.expiring` | `events.security` | a certificate or key expires soon | what, days | latest per what |
| `display.changed` | `events.display` | the screen size, scale, theme, skin or font changed | what | latest per what |
| `locale.changed` | `events.locale` | the language changed | locale | latest only |
| `accessibility.changed` | `events.locale` | an accessibility setting changed | what | latest per what |
| `audio.devices` | `events.devices` | a sound device was added or removed, or the default changed | kind, change, device | every event kept, 30 a minute |
| `camera.devices` | `events.devices` | a camera was added or removed, or the default changed | change, device | every event kept, 30 a minute |
| `bluetooth.device` | `events.devices` | a Bluetooth device connected or disconnected | device, change | every event kept, 30 a minute |
| `printer.changed` | `events.devices` | a printer or scanner was added or removed | kind, device, change | every event kept, 30 a minute |
| `disk.health` | `events.devices` | a disk reports a health warning, or recovered | disk, state | latest per disk |
| `storage.volume` | `events.storage` | a volume was mounted or unmounted, or is nearly full | volume, change | every event kept, 30 a minute |
| `backup.finished` | `events.storage` | a backup finished or failed | name, outcome | every event kept, 20 a minute |
| `clock.changed` | `events.clock` | the time zone changed or the clock jumped | change | every event kept, 10 a minute |
| `clock.midnight` | `events.clock` | a new day began | none | latest only |
| `clock.minute` | `events.clock` | a tick every minute (the core runs it only while an app listens) | none | latest only |
| `clock.hour` | `events.clock` | a tick every hour (the core runs it only while an app listens) | none | latest only |
| `reminder.due` | `events.reminders` | a reminder or a calendar entry is due | title | every event kept, 30 a minute |
| `settings.changed` | `events.settings` | a setting changed (its name, never its value) | setting | latest per setting |
| `app.lifecycle` | `events.apps` | an app came to the front or went to the back | app, change | latest per app |
| `app.installed` | `events.apps` | an app was installed, updated or removed | app, change | every event kept, 30 a minute |
| `inbox.pending` | `events.inbox` | notifications are waiting (the number) | count | latest only |
| `approvals.pending` | `events.inbox` | approvals are waiting for the person (the number) | count | latest only |
| `updates.changed` | `events.updates` | an update is available, downloaded or applied | component, phase | every event kept, 20 a minute |
| `transfer.finished` | `events.updates` | a download or an install finished or failed | kind, name, outcome | every event kept, 30 a minute |
| `flows.run` | `events.flows` | a flow or job finished or failed | flow, outcome | every event kept, 60 a minute |
| `models.host` | `events.models` | the model host is ready, failed or changed model | state, model | latest only |
| `files.changed` | `events.files` | a file in a watched folder was created, changed or removed (its name, never its content) | folder, name, change | latest per name |
| `voice.wake_heard` | `events.voice` | the wake phrase was heard (experimental) | none | every event kept, 12 a minute |

Timers (`clock.minute`, `clock.hour`, `reminder.due`) run only while somebody listens: the core asks `wanted(name)` before it starts one. `voice.wake_heard` exists only while the person has switched the experimental wake phrase on. `settings.changed` takes a key prefix and `files.changed` a folder inside the person's home that the app may read (the file policy); a folder it may not read is refused.

## Where things run, and what is not built

The engine, the catalogue, the queue and the rules are in `iyakki-core-api` (`appevents`); the fake core and the local core use the same engine, so a test means the same as the device. The local core keeps each app's registrations and queue in a file of the data folder (`appevents/`), so they survive a restart. The scopes are held by built-in apps through the capability table and by installed apps through the person's approval record.

Not built yet (planned rows): the services that call `emit` for most events (the engine takes any event, and the mappers for settings, flows and the inbox are written; the others wait for their source to call it), delivery to WASM tiles and to client devices.
