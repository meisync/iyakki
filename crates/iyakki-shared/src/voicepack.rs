//! Constants of the voice pack item that the Quick install list needs without the agent (F-SPLIT-03, bucket b0); `iyakki_agent::voicepack` re-exports them.

/// About how much Whisper tiny is (megabytes; the project's own figure is about 75 MB). An estimate.
pub const WHISPER_MB: u32 = 75;
/// About how much Kokoro is (megabytes; the Kokoro step says about 330 MB). An estimate.
pub const KOKORO_MB: u32 = 330;

/// The licence line of the Setup card (the review has the texts).
pub const LICENCE_SHORT: &str = "IYAKKI provides integration code only. Lemonade (Apache-2.0) is the separate server this item uses; the Whisper and Kokoro models are pulled by Lemonade at your click, under their own licences, which Lemonade or each model's page states. They are not shipped with IYAKKI; downloaded at your click, and IYAKKI gives no guarantee about them.";
