//! The picture choices of a story (which scenes get a picture, the look, the quality, the per-story options) and the model choice: plain data
//! moved out of `iyakki_shared::storyillus` and `iyakki_shared::settings`, which re-export them under the old paths.

use crate::{Audience, PictureStyle, Scene};
use serde::{Deserialize, Serialize};

/// A model of one provider the person chose (settings, a story's own picture model).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
}

/// Which scenes of a story get a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Coverage {
    /// One picture per chapter or scene (the story's own limit applies).
    #[default]
    Chapters,
    /// One picture, for the opening.
    Cover,
    /// Nothing is drawn until the person says so.
    Ask,
}

impl Coverage {
    pub const ALL: &'static [Coverage] = &[Coverage::Chapters, Coverage::Cover, Coverage::Ask];

    pub fn id(self) -> &'static str {
        match self {
            Coverage::Chapters => "chapters",
            Coverage::Cover => "cover",
            Coverage::Ask => "ask",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Coverage::Chapters => "every chapter",
            Coverage::Cover => "cover only",
            Coverage::Ask => "ask me each time",
        }
    }

    pub fn from_id(s: &str) -> Option<Coverage> {
        let s = s.trim().to_lowercase();
        Self::ALL.iter().copied().find(|c| c.id() == s)
    }

    /// The next one in the list (a chip that cycles).
    pub fn next(self) -> Coverage {
        let at = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// The scenes to draw: all of them, or only the first one (the cover).
pub fn pick_scenes(mut scenes: Vec<Scene>, c: Coverage) -> Vec<Scene> {
    if c == Coverage::Cover {
        scenes.truncate(1);
    }
    scenes
}

/// The look of the pictures of a story: the style line and what to avoid. `Storybook` is the look every story had before this
/// choice existed (the story's own style line, see [`crate::story::PictureStyle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Look {
    #[default]
    Storybook,
    Watercolor,
    Photo,
    Cinematic,
    Anime,
    Pencil,
}

impl Look {
    pub const ALL: &'static [Look] = &[Look::Storybook, Look::Watercolor, Look::Photo, Look::Cinematic, Look::Anime, Look::Pencil];

    pub fn id(self) -> &'static str {
        match self {
            Look::Storybook => "storybook",
            Look::Watercolor => "watercolor",
            Look::Photo => "photo",
            Look::Cinematic => "cinematic",
            Look::Anime => "anime",
            Look::Pencil => "pencil",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Look::Storybook => "storybook illustration",
            Look::Watercolor => "watercolor",
            Look::Photo => "photorealistic",
            Look::Cinematic => "cinematic",
            Look::Anime => "anime",
            Look::Pencil => "pencil sketch",
        }
    }

    pub fn from_id(s: &str) -> Option<Look> {
        let s = s.trim().to_lowercase();
        Self::ALL.iter().copied().find(|c| c.id() == s)
    }

    pub fn next(self) -> Look {
        let at = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The style line. `Storybook` is the story's own style line, so stories made before this choice read the same.
    pub fn phrase(self, story_style: PictureStyle) -> &'static str {
        match self {
            Look::Storybook => story_style.phrase(),
            Look::Watercolor => "a delicate watercolor painting, soft washes of colour, visible paper grain, loose brush strokes, gentle edges",
            Look::Photo => "a realistic photograph, shot on a 50mm lens at f/2.8, soft natural daylight, natural skin texture, natural proportions, real materials, sharp focus, fine detail, no extra fingers",
            Look::Cinematic => "a cinematic film still, 35mm anamorphic lens, dramatic soft lighting, shallow depth of field, rich colour grading, fine film grain",
            Look::Anime => "an anime illustration, clean line art, cel shading, expressive eyes, vivid colours, detailed background",
            Look::Pencil => "a detailed pencil sketch, graphite shading, cross-hatching, white paper, monochrome",
        }
    }

    /// A short style line for models that ignore long prompts (SD-Turbo).
    pub fn short_phrase(self, story_style: PictureStyle) -> String {
        match self {
            Look::Storybook => story_style.phrase().split(',').next().unwrap_or("a storybook illustration").trim().to_string(),
            Look::Watercolor => "a soft watercolor painting".into(),
            Look::Photo => "a realistic photo, 50mm lens, natural light".into(),
            Look::Cinematic => "a cinematic film still, 35mm lens, soft light".into(),
            Look::Anime => "an anime illustration, clean line art, cel shading".into(),
            Look::Pencil => "a detailed pencil sketch, graphite shading".into(),
        }
    }

    /// What the picture should not look like (the negative prompt): the look's own opposites, then what no picture should have.
    pub fn negative(self, a: Audience) -> String {
        let own = match self {
            Look::Storybook => "photograph, harsh shadows, dark and gloomy",
            Look::Watercolor => "photograph, 3d render, digital art, harsh outlines",
            Look::Photo => "cartoon, illustration, painting, anime, 3d render, plastic skin, distorted proportions, oversaturated",
            Look::Cinematic => "cartoon, flat colours, subtitles, oversaturated",
            Look::Anime => "photograph, realistic, 3d render, bad anatomy",
            Look::Pencil => "colour, photograph, painting, smudges",
        };
        let kids = if a == Audience::Kids { ", scary, violent" } else { "" };
        format!("{own}, text, letters, watermark, logo, extra fingers, deformed hands, distorted face, blurry{kids}")
    }
}

/// How good the pictures should be: picks the picture model among the ones that are ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Quality {
    /// The picture model that was selected (or the first one found): what every story did before this choice existed.
    #[default]
    Selected,
    /// The smallest model.
    Fast,
    /// The middle one.
    Balanced,
    /// The largest one that fits.
    Best,
}

impl Quality {
    pub const ALL: &'static [Quality] = &[Quality::Selected, Quality::Fast, Quality::Balanced, Quality::Best];

    pub fn id(self) -> &'static str {
        match self {
            Quality::Selected => "selected",
            Quality::Fast => "fast",
            Quality::Balanced => "balanced",
            Quality::Best => "best",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Selected => "as selected",
            Quality::Fast => "Fast",
            Quality::Balanced => "Balanced",
            Quality::Best => "Best",
        }
    }

    pub fn from_id(s: &str) -> Option<Quality> {
        let s = s.trim().to_lowercase();
        Self::ALL.iter().copied().find(|c| c.id() == s)
    }

    pub fn next(self) -> Quality {
        let at = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// The picture choices of one story, kept with it and as the default for new ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PictureOpts {
    pub look: Look,
    pub quality: Quality,
    /// A picture model for this story (a model of any configured provider); `None` is "use Quality, or the selected one".
    pub model: Option<ModelChoice>,
    /// Keep the same look: every picture of the story uses one seed.
    pub keep_look: bool,
    /// That seed, made when the first picture of the story is drawn with `keep_look` on (never part of the default).
    pub seed: Option<u64>,
    /// Verbose picture log: request headers (no secrets) and the server's health.
    pub verbose: bool,
    /// The most minutes one picture may take before it is given up (0: the usual 15).
    pub timeout_min: u32,
    /// The styles to draw (empty: just `look`). With more than one, every scene is drawn once per style.
    pub styles: Vec<Look>,
    /// How many pictures of each style (different seeds); 0 is 1.
    pub per_style: u32,
    /// What the styles were before Realistic pictures was ticked (so unticking brings them back).
    pub before_realistic: Option<Vec<Look>>,
}

/// The most variants of one scene and style.
pub const MAX_PER_STYLE: u32 = 3;

impl PictureOpts {
    /// The styles to draw, without repeats, at least one.
    pub fn styles_list(&self) -> Vec<Look> {
        let mut v: Vec<Look> = Vec::new();
        for l in if self.styles.is_empty() { vec![self.look] } else { self.styles.clone() } {
            if !v.contains(&l) {
                v.push(l);
            }
        }
        v
    }

    /// Pictures per style, 1 to 3.
    pub fn count(&self) -> u32 {
        self.per_style.clamp(1, MAX_PER_STYLE)
    }

    pub fn next_count(&mut self) {
        self.per_style = self.count() % MAX_PER_STYLE + 1;
    }

    /// Tick or untick a style; the last one stays ticked.
    pub fn toggle_style(&mut self, l: Look) {
        let mut v = self.styles_list();
        if let Some(i) = v.iter().position(|x| *x == l) {
            if v.len() > 1 {
                v.remove(i);
            }
        } else {
            v.push(l);
        }
        self.look = v[0];
        self.styles = v;
    }

    /// Realistic pictures: the Photorealistic style is among the styles.
    pub fn is_realistic(&self) -> bool {
        self.styles_list().contains(&Look::Photo)
    }

    /// The Realistic pictures checkbox. Ticked, it makes Photorealistic the style (added to the others when the person had
    /// ticked more than one); unticked, the styles from before come back.
    pub fn set_realistic(&mut self, on: bool) {
        if on == self.is_realistic() {
            return;
        }
        if on {
            let before = self.styles_list();
            self.before_realistic = Some(before.clone());
            self.styles = if before.len() > 1 { [before, vec![Look::Photo]].concat() } else { vec![Look::Photo] };
        } else {
            let mut back = self.before_realistic.take().unwrap_or_default();
            back.retain(|l| *l != Look::Photo);
            self.styles = if back.is_empty() { vec![Look::Storybook] } else { back };
        }
        self.look = self.styles[0];
    }

    /// Seed of variant `n` (from 0) of a scene whose base seed is `base`: the same for every style of the scene so the
    /// composition stays alike where the server allows, different for each further picture of a style.
    pub fn variant_seed(base: u64, n: u32) -> u64 {
        (base + n as u64 * 7919) % 2_147_483_647
    }
}

impl PictureOpts {
    /// The time limit for one picture in minutes (15 unless the person chose another).
    pub fn limit_min(&self) -> u32 {
        if self.timeout_min == 0 { 15 } else { self.timeout_min }
    }
}

/// A random seed (a new one each time), below 2^31.
pub fn fresh_seed(salt: u64) -> u64 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut z = n ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % 2_147_483_647
}

/// The seed of one picture: random each time, unless the story keeps one look (then its seed, `fresh` when it has none yet).
pub fn seed_for(o: &PictureOpts, fresh: u64) -> u64 {
    if o.keep_look { o.seed.unwrap_or(fresh) } else { fresh }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_ids_round_trip_and_cycle() {
        for c in Coverage::ALL {
            assert_eq!(Coverage::from_id(c.id()), Some(*c));
        }
        assert_eq!(Coverage::Chapters.next(), Coverage::Cover);
        assert_eq!(Coverage::Ask.next(), Coverage::Chapters);
        assert_eq!(Coverage::from_id(" COVER "), Some(Coverage::Cover));
        assert_eq!(Coverage::from_id("nope"), None);
    }

    #[test]
    fn styles_never_empty_and_the_last_one_stays_ticked() {
        let mut o = PictureOpts::default();
        assert_eq!(o.styles_list(), vec![Look::Storybook]);
        o.toggle_style(Look::Photo);
        assert_eq!(o.styles_list(), vec![Look::Storybook, Look::Photo]);
        o.toggle_style(Look::Storybook);
        o.toggle_style(Look::Photo);
        assert_eq!(o.styles_list(), vec![Look::Photo]);
    }

    #[test]
    fn realistic_switch_restores_the_earlier_styles() {
        let mut o = PictureOpts::default();
        o.toggle_style(Look::Anime);
        o.set_realistic(true);
        assert!(o.is_realistic());
        o.set_realistic(false);
        assert!(!o.is_realistic());
        assert_eq!(o.styles_list(), vec![Look::Storybook, Look::Anime]);
    }

    #[test]
    fn counts_limits_and_seeds_stay_in_range() {
        let mut o = PictureOpts::default();
        assert_eq!(o.count(), 1);
        o.next_count();
        o.next_count();
        o.next_count();
        assert_eq!(o.count(), 1);
        assert_eq!(o.limit_min(), 15);
        o.timeout_min = 30;
        assert_eq!(o.limit_min(), 30);
        assert!(fresh_seed(7) < 2_147_483_647);
        o.keep_look = true;
        o.seed = Some(5);
        assert_eq!(seed_for(&o, 9), 5);
        o.keep_look = false;
        assert_eq!(seed_for(&o, 9), 9);
        assert_ne!(PictureOpts::variant_seed(10, 0), PictureOpts::variant_seed(10, 1));
    }

    #[test]
    fn model_choice_json_shape_is_stable() {
        let m = ModelChoice { provider: "p".into(), model: "m".into() };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"provider":"p","model":"m"}"#);
    }
}
