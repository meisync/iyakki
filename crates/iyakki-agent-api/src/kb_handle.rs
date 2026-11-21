//! The knowledge base of the services handle as a source for the workspace history (F-SPLIT-03): `History` asks a `KbSource`, the handle serves a
//! `KnowledgeBaseApi` (which may be a connection to the core), this struct is the small adapter between the two.

use std::sync::Arc;

use iyakki_memx::kb::{DocInfo, KbError, KbSource};
use iyakki_shared::kb::Hit;

use crate::KnowledgeBaseApi;

pub struct KbHandle(pub Arc<dyn KnowledgeBaseApi>);

impl KbSource for KbHandle {
    fn namespaces(&self) -> Vec<String> {
        self.0.namespaces()
    }
    fn docs(&self, ns: &str) -> Vec<DocInfo> {
        self.0.docs(ns)
    }
    fn add_document(&self, ns: &str, title: &str, source: &str, text: &str, meta: std::collections::BTreeMap<String, String>) -> Result<(String, bool), KbError> {
        self.0.add_document(ns, title, source, text, meta)
    }
    fn remove_document(&self, ns: &str, doc_id: &str) -> Result<(), KbError> {
        self.0.remove_document(ns, doc_id)
    }
    fn drop_namespace(&self, ns: &str) -> Result<(), KbError> {
        self.0.drop_namespace(ns)
    }
    fn search_weighted(&self, namespaces: &[(String, f32)], query: &str, k: usize) -> Vec<Hit> {
        self.0.search_weighted_in(namespaces, query, k)
    }
}
