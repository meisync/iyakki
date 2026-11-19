

/// What came out: the file, its pages, and how many pictures went in or were left out (videos, undecodable files).
pub struct StoryPdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub pictures: usize,
    pub left_out: usize,
}

/// At most this many pictures are put in one PDF (a story has at most six scenes).
pub const MAX_PICTURES: usize = 12;
