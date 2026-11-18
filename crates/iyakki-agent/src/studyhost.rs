//! The installation as the lesson writer sees it (`iyakki_shared::studylesson::LessonHost`).

use crate::services::Services;
use iyakki_shared::study::StudyStore;

impl iyakki_shared::studylesson::LessonHost for Services {
    fn study(&self) -> &StudyStore {
        &self.study
    }
    fn has_image_model(&self) -> bool {
        !self.image_models().is_empty()
    }
    fn generate_image(&self, prompt: &str, size: &str) -> Result<Vec<u8>, String> {
        self.generate_image(None, prompt, size).map(|(_, _, bytes, _)| bytes)
    }
}
