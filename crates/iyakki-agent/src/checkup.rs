//! CHECK-UP: a guided hardware self-test, in plain words. Every step ends as Pass, Fail or Skipped, with advice and (where DOCTOR
//! or Settings has one) the same fix button as DOCTOR.
//!
//! This file holds everything the CHECK-UP tile decides and reads, so the tile (`apps/checkup*.rs` in the shell) only draws and keeps
//! state:
//!
//! * [`Inventory`] is what the machine lists (input devices, sound cards, microphones, network adapters, Bluetooth, screens,
//!   battery). [`read_inventory`] reads `/proc` and `/sys`; tests give a made-up one, and the readers take a root folder so they
//!   are tested on files in a temporary folder.
//! * One small function per step turns the inventory plus what the person said or what a probe returned into a [`StepResult`].
//! * Nothing recorded is kept: the microphone step looks at the samples for loudness and drops them; the camera step is handled by
//!   the tile and only a verdict reaches here. The report ([`report_lines`]) holds verdicts and device names, never sound or pictures.
//!   The tile hands the report to Share diagnostics ([`set_last_report`], read by `diagshare` through the shell), which redacts it like
//!   every other section.

use std::path::Path;

use crate::voicedesk::{self};
pub use iyakki_shared::checkup::*;
pub use iyakki_shared::checkup::{start_mic, CamHeard, MicHeard, MicStart, Outcome, Said, ScreenOut};









// ------------------------------------------------------------------------------------------------ the inventory













/// The real machine. `screens` come from the shell (it drives the displays); everything else is read here.
pub fn read_inventory(screens: Vec<ScreenOut>) -> Inventory {
    let proc_root = Path::new("/proc");
    let sys = Path::new("/sys");
    let (keyboards, pointers, touchscreens) = count_inputs(&std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default());
    let sound_chosen = String::new();
    let (eth, wifi) = read_adapters(sys);
    let (bluetooth, bluetooth_blocked, wifi_blocked) = read_radios(sys);
    let env = crate::battery::BatteryEnv::real();
    let r = crate::battery::read(&env);
    let power = PowerFacts {
        battery: r.readable(),
        no_driver: r.presence == crate::battery::Presence::NoDriver,
        laptop: crate::battery::laptop_chassis(&env),
        percent: r.percent_whole(),
        status: r.status_words().to_string(),
        ac_online: r.ac_online,
        health_pct: r.health_pct.map(|h| h.round() as u32),
    };
    Inventory {
        keyboards,
        pointers,
        touchscreens,
        sound_cards: crate::doctor::sound_card_count(),
        sound_chosen,
        sound_chosen_present: true,
        mics: voicedesk::list_mics(proc_root),
        product: voicedesk::product_name(sys),
        t2_mic_hidden: voicedesk::t2_mic_hidden(proc_root),
        screens,
        ethernet: eth,
        wifi,
        route: crate::doctor::has_default_route(),
        bluetooth,
        bluetooth_blocked,
        wifi_blocked,
        power,
        gpus: crate::gpu::detect(Path::new("/")),
        k3s_gpu: K3sSees::NotAsked,
        usb: read_usb(sys),
    }
}

// ------------------------------------------------------------------------------------------------ the person's side







// ------------------------------------------------------------------------------------------------ the speaker

/// A short, soft test tone (a rising pair of notes) as a WAV: 16 kHz mono, about 0.7 s.
pub fn tone_wav() -> Vec<u8> {
    let mut pcm = crate::voice::tone(523.0, 300);
    pcm.extend(crate::voice::tone(784.0, 400));
    crate::voice::encode_wav(&pcm, crate::voice::SAMPLE_RATE)
}


// ------------------------------------------------------------------------------------------------ the microphone







// ------------------------------------------------------------------------------------------------ the camera




// ------------------------------------------------------------------------------------------------ the machine's own steps








// ------------------------------------------------------------------------------------------------ the report








#[cfg(test)]
#[path = "checkup_tests.rs"]
mod tests;

#[cfg(test)]
use crate::doctor::Action;
#[cfg(test)]
use crate::voicedesk::Capture;
#[cfg(test)]
use crate::voicedesk::Mic;
#[cfg(test)]
use crate::voicedesk::Recorders;
#[cfg(test)]
use std::time::Duration;
