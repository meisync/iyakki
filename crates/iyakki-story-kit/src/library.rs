//! The TALES library and what is remembered: the stories of one account on disk (`StoryStore`, every path checked against the
//! account's scopes), the conversation and preferences files, and the choice of model. Pure leaf code over the story, chat, scene and
//! picture-log crates (alpha.11 AP1, D-P6-03: `StoryStore` is a leaf crate type, not a trait, because a tile crate may name it).
//! `iyakki_shared::story` re-exports every item.

use std::path::{Path, PathBuf};

use iyakki_chat_model::Message;
use iyakki_sec::policy::Policy;
use iyakki_story_model::*;
use serde::{Deserialize, Serialize};

use crate::{split_audio, storyillus, valid_pic_file, valid_var_file};

/// Which model a TALES run uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelPlan {
    /// The remembered choice, still among the `available` models.
    Chosen(ModelChoice),
    /// The model CHAT uses; `note` says why when a remembered choice could not be used.
    SameAsChat { note: Option<String> },
}


// ------------------------------------------------------------------------------------------------------------ the library


/// A saved story opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryDoc {
    pub entry: StoryEntry,
    pub story: Story,
}

/// The sidecar `audio.json` of a saved sound: the pieces and where each starts, so the saved sound can be heard again
/// piece by piece with the page following along, with no speech engine needed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AudioIndex {
    pieces: Vec<(usize, usize, usize, String, f32)>,
}

/// `pics/index.json`: what each picture file is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PicIndex {
    pics: Vec<PicMeta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PicMeta {
    #[serde(default)]
    scene_text: String,
    file: String,
    unit: usize,
    title: String,
    prompt: String,
    style: String,
    mode: String,
    /// `svg`, `image` or `video`, what made it, and when it was saved (seconds since 1970).
    #[serde(default)]
    kind: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    saved_at: u64,
    /// The other pictures of this scene (`01-v1.png`, ...); the file above is the main one.
    #[serde(default)]
    variants: Vec<VarMeta>,
    /// How the main picture was made.
    #[serde(default)]
    look: String,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct VarMeta {
    file: String,
    #[serde(default)]
    style: String,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    ms: u64,
    #[serde(default)]
    prompt: String,
}

/// The conversation kept beside a story: which version is in use and the messages of the panel (the person's requests, the
/// steps and what they did). Written by the shell; read back when the story is opened from Listen again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StoryChat {
    pub version: u32,
    pub messages: Vec<Message>,
}

/// What is remembered between visits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub voice: Option<String>,
    pub speed: f32,
    pub read_while_writing: bool,
    /// Save each story (text and pictures) as soon as it is written.
    pub auto_save: bool,
    pub options: StoryOptions,
    /// The model that writes the stories (and draws SVG pictures); `None` is "same as chat".
    pub model: Option<ModelChoice>,
    /// The model that draws pictures when the picture source is a picture model; `None` is "the first one found".
    pub picture_model: Option<ModelChoice>,
    /// An external agent (`claude-code`, `copilot`, `custom:<id>`) that writes the stories; `None` is our own model.
    pub agent: Option<String>,
    /// The model chosen with that agent (empty: the agent's own).
    pub agent_model: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { voice: None, speed: 1.0, read_while_writing: true, auto_save: true, options: StoryOptions::default(), model: None, picture_model: None, agent: None, agent_model: String::new() }
    }
}


/// Decide the model of a run: the remembered `choice` when its provider is still enabled and still lists the model,
/// otherwise CHAT's model with a note that says what happened (the choice itself is kept, so it works again when the
/// provider comes back). `what` is "model" or "picture model" for the note.
pub fn plan_model(choice: Option<&ModelChoice>, available: &[(String, String)], what: &str) -> ModelPlan {
    match choice {
        None => ModelPlan::SameAsChat { note: None },
        Some(c) if available.iter().any(|(p, m)| *p == c.provider && *m == c.model) => ModelPlan::Chosen(c.clone()),
        Some(c) => ModelPlan::SameAsChat { note: Some(format!("The {what} you chose ({} on {}) is not available any more, so the chat model is used. Choose another under Model.", c.model, c.provider)) },
    }
}

/// The stories of one account, one folder each: `<home>/Stories/<id>/story.md` (the text, with picture lines),
/// `pics/` (the pictures and `index.json` with the prompt of each) and `audio.wav` with `audio.json`. Every path is
/// resolved (links followed) and checked against the account's read and write scopes before it is touched.
#[derive(Clone)]
pub struct StoryStore {
    pub dir: PathBuf,
    pub user: String,
    pub policy: Policy,
    pub roots: iyakki_filescope::FileRoots,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A file name from a title: lower-case ASCII letters and digits with dashes ("The Brave Fox!" is "the-brave-fox").
pub fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(48).collect();
    s.trim_matches('-').to_string()
}

pub fn front_matter(e: &StoryEntry) -> String {
    let l = |s: &str| one_line(s);
    format!(
        "---\ntitle: {}\ncreated: {}\nfavourite: {}\ntopic: {}\naudience: {}\ngenre: {}\nlength: {}\nlanguage: {}\ntone: {}\ncharacters: {}\n---\n\n",
        l(&e.title),
        e.created,
        if e.favourite { "yes" } else { "no" },
        l(&e.topic),
        l(&e.audience),
        l(&e.genre),
        l(&e.length),
        l(&e.language),
        l(&e.tone),
        l(&e.characters)
    )
}

/// Split a saved file into its front matter fields and its body.
pub fn split_front(text: &str) -> (Vec<(String, String)>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else { return (Vec::new(), text) };
    let Some(end) = rest.find("\n---") else { return (Vec::new(), text) };
    let fields = rest[..end].lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    let body = rest[end + 4..].trim_start_matches(['-', '\n']);
    (fields, body)
}

impl StoryStore {
    /// The store of `user` whose home folder is `home`, saving only inside `roots` (the agent works out both: `iyakki_agent::storykit::open`).
    pub fn open_with(policy: &Policy, user: &str, home: PathBuf, roots: iyakki_filescope::FileRoots) -> StoryStore {
        StoryStore { dir: home.join("Stories"), user: user.to_string(), policy: policy.clone(), roots }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The bytes of every picture file of every story (the global picture cap looks at this).
    pub fn pictures_bytes(&self) -> u64 {
        self.ids()
            .iter()
            .filter_map(|id| self.check_read(&self.dir.join(id).join("pics")).ok())
            .filter_map(|p| std::fs::read_dir(p).ok())
            .flat_map(|rd| rd.flatten())
            .filter_map(|e| e.metadata().ok().filter(|m| m.is_file()).map(|m| m.len()))
            .sum()
    }

    /// The folder of one story.
    pub fn story_dir(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("not a story name".into());
        }
        Ok(self.dir.join(id))
    }

    /// May this path be written (links followed, even for a file that does not exist yet)?
    fn check_write(&self, p: &Path) -> Result<PathBuf, String> {
        let real = iyakki_filescope::resolve_for_write(Path::new("/"), &p.to_string_lossy())?;
        if !self.roots.contains(&real) || !self.roots.write_ok(&self.policy, &self.user, &real) {
            return Err(format!("Your account may not save into {} (it is outside the folders you may write to).", self.dir.display()));
        }
        Ok(real)
    }

    fn check_read(&self, p: &Path) -> Result<PathBuf, String> {
        let real = iyakki_filescope::resolve(Path::new("/"), &p.to_string_lossy()).map_err(|_| format!("{} cannot be read", p.display()))?;
        if !self.roots.read_ok(&self.policy, &self.user, &real) {
            return Err(format!("Your account may not read {}.", p.display()));
        }
        Ok(real)
    }

    fn md_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.story_dir(id)?.join("story.md"))
    }

    /// The saved sound of a story, if any (`.wav` or `.mp3`).
    pub fn audio_path(&self, id: &str) -> Option<PathBuf> {
        let d = self.story_dir(id).ok()?;
        ["wav", "mp3"].iter().map(|e| d.join(format!("audio.{e}"))).find(|p| p.is_file() && self.check_read(p).is_ok())
    }

    /// The saved PDF of a story, if any (`story.pdf`, beside `story.md`).
    pub fn pdf_path(&self, id: &str) -> Option<PathBuf> {
        let p = self.story_dir(id).ok()?.join("story.pdf");
        (p.is_file() && self.check_read(&p).is_ok()).then_some(p)
    }

    /// Keep the PDF of a story beside its text (`story.pdf`; an earlier copy of it is replaced: it is made from the story, not
    /// written by the person). The story must be saved first. Returns the path and whether an earlier copy was replaced.
    pub fn save_pdf(&self, id: &str, bytes: &[u8]) -> Result<(PathBuf, bool), String> {
        let dir = self.story_dir(id)?;
        self.check_read(&self.md_path(id)?).and_then(|p| if p.is_file() { Ok(p) } else { Err("save the story first".to_string()) })?;
        let target = self.check_write(&dir.join("story.pdf"))?;
        let replaced = target.is_file();
        let tmp = self.check_write(&dir.join("story.pdf.part"))?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("could not save the PDF: {e}"))?;
        std::fs::rename(&tmp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("could not save the PDF: {e}")
        })?;
        Ok((target, replaced))
    }

    fn entry_from(&self, id: &str, text: &str) -> (StoryEntry, Story) {
        let (fields, body) = split_front(text);
        let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        let story = parse_story(body, true);
        let title = if get("title").is_empty() { if story.title.is_empty() { id.to_string() } else { story.title.clone() } } else { get("title") };
        let mut story = story;
        if story.title.is_empty() {
            story.title = title.clone();
        }
        let e = StoryEntry {
            id: id.to_string(),
            title,
            created: get("created").parse().unwrap_or(0),
            favourite: matches!(get("favourite").as_str(), "yes" | "true"),
            topic: get("topic"),
            audience: get("audience"),
            genre: get("genre"),
            length: get("length"),
            language: get("language"),
            tone: get("tone"),
            characters: get("characters"),
            words: story.words(),
            has_audio: self.audio_path(id).is_some(),
            pictures: self.pic_index(id).map_or(0, |i| i.pics.len()),
        };
        (e, story)
    }

    fn read_doc(&self, id: &str) -> Result<StoryDoc, String> {
        let p = self.check_read(&self.md_path(id)?)?;
        let text = std::fs::read_to_string(&p).map_err(|_| format!("The story \"{id}\" could not be opened."))?;
        let (entry, story) = self.entry_from(id, &text);
        Ok(StoryDoc { entry, story })
    }

    /// Save a finished story into a new folder of its own and return its entry. `now` is the time in seconds since 1970.
    pub fn save(&self, story: &Story, o: &StoryOptions, now: u64) -> Result<StoryEntry, String> {
        if story.blocks.is_empty() {
            return Err("There is no story to save yet.".into());
        }
        self.check_write(&self.dir)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("could not make {}: {e}", self.dir.display()))?;
        let real = self.check_write(&self.dir)?;
        let title = if story.title.is_empty() { o.topic.chars().take(60).collect::<String>() } else { story.title.clone() };
        let base = match slug(&title) {
            s if s.is_empty() => format!("story-{now}"),
            s => s,
        };
        let entry = StoryEntry {
            id: String::new(),
            title: title.clone(),
            created: now,
            favourite: false,
            topic: o.topic.clone(),
            audience: o.audience.id().into(),
            genre: o.genre.id().into(),
            length: o.length.id().into(),
            language: o.language.clone(),
            tone: o.tone.id().into(),
            characters: o.characters.clone(),
            words: story.words(),
            has_audio: false,
            pictures: 0,
        };
        let mut st = story.clone();
        st.title = title;
        let text = format!("{}{}", front_matter(&entry), st.markdown());
        for n in 1..1000 {
            let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            let folder = real.join(&id);
            self.check_write(&folder)?;
            match std::fs::create_dir(&folder) {
                Ok(()) => {
                    let path = folder.join("story.md");
                    iyakki_schema::write_atomic(&path, text.as_bytes(), 0o644).map_err(|e| format!("could not save {}: {e}", path.display()))?;
                    // the first version is kept beside it, so a later revision never loses it
                    let v1 = folder.join("story.v1.md");
                    iyakki_schema::write_atomic(&v1, text.as_bytes(), 0o644).map_err(|e| format!("could not save {}: {e}", v1.display()))?;
                    return Ok(StoryEntry { id, ..entry });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("could not save {}: {e}", folder.display())),
            }
        }
        Err("too many stories with that name".into())
    }

    /// All saved stories, newest first.
    pub fn list(&self) -> Vec<StoryEntry> {
        let mut v: Vec<StoryEntry> = self.ids().into_iter().filter_map(|id| self.read_doc(&id).ok().map(|d| d.entry)).collect();
        v.sort_by(|a, b| b.created.cmp(&a.created).then(a.id.cmp(&b.id)));
        v
    }

    fn ids(&self) -> Vec<String> {
        if self.check_read(&self.dir).is_err() {
            return Vec::new();
        }
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|id| valid_id(id)).collect()
    }

    /// Stories whose title, topic, genre, language or text contain every word of `query` (any case).
    pub fn search(&self, query: &str) -> Vec<StoryEntry> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        if words.is_empty() {
            return self.list();
        }
        self.list()
            .into_iter()
            .filter(|e| {
                let body = self.read_doc(&e.id).map(|d| d.story.plain()).unwrap_or_default();
                let hay = format!("{} {} {} {} {} {}", e.title, e.topic, e.genre, e.language, e.characters, body).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    pub fn load(&self, id: &str) -> Result<StoryDoc, String> {
        self.read_doc(id)
    }

    /// The story's file as it is stored: the header, the text and the picture lines.
    pub fn raw_markdown(&self, id: &str) -> Result<String, String> {
        let p = self.check_read(&self.md_path(id)?)?;
        std::fs::read_to_string(p).map_err(|_| format!("The story \"{id}\" could not be opened."))
    }

    /// Change the header of a story's file and leave its text (and picture lines) as they are.
    fn rewrite_front(&self, id: &str, f: impl FnOnce(&mut StoryEntry)) -> Result<(), String> {
        let p = self.check_read(&self.md_path(id)?)?;
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let (mut e, _) = self.entry_from(id, &text);
        let (_, body) = split_front(&text);
        f(&mut e);
        let w = self.check_write(&p)?;
        iyakki_schema::write_atomic(&w, format!("{}{}", front_matter(&e), body).as_bytes(), 0o644).map_err(|e| format!("could not save: {e}"))
    }

    pub fn set_favourite(&self, id: &str, on: bool) -> Result<(), String> {
        self.rewrite_front(id, |e| e.favourite = on)
    }

    /// Remove the story with its pictures and its sound.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let dir = self.story_dir(id)?;
        let w = self.check_write(&dir)?;
        // only a folder directly inside Stories, and only what this program makes in it
        if w.parent() != self.check_write(&self.dir).ok().as_deref() || !w.is_dir() {
            return Err("that is not a story folder".into());
        }
        let _ = std::fs::remove_dir_all(w.join("pics"));
        let mut files: Vec<String> = ["story.md", "audio.wav", "audio.mp3", "audio.json", "chat.json", "character.json", "pictures.log", "picture-options.json", "setting.txt", "story.pdf", "story.pdf.part"].iter().map(|f| f.to_string()).collect();
        files.extend((1..=self.versions(id)).map(|n| format!("story.v{n}.md")));
        files.extend(iyakki_piclog::file_names());
        for f in files {
            let p = w.join(f);
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| format!("could not delete the story: {e}"))?;
            }
        }
        let _ = std::fs::remove_dir(&w); // only when nothing else is in it
        Ok(())
    }

    /// Keep the sound next to the story (`audio.wav`, or `audio.mp3` when that is what the engine made).
    pub fn save_audio(&self, id: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let dir = self.story_dir(id)?;
        if !self.check_read(&self.md_path(id)?).is_ok_and(|p| p.is_file()) {
            return Err("save the story first".into());
        }
        let ext = if bytes.starts_with(b"ID3") || bytes.starts_with(&[0xFF, 0xFB]) { "mp3" } else { "wav" };
        let path = self.check_write(&dir.join(format!("audio.{ext}")))?;
        std::fs::write(&path, bytes).map_err(|e| format!("could not save the audio: {e}"))?;
        Ok(path)
    }

    /// Keep the sound and, beside it, where each piece of the story starts in it.
    pub fn save_audio_indexed(&self, id: &str, wav: &[u8], chunks: &[Chunk], starts: &[f32]) -> Result<PathBuf, String> {
        let path = self.save_audio(id, wav)?;
        let idx = AudioIndex { pieces: chunks.iter().zip(starts).map(|(c, t)| (c.unit, c.start, c.end, c.text.clone(), *t)).collect() };
        let json = self.check_write(&self.story_dir(id)?.join("audio.json"))?;
        std::fs::write(json, serde_json::to_string(&idx).map_err(|e| e.to_string())?).map_err(|e| format!("could not save the audio index: {e}"))?;
        Ok(path)
    }

    /// The saved sound cut into the pieces of the story, ready to play one by one. Fails when there is no saved sound, or
    /// it has no index (an `.mp3` from an engine, or a file put there by hand).
    pub fn load_audio_pieces(&self, id: &str) -> Result<Vec<(Chunk, Vec<u8>)>, String> {
        let dir = self.story_dir(id)?;
        let real = self.check_read(&dir.join("audio.wav")).ok().filter(|p| p.is_file()).ok_or_else(|| "There is no saved audio for this story.".to_string())?;
        let no_index = || "The saved audio has no index, so it cannot follow the text.".to_string();
        let idx_path = self.check_read(&dir.join("audio.json")).map_err(|_| no_index())?;
        let idx: AudioIndex = serde_json::from_str(&std::fs::read_to_string(idx_path).map_err(|_| no_index())?).map_err(|_| "The audio index is damaged.".to_string())?;
        let wav = std::fs::read(real).map_err(|e| e.to_string())?;
        let starts: Vec<f32> = idx.pieces.iter().map(|p| p.4).collect();
        let parts = split_audio(&wav, &starts)?;
        Ok(idx.pieces.into_iter().zip(parts).map(|((unit, start, end, text, _), w)| (Chunk { unit, start, end, text }, w)).collect())
    }

    fn pic_index(&self, id: &str) -> Option<PicIndex> {
        let p = self.check_read(&self.story_dir(id).ok()?.join("pics/index.json")).ok()?;
        serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
    }

    /// Keep the pictures of a story (`pics/01.svg`, …, and `pics/index.json` with the prompt used for each), replacing the ones
    /// kept before, and put a picture line before the paragraph each belongs to in `story.md`.
    pub fn save_pictures(&self, id: &str, pics: &[StoredPic]) -> Result<(), String> {
        let dir = self.story_dir(id)?;
        let md = self.check_read(&self.md_path(id)?)?;
        let text = std::fs::read_to_string(&md).map_err(|_| "save the story first".to_string())?;
        let pdir = self.check_write(&dir.join("pics"))?;
        if pdir.exists() {
            let _ = std::fs::remove_dir_all(&pdir);
        }
        let mut metas = Vec::new();
        if !pics.is_empty() {
            std::fs::create_dir_all(&pdir).map_err(|e| format!("could not make {}: {e}", pdir.display()))?;
            let pdir = self.check_write(&pdir)?;
            for (i, p) in pics.iter().enumerate() {
                let ext = if matches!(p.ext.as_str(), "svg" | "png" | "jpg" | "webp" | "gif" | "mp4" | "webm") { p.ext.as_str() } else { "png" };
                let file = format!("{:02}.{ext}", i + 1);
                std::fs::write(self.check_write(&pdir.join(&file))?, &p.bytes).map_err(|e| format!("could not save a picture: {e}"))?;
                let kind = match ext {
                    "svg" => "svg",
                    "mp4" | "webm" => "video",
                    _ => "image",
                };
                let mut vmeta = Vec::new();
                for (k, v) in p.variants.iter().enumerate() {
                    let vext = if matches!(v.ext.as_str(), "svg" | "png" | "jpg" | "webp" | "gif") { v.ext.as_str() } else { "png" };
                    let vfile = format!("{:02}-v{}.{vext}", i + 1, k + 1);
                    std::fs::write(self.check_write(&pdir.join(&vfile))?, &v.bytes).map_err(|e| format!("could not save a picture: {e}"))?;
                    vmeta.push(VarMeta { file: vfile, style: v.style.clone(), seed: v.seed, provider: v.provider.clone(), model: v.model.clone(), size: v.size.clone(), ms: v.ms, prompt: v.prompt.clone() });
                }
                metas.push(PicMeta { variants: vmeta, look: p.look.clone(), seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), ms: p.ms, scene_text: p.scene_text.clone(), file, unit: p.unit, title: p.title.clone(), prompt: p.prompt.clone(), style: p.style.clone(), mode: p.mode.clone(), kind: kind.into(), source: p.source.clone(), size: p.size.clone(), saved_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) });
            }
            let idx = PicIndex { pics: metas.clone() };
            std::fs::write(self.check_write(&pdir.join("index.json"))?, serde_json::to_string_pretty(&idx).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        // the text, with picture lines
        let (e, story) = self.entry_from(id, &text);
        let refs: Vec<(usize, String, String)> = metas.iter().map(|m| (m.unit, m.title.clone(), format!("pics/{}", m.file))).collect();
        let w = self.check_write(&md)?;
        iyakki_schema::write_atomic(&w, format!("{}{}", front_matter(&e), story.markdown_with_pictures(&refs)).as_bytes(), 0o644).map_err(|e| format!("could not save: {e}"))
    }

    /// Keep what a story's pictures were made from beside it: `character.json` (the character sheet) and `pictures.log` (what was
    /// asked of the model for each picture, what it answered, and what was used), so a poor picture can be explained.
    pub fn save_notes(&self, id: &str, character: Option<&str>, log: &str) -> Result<(), String> {
        let dir = self.story_dir(id)?;
        if let Some(c) = character {
            std::fs::write(self.check_write(&dir.join("character.json"))?, c).map_err(|e| format!("could not save the character sheet: {e}"))?;
        }
        if !log.is_empty() {
            std::fs::write(self.check_write(&dir.join("pictures.log"))?, log).map_err(|e| format!("could not save the picture log: {e}"))?;
        }
        Ok(())
    }

    /// Keep the setting note of a story (`setting.txt`: where and when it happens, one line) so every picture of it looks alike.
    pub fn save_setting(&self, id: &str, setting: &str) -> Result<(), String> {
        let dir = self.story_dir(id)?;
        std::fs::write(self.check_write(&dir.join("setting.txt"))?, setting).map_err(|e| format!("could not save the setting note: {e}"))
    }

    /// Keep the picture choices of a story (`picture-options.json`: look, quality, model, same-look seed).
    pub fn save_picture_opts(&self, id: &str, o: &storyillus::PictureOpts) -> Result<(), String> {
        let dir = self.story_dir(id)?;
        std::fs::write(self.check_write(&dir.join("picture-options.json"))?, serde_json::to_string_pretty(o).unwrap_or_default()).map_err(|e| format!("could not save the picture options: {e}"))
    }

    /// The picture choices kept for a story, `None` for a story saved before they existed.
    pub fn load_picture_opts(&self, id: &str) -> Option<storyillus::PictureOpts> {
        let p = self.check_read(&self.story_dir(id).ok()?.join("picture-options.json")).ok()?;
        serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
    }

    /// Add events to the picture log of a story (`pictures.jsonl`, 1 MB a file, the newest three files kept).
    pub fn append_picture_log(&self, id: &str, events: &[iyakki_piclog::PicEvent]) -> Result<(), String> {
        let dir = self.check_write(&self.story_dir(id)?)?;
        iyakki_piclog::append(&dir, events).map_err(|e| format!("could not save the picture log: {e}"))
    }

    /// The picture log of a story, oldest first.
    pub fn load_picture_log(&self, id: &str) -> Vec<iyakki_piclog::PicEvent> {
        self.story_dir(id).ok().and_then(|d| self.check_read(&d).ok()).map(|d| iyakki_piclog::read_all(&d)).unwrap_or_default()
    }

    /// Delete the picture log of a story.
    pub fn clear_picture_log(&self, id: &str) {
        if let Some(d) = self.story_dir(id).ok().and_then(|d| self.check_write(&d).ok()) {
            iyakki_piclog::clear(&d);
        }
    }

    /// The setting note kept for a story.
    pub fn load_setting(&self, id: &str) -> Option<String> {
        let p = self.check_read(&self.story_dir(id).ok()?.join("setting.txt")).ok()?;
        std::fs::read_to_string(p).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// The character sheet kept for a story.
    pub fn load_character(&self, id: &str) -> Option<iyakki_scene::CharacterSheet> {
        let dir = self.story_dir(id).ok()?;
        let p = self.check_read(&dir.join("character.json")).ok()?;
        iyakki_scene::CharacterSheet::from_json(&std::fs::read_to_string(p).ok()?)
    }

    /// The pictures kept for a story.
    pub fn load_pictures(&self, id: &str) -> Vec<StoredPic> {
        let Some(idx) = self.pic_index(id) else { return Vec::new() };
        let Ok(dir) = self.story_dir(id) else { return Vec::new() };
        idx.pics
            .into_iter()
            .filter(|m| valid_pic_file(&m.file))
            .filter_map(|m| {
                let p = self.check_read(&dir.join("pics").join(&m.file)).ok()?;
                let bytes = std::fs::read(p).ok()?;
                let ext = m.file.rsplit('.').next().unwrap_or("png").to_string();
                let variants = m
                    .variants
                    .iter()
                    .filter(|v| valid_var_file(&v.file))
                    .filter_map(|v| {
                        let vp = self.check_read(&dir.join("pics").join(&v.file)).ok()?;
                        Some(StoredVariant { ext: v.file.rsplit('.').next().unwrap_or("png").to_string(), bytes: std::fs::read(vp).ok()?, style: v.style.clone(), seed: v.seed, provider: v.provider.clone(), model: v.model.clone(), size: v.size.clone(), ms: v.ms, prompt: v.prompt.clone() })
                    })
                    .collect();
                Some(StoredPic { variants, look: m.look, seed: m.seed, provider: m.provider, model: m.model, ms: m.ms, size: m.size, source: m.source, scene_text: m.scene_text, unit: m.unit, title: m.title, prompt: m.prompt, style: m.style, mode: m.mode, ext, bytes })
            })
            .collect()
    }

    // ---- versions and the conversation

    /// How many versions are kept (`story.v1.md`, `story.v2.md`, … without a gap). `story.md` is the one in use.
    pub fn versions(&self, id: &str) -> u32 {
        let Ok(dir) = self.story_dir(id) else { return 0 };
        (1..200u32).take_while(|n| dir.join(format!("story.v{n}.md")).is_file()).last().unwrap_or(0)
    }

    fn version_path(&self, id: &str, n: u32) -> Result<PathBuf, String> {
        if n == 0 || n > 999 {
            return Err("there is no such version".into());
        }
        Ok(self.story_dir(id)?.join(format!("story.v{n}.md")))
    }

    /// Keep a revised story as the next version and make it the one in use; the earlier versions stay as they are. The header
    /// (created, favourite, topic) is kept, the title and the words follow the new text. Returns the new version's number.
    pub fn save_revision(&self, id: &str, story: &Story, o: &StoryOptions) -> Result<u32, String> {
        if story.blocks.is_empty() {
            return Err("There is no story to save yet.".into());
        }
        let cur = self.read_doc(id)?;
        let n = self.versions(id).max(1) + 1;
        let mut e = cur.entry;
        let mut st = story.clone();
        if st.title.is_empty() {
            st.title = e.title.clone();
        }
        e.title = st.title.clone();
        e.audience = o.audience.id().into();
        e.genre = o.genre.id().into();
        e.length = o.length.id().into();
        e.language = o.language.clone();
        e.tone = o.tone.id().into();
        let text = format!("{}{}", front_matter(&e), st.markdown());
        let vp = self.check_write(&self.version_path(id, n)?)?;
        iyakki_schema::write_atomic(&vp, text.as_bytes(), 0o644).map_err(|e| format!("could not save {}: {e}", vp.display()))?;
        let mp = self.check_write(&self.md_path(id)?)?;
        iyakki_schema::write_atomic(&mp, text.as_bytes(), 0o644).map_err(|e| format!("could not save: {e}"))?;
        Ok(n)
    }

    /// Make version `n` the one in use again (its text; the pictures are matched to it by the caller). Returns it.
    pub fn select_version(&self, id: &str, n: u32) -> Result<StoryDoc, String> {
        let vp = self.check_read(&self.version_path(id, n)?)?;
        let text = std::fs::read_to_string(&vp).map_err(|_| format!("Version {n} could not be opened."))?;
        // the header of the version, but the favourite mark and the date of the story as it is now
        let cur = self.read_doc(id)?;
        let (mut e, story) = self.entry_from(id, &text);
        e.favourite = cur.entry.favourite;
        e.created = cur.entry.created;
        let mp = self.check_write(&self.md_path(id)?)?;
        iyakki_schema::write_atomic(&mp, format!("{}{}", front_matter(&e), story.markdown()).as_bytes(), 0o644).map_err(|e| format!("could not save: {e}"))?;
        self.read_doc(id)
    }

    /// The conversation kept with a story (`chat.json`): the version in use and the messages shown beside the story.
    pub fn save_chat(&self, id: &str, chat: &StoryChat) -> Result<(), String> {
        let p = self.check_write(&self.story_dir(id)?.join("chat.json"))?;
        iyakki_schema::write_atomic(&p, serde_json::to_string_pretty(chat).map_err(|e| e.to_string())?.as_bytes(), 0o644).map_err(|e| format!("could not save the conversation: {e}"))
    }

    pub fn load_chat(&self, id: &str) -> Option<StoryChat> {
        let p = self.check_read(&self.story_dir(id).ok()?.join("chat.json")).ok()?;
        serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
    }

    pub fn load_prefs(&self) -> Prefs {
        self.check_read(&self.dir.join(".prefs.json")).ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_prefs(&self, p: &Prefs) -> Result<(), String> {
        self.check_write(&self.dir)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.check_write(&self.dir.join(".prefs.json"))?;
        iyakki_schema::write_atomic(&path, serde_json::to_string_pretty(p).map_err(|e| e.to_string())?.as_bytes(), 0o644)
    }
}


