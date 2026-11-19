//! A story as a PDF (TALES, F-TALES-22): the title, a line saying who it is for, the chapters and paragraphs, and the illustrations
//! in their places (the first one right under the title, so it serves as the cover). Written directly with the in-house PDF writer
//! (`iyakki-pdf`), not through the model. A story in a script the writer cannot write yet (Tamil, Hindi, Arabic, Chinese, ...) is
//! refused with a plain message; nothing damaged is produced (docs/PDF.md).

pub use iyakki_shared::storypdf::{MAX_PICTURES, StoryPdf};
use crate::pdftools::{decode_picture, finish_pdf};
use crate::story::{title_from_topic, Block as SBlock, Story, StoredPic, StoryOptions};
use iyakki_pdf::{md::fold, Block, PageSize};



pub fn story_title(story: &Story, o: &StoryOptions) -> String {
    if story.title.trim().is_empty() {
        title_from_topic(&o.topic)
    } else {
        story.title.trim().to_string()
    }
}

/// The blocks of the story with its pictures; the second number counts pictures that could not be used.
pub fn story_blocks(story: &Story, o: &StoryOptions, pics: &[StoredPic]) -> (Vec<Block>, usize, usize) {
    let title = story_title(story, o);
    let mut out = vec![Block::Heading(1, fold(&title))];
    let meta = format!("A {} story for {}, in {}", o.genre.label().to_lowercase(), o.audience.label().to_lowercase(), if o.language.trim().is_empty() { "English" } else { o.language.trim() });
    out.push(Block::Quote(fold(&meta)));
    let base = story.unit_base();
    let (mut used, mut left_out) = (0usize, 0usize);
    let mut picture = |out: &mut Vec<Block>, p: &StoredPic| {
        if used >= MAX_PICTURES || matches!(p.ext.as_str(), "mp4" | "webm") {
            left_out += 1;
            return;
        }
        match decode_picture(&p.bytes) {
            Some(img) => {
                out.push(Block::Image(img));
                used += 1;
            }
            None => left_out += 1,
        }
    };
    // pictures that belong before the first block come right after the title: the cover
    for p in pics.iter().filter(|p| p.unit <= base) {
        picture(&mut out, p);
    }
    for (i, b) in story.blocks.iter().enumerate() {
        if i > 0 {
            for p in pics.iter().filter(|p| p.unit == base + i) {
                picture(&mut out, p);
            }
        }
        match b {
            SBlock::Heading(h) => out.push(Block::Heading(2, fold(h))),
            SBlock::Para(p) => out.push(Block::Para(fold(p))),
        }
    }
    (out, used, left_out)
}

pub fn story_pdf(story: &Story, o: &StoryOptions, pics: &[StoredPic], size: PageSize, date: Option<String>) -> Result<StoryPdf, String> {
    if story.blocks.is_empty() {
        return Err("There is no story to put in a PDF yet.".into());
    }
    let (blocks, pictures, left_out) = story_blocks(story, o, pics);
    let (bytes, pages) = finish_pdf(&fold(&story_title(story, o)), "IYAKKI TALES", size, blocks, date).map_err(|e| format!("The PDF could not be made: {e}"))?;
    Ok(StoryPdf { bytes, pages, pictures, left_out })
}

/// Does the person's message ask for the story as a PDF ("make it a PDF", "save this as pdf", "export to PDF please")? A message that
/// only mentions the word in a correction ("add a pdf-reading owl") is not a request: it needs a verb that makes or saves a file.
pub fn asks_for_pdf(text: &str) -> bool {
    let t = text.to_lowercase();
    if !t.split(|c: char| !c.is_alphanumeric()).any(|w| w == "pdf") {
        return false;
    }
    ["make", "save", "export", "create", "give me", "produce", "generate", "turn", "convert", "download", "print", "as a pdf", "to pdf", "into a pdf", "in pdf"].iter().any(|v| t.contains(v)) && !t.contains("owl") && t.split_whitespace().count() <= 14
}
