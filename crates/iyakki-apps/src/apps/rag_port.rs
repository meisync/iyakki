//! The real port of the RAG tile (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): the knowledge base, the research kit, the model,
//! the settings, the documents folder and the audit log of the signed-in person. This is the only RAG code that names `Services`; the tile
//! (`crates/iyakki-tile-rag`) asks the port in `iyakki_tile_api::rag` and its tests use a fake.

use std::sync::mpsc::Sender;

use iyakki_shared::agenticrag::{Ledger, Mode};
use iyakki_shared::rag::{build_prompt, Passage};
use iyakki_tile_api::rag as t;

use super::AppCx;

fn run_view(l: &Ledger) -> t::RunView {
    t::RunView {
        run_id: l.run_id.clone(),
        agentic: l.mode == Mode::Agentic,
        summary: l.summary_line(),
        searches: l.searches,
        citations: l.citations.len(),
        stop: l.stop.text().to_string(),
        lines: l.lines(),
        answer: l.answer.clone(),
        passages: l.passages.iter().map(|p| t::PassageRow { id: p.id.clone(), collection: p.collection.clone(), source: p.source.clone(), title: p.title.clone(), text: p.text.clone(), score: p.score, backend: p.backend.clone() }).collect(),
        warnings: l.warnings.clone(),
    }
}

fn hit_row(h: iyakki_shared::kb::Hit) -> t::HitRow {
    t::HitRow { ns: h.ns, doc_id: h.doc_id, title: h.title, source: h.source, text: h.text, score: h.score, bm25: h.bm25, vector: h.vector }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    fn base(&self, ns: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.0.cfg.rag_docs).join(if ns == "shared" { "shared".to_string() } else { self.0.session.user.clone() })
    }
}

impl t::RagPort for CxPort<'_> {
    fn namespaces(&self) -> Vec<String> {
        vec![format!("user/{}", self.0.session.user), "shared".to_string()]
    }
    fn docs_folders(&self) -> String {
        format!("{}/{}  and  {}/shared", self.0.cfg.rag_docs, self.0.session.user, self.0.cfg.rag_docs)
    }
    fn external_backends(&self) -> usize {
        self.0.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().external_rag.iter().filter(|e| e.enabled).count()
    }
    fn index(&self, done: Sender<String>) {
        let svc = self.0.services.clone();
        let base = std::path::PathBuf::from(&self.0.cfg.rag_docs);
        let user = self.0.session.user.clone();
        std::thread::spawn(move || {
            let kb = svc.kb();
            let (a1, _) = kb.ingest_dir(&format!("user/{user}"), &base.join(&user));
            let (a2, _) = kb.ingest_dir("shared", &base.join("shared"));
            let docs: usize = ["shared".to_string(), format!("user/{user}")].iter().filter_map(|n| kb.stats(n)).map(|s| s.docs).sum();
            let _ = done.send(format!("{docs} documents indexed{}", if a1 + a2 > 0 { format!(" ({} new)", a1 + a2) } else { String::new() }));
        });
    }
    fn docs(&self) -> Vec<t::DocRow> {
        let kb = self.0.kb();
        let mut v = Vec::new();
        for ns in t::RagPort::namespaces(self).into_iter().rev() {
            for d in kb.docs(&ns) {
                v.push(t::DocRow { ns: ns.clone(), id: d.id, title: d.title, source: d.source, chunks: d.chunks });
            }
        }
        v
    }
    fn can_delete(&self, d: &t::DocRow) -> bool {
        d.ns == format!("user/{}", self.0.session.user) || (d.ns == "shared" && self.0.session.policy.manage_settings)
    }
    fn delete_doc(&self, d: &t::DocRow) -> Result<(), String> {
        self.0.kb().remove_document(&d.ns, &d.id).map_err(|e| e.to_string())?;
        // only a plain file inside the documents folder is touched
        let base = self.base(&d.ns);
        let file = base.join(&d.source);
        if let (Ok(b), Ok(f)) = (std::fs::canonicalize(&base), std::fs::canonicalize(&file)) {
            if f.starts_with(&b) && f.is_file() {
                let _ = std::fs::remove_file(&f);
            }
        }
        self.0.audit.record(&self.0.session.user, "rag.delete", &d.source);
        Ok(())
    }
    fn search(&self, q: &str, ask: bool, done: Sender<t::SearchOut>) {
        self.0.audit.record(&self.0.session.user, if ask { "rag.ask" } else { "rag.search" }, &format!("{} chars", q.len()));
        let (svc, backend, q) = (self.0.services.clone(), self.0.chat.clone(), q.to_string());
        let nss = t::RagPort::namespaces(self);
        std::thread::spawn(move || {
            let k = svc.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().rag.top_k;
            let refs: Vec<&str> = nss.iter().map(String::as_str).collect();
            let (hits, warnings) = svc.search(&refs, &q, k);
            if hits.is_empty() {
                let _ = done.send(Err(if warnings.is_empty() { "No matching passages.".into() } else { format!("No results. {}", warnings.join("; ")) }));
                return;
            }
            let answer = if ask {
                let ps: Vec<Passage> = hits.iter().take(k.min(6)).map(Passage::from).collect();
                match backend.complete(&build_prompt(&q, &ps)) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        let _ = done.send(Err(crate::agent().helpers_b4().provider_plain_error(&backend.provider_name(), &e)));
                        return;
                    }
                }
            } else {
                None
            };
            let _ = done.send(Ok((answer, hits.into_iter().map(hit_row).collect(), warnings)));
        });
    }
    fn research(&self, q: &str, done: Sender<t::RunView>) {
        self.0.audit.record(&self.0.session.user, "rag.research", &format!("{} chars", q.len()));
        let (svc, backend, user, q) = (self.0.services.clone(), self.0.chat.clone(), self.0.session.user.clone(), q.to_string());
        std::thread::spawn(move || {
            let k = svc.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().rag.top_k;
            let l = svc.rag_kit(&user).research(&q, k, Some(backend.as_ref()));
            let _ = done.send(run_view(&l));
        });
    }
    fn runs(&self) -> Vec<t::RunView> {
        self.0.services.rag_kit(&self.0.session.user).runs().iter().map(run_view).collect()
    }
    fn copy(&self, text: &str) {
        crate::ui::clipboard::copy(text);
    }
}
