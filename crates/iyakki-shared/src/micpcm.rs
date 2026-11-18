//! A capture PCM with the sample format named, for sound cards whose capture offers BOTH 32-bit-container formats (`S24_LE` and
//! `S32_LE`). ALSA's `plug` picks `S24_LE` for a 16-bit recorder on such a card, and on the MacBookPro14,1's CS8409 codec (the
//! out-of-tree Apple driver) the hardware fills the words from the top: the 24 bits read the `S24_LE` way are the wrong 24, every
//! sample wraps and the recording is full-scale noise (measured 2026-11-08: 7000 zero crossings a second, no voice; read as
//! `S32_LE` the same bytes are the voice, bits 31 to 13 live, the rest dead). Standard HDA codecs offer `S32_LE` alone and are never
//! affected.
//!
//! The agent learns the capture formats once per start (`arecord --dump-hw-params`, [`crate::soundprobe`]), remembers the cards
//! that need it ([`learn`]), writes `pcm.iyakkimicCdD { type plug; slave { pcm "hw:C,D"; format S32_LE } }` into `/etc/asound.conf`
//! with the output configuration ([`sections`], appended by [`crate::softvol`]), and every recorder opens that PCM instead of
//! `plughw:C,D` ([`pcm_for`]): the voice recorder and dictation (`voicedesk::record_cmd_for`), the browser's microphone plugin
//! (`IYAKKI_WEBVIEW_MIC_PCMS`, `dist/webview/wpe-helper/micshare.c`).

use std::sync::Mutex;

/// One capture device that needs its format named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicPcm {
    pub card: u32,
    pub device: u32,
    pub format: &'static str,
}

static KNOWN: Mutex<Vec<MicPcm>> = Mutex::new(Vec::new());

/// The format to name for a capture device that lists these formats: `S32_LE` when both 32-bit-container formats are offered (the
/// ambiguous case), nothing otherwise (one format, or 16-bit ones: `plug` chooses right).
pub fn named_format(formats: &[String]) -> Option<&'static str> {
    let has = |f: &str| formats.iter().any(|x| x == f);
    (has("S32_LE") && has("S24_LE")).then_some("S32_LE")
}

/// The PCM's name: `iyakkimic0d0` (letters and digits only: ALSA names with `:` or `,` are device strings).
pub fn pcm_name(card: u32, device: u32) -> String {
    format!("iyakkimic{card}d{device}")
}

/// The `asound.conf` text of one such PCM.
pub fn section(m: &MicPcm) -> String {
    format!(
        "# Capture on card {card} device {device} offers S24_LE and S32_LE: the format is named, or plug picks the wrong 24 bits (micpcm.rs).\n\
         pcm.{name} {{\n    type plug\n    slave {{\n        pcm \"hw:{card},{device}\"\n        format {fmt}\n    }}\n}}\n",
        card = m.card,
        device = m.device,
        name = pcm_name(m.card, m.device),
        fmt = m.format
    )
}

/// The text of every known PCM, in card order (empty when none is needed).
pub fn sections_of(known: &[MicPcm]) -> String {
    let mut v: Vec<&MicPcm> = known.iter().collect();
    v.sort_by_key(|m| (m.card, m.device));
    v.iter().map(|m| section(m)).collect()
}

pub fn sections() -> String {
    sections_of(&known())
}

/// Remember a capture device's formats; true when it needs (and now has) a named PCM.
pub fn learn(card: u32, device: u32, formats: &[String]) -> bool {
    let Some(format) = named_format(formats) else { return false };
    let mut k = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let m = MicPcm { card, device, format };
    if !k.contains(&m) {
        k.retain(|x| !(x.card == card && x.device == device));
        k.push(m);
        log::info!("microphone: card {card} device {device} offers S24_LE and S32_LE: recordings go through {} (format {format})", pcm_name(card, device));
    }
    true
}

pub fn known() -> Vec<MicPcm> {
    KNOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The device string of a known card and device, by the `plughw:C,D` id of its microphone: `iyakkimic0d0`, or `None` (use the id).
pub fn pcm_for_in(known: &[MicPcm], plughw_id: &str) -> Option<String> {
    let rest = plughw_id.strip_prefix("plughw:").or_else(|| plughw_id.strip_prefix("hw:"))?;
    let (c, d) = rest.split_once(',')?;
    let (card, device) = (c.trim().parse::<u32>().ok()?, d.trim().parse::<u32>().ok()?);
    known.iter().find(|m| m.card == card && m.device == device).map(|m| pcm_name(m.card, m.device))
}

pub fn pcm_for(plughw_id: &str) -> Option<String> {
    pcm_for_in(&known(), plughw_id)
}

/// The browser helper's map, `plughw:0,0=iyakkimic0d0;plughw:2,0=iyakkimic2d0` (empty when none).
pub fn helper_env_of(known: &[MicPcm]) -> String {
    let mut v: Vec<&MicPcm> = known.iter().collect();
    v.sort_by_key(|m| (m.card, m.device));
    v.iter().map(|m| format!("plughw:{},{}={}", m.card, m.device, pcm_name(m.card, m.device))).collect::<Vec<_>>().join(";")
}

/// Look at every capture device of `/proc/asound/pcm` once (`probe` runs `arecord --dump-hw-params` for a card and device and returns
/// its text); returns the cards that were given a named PCM. Called from the start-up look that also sets the capture defaults.
pub fn learn_cards(pcm_list: &str, probe: &dyn Fn(u32, u32) -> Option<Vec<String>>) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for line in pcm_list.lines() {
        // "00-00: CS8409/CS42L83 Analog : CS8409/CS42L83 Analog : playback 1 : capture 1"
        if !line.contains("capture") {
            continue;
        }
        let Some((ids, _)) = line.split_once(':') else { continue };
        let Some((c, d)) = ids.trim().split_once('-') else { continue };
        let (Ok(card), Ok(device)) = (c.parse::<u32>(), d.parse::<u32>()) else { continue };
        if let Some(formats) = probe(card, device) {
            if learn(card, device, &formats) {
                out.push((card, device));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn only_a_card_with_both_32_bit_container_formats_gets_a_named_pcm() {
        assert_eq!(named_format(&s(&["S24_LE", "S32_LE", "S24_3LE"])), Some("S32_LE"), "the MacBookPro14,1's codec");
        assert_eq!(named_format(&s(&["S16_LE", "S32_LE"])), None, "a standard HDA codec: plug chooses right");
        assert_eq!(named_format(&s(&["S16_LE"])), None);
        assert_eq!(named_format(&s(&["S24_LE"])), None, "one 24-bit format only: nothing to choose between");
        let m = MicPcm { card: 0, device: 0, format: "S32_LE" };
        let t = section(&m);
        assert!(t.contains("pcm.iyakkimic0d0 {") && t.contains("pcm \"hw:0,0\"") && t.contains("format S32_LE"), "{t}");
        assert_eq!(pcm_for_in(&[m.clone()], "plughw:0,0").as_deref(), Some("iyakkimic0d0"));
        assert_eq!(pcm_for_in(&[m.clone()], "plughw:1,0"), None, "another card keeps plughw");
        assert_eq!(pcm_for_in(&[m.clone()], "default"), None);
        assert_eq!(helper_env_of(&[MicPcm { card: 2, device: 0, format: "S32_LE" }, m.clone()]), "plughw:0,0=iyakkimic0d0;plughw:2,0=iyakkimic2d0");
        assert_eq!(sections_of(&[]), "");
    }

    #[test]
    fn the_start_up_look_reads_the_pcm_list_and_asks_each_capture_device_once() {
        let list = "00-00: CS8409/CS42L83 Analog : CS8409/CS42L83 Analog : playback 1 : capture 1\n00-03: HDMI 0 : HDMI 0 : playback 1\n02-00: USB Audio : USB Audio : playback 1 : capture 1\n";
        let asked = std::sync::Mutex::new(Vec::new());
        let probe = |c: u32, d: u32| -> Option<Vec<String>> {
            asked.lock().unwrap().push((c, d));
            Some(if c == 0 { s(&["S24_LE", "S32_LE"]) } else { s(&["S16_LE"]) })
        };
        let got = learn_cards(list, &probe);
        assert_eq!(asked.lock().unwrap().clone(), vec![(0, 0), (2, 0)], "capture devices only, not the HDMI output");
        assert_eq!(got, vec![(0, 0)]);
        assert!(known().iter().any(|m| m.card == 0 && m.device == 0 && m.format == "S32_LE"));
        assert!(sections().contains("pcm.iyakkimic0d0"));
        assert_eq!(pcm_for("plughw:0,0").as_deref(), Some("iyakkimic0d0"));
        assert_eq!(pcm_for("plughw:2,0"), None);
        assert!(learn_cards("00-00: x : x : capture 1\n", &|_, _| None).is_empty(), "a device that did not answer is left alone");
    }
}
