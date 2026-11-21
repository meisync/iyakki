//! The real machine under [`crate::a2dp_send`]: two outgoing L2CAP connections to the paired speaker's AVDTP channel (the first
//! carries the signalling, the second the media). Needs administrator rights like the rest of the Bluetooth code. Nothing here has
//! been tried against real headphones: the protocol and the encoding it drives are tested (`a2dp_send_tests.rs`).

use std::sync::{Arc, OnceLock};

use crate::a2dp_send::{Sender, SendControl};
use crate::avdtp::PSM_AVDTP;
use crate::bthid_sys::{hub, L2Sock};
use crate::btmgmt;

/// The real [`SendControl`]: one stream at a time, paced to the speed of the sound.
pub fn real_sender() -> Arc<dyn SendControl> {
    static S: OnceLock<Arc<dyn SendControl>> = OnceLock::new();
    S.get_or_init(|| {
        Arc::new(Sender::<L2Sock, L2Sock>::new(true, |address| {
            if hub().known(address).is_none() {
                return Err("that device is not paired: pair it first (Settings -> Bluetooth)".into());
            }
            let addr = btmgmt::parse_addr(address).ok_or("bad address")?;
            let sig = L2Sock::connect(addr, PSM_AVDTP, 12).map_err(|e| format!("could not connect to the device for audio: {e}"))?;
            let media: Box<dyn FnOnce() -> std::io::Result<L2Sock> + Send> = Box::new(move || L2Sock::connect(addr, PSM_AVDTP, 12).map_err(|e| std::io::Error::other(e.to_string())));
            Ok((sig, media))
        }).with_aac(crate::a2dp_aac::ready_backend))
    })
    .clone()
}
