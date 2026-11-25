//! Settings -> Documents (RAG): how documents are cut and found, the external backends, re-embedding and indexing the person's documents.
//! The page reaches the settings, the knowledge base and the secrets only through `RagPort` (docs/development/SETTINGS-EXTRACTION-PLAN.md, lane ST-B).

use super::*;
use super::apps_port::RagPort;

impl SettingsApp {
    pub(super) fn handle_rag(&mut self, k: &Key, p: &dyn RagPort) {
        let exts = p.settings_get().external_rag;
        let cur = exts.get(self.sel()).cloned();
        let tweak = |p: &dyn RagPort, f: &dyn Fn(&mut iyakki_shared::settings::RagSettings)| p.settings_update(&mut |s| f(&mut s.rag));
        let mut result: Option<Result<(), super::super::settings_port::SettingsWriteError>> = None;
        match k {
            Key::Up => self.move_sel(-1, exts.len()),
            Key::Down => self.move_sel(1, exts.len()),
            Key::Char('[') if self.admin(p) => result = Some(tweak(p, &|r| r.top_k = r.top_k.saturating_sub(1).max(1))),
            Key::Char(']') if self.admin(p) => result = Some(tweak(p, &|r| r.top_k = (r.top_k + 1).min(50))),
            Key::Char('-') if self.admin(p) => {
                result = Some(tweak(p, &|r| {
                    r.chunk_max = r.chunk_max.saturating_sub(100).max(100);
                    r.overlap = r.overlap.min((r.chunk_max / 2).saturating_sub(1)) / 20 * 20;
                }))
            }
            Key::Char('=') if self.admin(p) => result = Some(tweak(p, &|r| r.chunk_max = (r.chunk_max + 100).min(4000))),
            Key::Char(',') if self.admin(p) => result = Some(tweak(p, &|r| r.overlap = r.overlap.saturating_sub(20))),
            Key::Char('.') if self.admin(p) => result = Some(tweak(p, &|r| r.overlap += 20)),
            Key::Char('u') if self.admin(p) => result = Some(tweak(p, &|r| r.use_external = !r.use_external)),
            Key::Char('o' | 'O' | 'g') if self.admin(p) => self.handle_rag_order(k, p),
            Key::Char('a') if self.admin(p) => {
                let f = Self::external_form(None);
                self.open_form(FormKind::External(None), f);
            }
            Key::Char('r') => self.reindex(p),
            Key::Char('i') => self.ingest(p),
            Key::Char(c) if cur.is_some() && "edtx".contains(*c) => {
                let e = cur.expect("checked");
                match *c {
                    't' => {
                        let tester = p.rag_backend_tester();
                        let backend = e.backend.clone();
                        self.start("testing backend", move || JobOut::Msg(tester.test(backend.clone()).map(|m| format!("{}: {m}", backend.id())).map_err(|e| format!("{}: {e}", backend.id()))));
                    }
                    _ if !self.admin(p) => {}
                    'e' => {
                        let f = Self::external_form(Some(&e));
                        self.open_form(FormKind::External(Some(e.backend.id().into())), f);
                    }
                    'd' => self.modal = Some(Modal::Confirm(ConfirmKind::External(e.backend.id().into()), format!("Remove backend “{}”?", e.backend.id()))),
                    'x' => {
                        let mut n = e.clone();
                        n.enabled = !n.enabled;
                        result = Some(p.settings_update(&mut |s| s.put_external(n.clone())));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if let Some(r) = result {
            match r {
                Ok(()) => {
                    p.audit("settings.rag", "");
                    self.ok("Saved.");
                }
                Err(e) => self.fail(e.message()),
            }
        }
    }

    pub(super) fn own_namespaces(p: &dyn RagPort) -> Vec<String> {
        if p.is_admin() {
            let mut v = p.kb().namespaces();
            for n in ["shared"] {
                if !v.iter().any(|x| x == n) {
                    v.push(n.into());
                }
            }
            v
        } else {
            vec![format!("user/{}", p.user())]
        }
    }

    pub(super) fn reindex(&mut self, p: &dyn RagPort) {
        let kb = p.kb();
        let nss = Self::own_namespaces(p);
        self.start("re-embedding documents", move || {
            let mut done = 0;
            for ns in &nss {
                if kb.stats(ns).is_some() {
                    if let Err(e) = kb.reindex(ns) {
                        return JobOut::Msg(Err(format!("{ns}: {e:?}")));
                    }
                    done += 1;
                }
            }
            JobOut::Msg(Ok(format!("Re-embedded {done} namespace(s) with {}.", kb.embedder_id())))
        });
    }

    pub(super) fn ingest(&mut self, p: &dyn RagPort) {
        let kb = p.kb();
        let base = p.rag_docs_dir();
        let (ns, dir) = if p.is_admin() { ("shared".to_string(), base.join("shared")) } else { (format!("user/{}", p.user()), base.join(&p.user())) };
        self.start("indexing documents", move || {
            let (added, seen) = kb.ingest_dir(&ns, &dir);
            JobOut::Msg(Ok(format!("{ns}: {added} new document(s) indexed ({seen} files scanned in {}).", dir.display())))
        });
    }

    pub(super) fn render_rag(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn RagPort) {
        let admin = p.is_admin();
        let (rag, exts) = {
            let st = p.settings_get();
            (st.rag.clone(), st.external_rag.clone())
        };
        let (embed_id, ns_stats) = {
            let kb = p.kb();
            let nss = if admin { kb.namespaces() } else { vec![format!("user/{}", p.user()), "shared".to_string()] };
            (kb.embedder_id(), nss.into_iter().filter_map(|n| kb.stats(&n).map(|st| (n, st))).collect::<Vec<_>>())
        };
        // sliders in two columns when there is room, one when there is not
        let can = admin;
        let two = a.w > 820.0 * s;
        let gap = 10.0 * s;
        let (cw, rh) = if two { ((a.w - gap) / 2.0, 82.0 * s) } else { (a.w, 82.0 * s) };
        let mut y = a.y;
        y += controls::section_title(g, a.x, y, s, "Retrieval");
        let max_overlap = (rag.chunk_max / 2).saturating_sub(1) as i64;
        let specs = [
            (SLIDER_CHUNK, "Chunk size", "Characters of a document in one searchable piece. Bigger keeps more context; smaller finds more exactly.", rag.chunk_max as i64, 100, 4000, "characters", '-', '='),
            (SLIDER_OVERLAP, "Overlap", "Characters shared by neighbouring pieces, so a sentence is not cut off at an edge.", rag.overlap as i64, 0, max_overlap, "characters", ',', '.'),
            (SLIDER_TOPK, "Results", "How many pieces come back for each question.", rag.top_k as i64, 1, 50, "per question", '[', ']'),
        ];
        let mut col = 0usize;
        let mut row_y = y;
        for (id, title, help, value, min, max, unit, minus, plus) in specs {
            let x = if two { a.x + col as f32 * (cw + gap) } else { a.x };
            let tr = controls::slider_row(g, R::new(x, row_y, cw, rh), s, &Slider { id, title, help, value, min, max, unit, minus: Event::Key(Key::Char(minus)), plus: Event::Key(Key::Char(plus)), enabled: can }, ACCENT);
            self.slider_w.push((id, tr.w));
            if two && col == 0 {
                col = 1;
            } else {
                col = 0;
                row_y += rh + gap;
            }
        }
        if col == 1 {
            // the odd one out: the switch sits beside it
            controls::switch_row(g, R::new(a.x + cw + gap, row_y, cw, rh), s, "Search external backends", "Also ask the backends listed below, not only the local documents.", rag.use_external, can, Event::Key(Key::Char('u')), ACCENT);
            row_y += rh + gap;
        } else {
            controls::switch_row(g, R::new(a.x, row_y, a.w, 66.0 * s), s, "Search external backends", "Also ask the backends listed below, not only the local documents.", rag.use_external, can, Event::Key(Key::Char('u')), ACCENT);
            row_y += 66.0 * s + gap;
        }
        row_y += self.render_rag_order(g, R::new(a.x, row_y, a.w, 0.0), s, p, can, two);
        let embed_note = if p.kb_is_reindexing() { format!("{embed_id} — updating documents in the background…") } else { embed_id.clone() };
        controls::info_row(g, R::new(a.x, row_y, a.w, 44.0 * s), s, "Embedding", &embed_note);
        let mut y = row_y + 44.0 * s + 16.0 * s;
        g.text(a.x, y, 14.0 * s, MUTED, "Local knowledge base (r reindex · i ingest documents folder)", Face::Bold);
        y += 24.0 * s;
        if ns_stats.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "empty — press i to index the documents folder", Face::Sans);
            y += 24.0 * s;
        }
        for (n, st) in ns_stats.iter().take(4) {
            let un = if st.unembedded > 0 { format!("  ({} without vectors: press r)", st.unembedded) } else { String::new() };
            g.text(a.x, y, 14.0 * s, TEXT, &format!("{n:<22}{} docs · {} chunks{un}", st.docs, st.chunks), Face::Mono);
            y += 22.0 * s;
        }
        y += 14.0 * s;
        g.text(a.x, y, 14.0 * s, MUTED, "External RAG backends", Face::Bold);
        y += 26.0 * s;
        if exts.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, if admin { "none — press a to add Qdrant, Chroma or a custom HTTP search service" } else { "none configured" }, Face::Sans);
        }
        for (i, e) in exts.iter().enumerate() {
            if y > a.y + a.h - 60.0 * s {
                break;
            }
            let url = if admin { match &e.backend { ExternalRag::Qdrant { url, collection, .. } | ExternalRag::Chroma { url, collection, .. } => format!("{url}  ·  {collection}"), ExternalRag::Http { url, .. } => url.clone() } } else { String::new() };
            let chips: Vec<(&str, [u8; 4])> = vec![(e.backend.label(), ACCENT), if e.enabled { ("on", OK) } else { ("off", ERR) }];
            let on = i == self.sel();
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, on, e.backend.id(), &url, &chips);
            y += 66.0 * s;
        }
    }
}
