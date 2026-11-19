# Core split, lane B: groups (alpha.6)

Rule: one group = all its moves and facets in one go, then ONE compile and test pass (affected crates plus the contract tests
`boundary`, `dependency_ratchet`, `ui_agent_uses`, `layer_split`, `feature_matrix`), regenerate the ratchet
(`~/iyakki-work/regen_ratchet.sh`), commit. The count after each group is written in the commit message.

Method per module: whole flat module moves into `iyakki-shared` (the agent keeps `pub use iyakki_shared::M;`, UI paths rewritten to
`iyakki_shared::M`) when everything it needs already lives below the agent; otherwise the pure part moves and the rest gets a facet.

| Group | Modules (domain) | Plan |
|---|---|---|
| G1 sound and display | bluetoothpair, btaudiotools, audiosink, player, webmix, sounddoctor, micpcm, cameras, dash | whole-module moves where the closure is below the agent (bluetoothpair, micpcm done); mixer/audiosink chain next |
| G2 accounts and vault | cloudroles, sharedvault, cloudtenants, authcodes, gitkeys, pending, pendingplus | cloudroles done; the rest need cloudvault/cloudclient/bugs moved first or a facet |
| G3 settings | platform, scope, config, power, cloudmode | scope needs a facet (file roots); config/power need tenantconfig/battery |
| G4 files and network | gitconn, gitbrowse, quickinstall, mediadl, sharedfolder, domains, devmode, webengine, browserext | git trio shares sshkey/bugs/clock: move those helpers first, then the trio |
| G5 chat helpers | flows, extagent, voicedesk, voice, story, tasks, assistrun, gemini, geminilive, lemonade, modelopts, modelhost | models moved; providererr/limit_advice blocks gemini; voicedesk pure half (mics, language hint) moves to shared::voicedesk |

Status and counts are recorded in the F-SPLIT-03 row notes of FEATURES.md and in `UI-AGENT-USES.txt`.
