// Live checks of the chat's PDF tools with a real model (qwen3-8b) and real pictures (SD-Turbo) on a Lemonade server on the LAN.
// Items of the `tests` module of agent.rs (included from there; `world`, `Script`-style helpers are in scope). All ignored:
//   IYAKKI_LIVE_LEMONADE_URL=http://host:port IYAKKI_LIVE_LEMONADE_KEY_FILE=/absolute/key/file [IYAKKI_LIVE_EVIDENCE=dir]
//   cargo test -p iyakki-agent --lib agent::tests::live_ -- --ignored --test-threads=1

mod live {
    use super::*;
    use crate::providers::{Kind, ProviderCfg, ProviderChat};

    pub fn env() -> (String, String) {
        (std::env::var("IYAKKI_LIVE_LEMONADE_URL").expect("set IYAKKI_LIVE_LEMONADE_URL"), std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").expect("set IYAKKI_LIVE_LEMONADE_KEY_FILE"))
    }

    pub fn note(row: &str, name: &str, text: &str) {
        let (url, key_file) = env();
        let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
        let mut t = text.to_string();
        for h in [key, crate::lemonade::origin(&url).unwrap_or_default(), url.trim_end_matches('/').to_string()] {
            if !h.is_empty() {
                t = t.replace(h.as_str(), "<redacted>");
            }
        }
        eprintln!("[{row}] {name}: {}", t.lines().next().unwrap_or(""));
        if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
            let d = std::path::Path::new(&d).join(row);
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(format!("{name}.txt")), t);
        }
    }

    pub fn save(row: &str, name: &str, bytes: &[u8]) {
        if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
            let d = std::path::Path::new(&d).join(row);
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(name), bytes);
        }
    }

    /// Services that reach the real server (provider `lemon`), and the text model on it.
    pub fn real(tag: &str) -> (Arc<Services>, Arc<ProviderChat>) {
        let (url, key_file) = env();
        let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
        let dir = std::env::temp_dir().join(format!("iyakki-livepdf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let svc = Services::open(&dir, up.clone(), up).unwrap();
        let mut p = ProviderCfg::new("lemon", Kind::Lemonade);
        p.base_url = Some(url);
        p.key_ref = Some(format!("file:{key_file}"));
        svc.settings.lock().unwrap().upsert_provider(p).unwrap();
        svc.refresh_lemonade("lemon").expect("typed list");
        let chat = Arc::new(ProviderChat { client: svc.client_for("lemon", false).unwrap(), model: "qwen3-8b".into(), streamer: None });
        (svc, chat)
    }

    /// Run the agent with `tools` and `system`+`user`, approving every call whose tool is in `approve`; wait up to `secs`.
    pub fn run(chat: Arc<ProviderChat>, tools: Arc<dyn Tools>, system: &str, user: &str, approve: &[&str], secs: u64) -> Vec<RunEvent> {
        let h = start(chat, tools, vec![Message::system(system), Message::user(user)], RunLimits::default());
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(e) = h.events.recv_timeout(Duration::from_millis(100)) {
                if let RunEvent::Approval { id, name, .. } = &e {
                    h.decide(id, if approve.contains(&name.as_str()) { Verdict::Approve } else { Verdict::Deny });
                }
                let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
                out.push(e);
                if done {
                    return out;
                }
            }
        }
        h.cancel();
        out
    }

    pub fn results(ev: &[RunEvent], tool: &str) -> Vec<(bool, String)> {
        ev.iter().filter_map(|e| if let RunEvent::ToolResult { name, ok, text, .. } = e { (name == tool).then(|| (*ok, text.clone())) } else { None }).collect()
    }

    /// Which tools the model asked for and what became of each (for the report).
    pub fn trace(ev: &[RunEvent]) -> String {
        ev.iter()
            .filter_map(|e| match e {
                RunEvent::Model { calls, .. } if !calls.is_empty() => Some(format!("asks {}", calls.iter().map(|c| format!("{}({})", c.name, c.args.to_string().chars().take(60).collect::<String>())).collect::<Vec<_>>().join(", "))),
                RunEvent::Decision { name, approved, by, .. } => Some(format!("{name} approved={approved} by {by}")),
                RunEvent::ToolResult { name, ok, text, .. } => Some(format!("{name} ok={ok}: {}", text.chars().take(100).collect::<String>())),
                RunEvent::Failed { error } => Some(format!("FAILED {error}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn final_text(ev: &[RunEvent]) -> String {
        ev.iter().rev().find_map(|e| if let RunEvent::Final { text } = e { Some(text.clone()) } else { None }).unwrap_or_default()
    }

    pub struct Pdf {
        pub pages: usize,
        pub text: String,
        pub images: usize,
        pub embedded_fonts: usize,
        pub bytes: usize,
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    /// The PDF opened with the repo's reader and looked at structurally.
    pub fn check(bytes: &[u8]) -> Result<Pdf, String> {
        if !bytes.starts_with(b"%PDF-") {
            return Err("no %PDF header".into());
        }
        let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(64)..]).to_string();
        if !tail.trim_end().ends_with("%%EOF") {
            return Err(format!("no %%EOF at the end: {tail:?}"));
        }
        let sx = tail.rfind("startxref").ok_or("no startxref")?;
        let off: usize = tail[sx + 9..].split_whitespace().next().and_then(|n| n.parse().ok()).ok_or("startxref has no offset")?;
        if off >= bytes.len() || !(bytes[off..].starts_with(b"xref") || bytes[off].is_ascii_digit()) {
            return Err(format!("startxref {off} does not point at a table"));
        }
        let d = iyakki_pdf::Document::open(bytes.to_vec()).map_err(|e| e.to_string())?;
        let info = d.info();
        if info.repaired {
            return Err("the reader had to repair the file".into());
        }
        let mut text = String::new();
        for i in 0..d.page_count() {
            text.push_str(&d.page_text(i).map_err(|e| format!("page {}: {e}", i + 1))?);
            text.push('\n');
        }
        let images = count(bytes, b"/Subtype /Image") + count(bytes, b"/Subtype/Image");
        let embedded_fonts = count(bytes, b"/FontFile2");
        Ok(Pdf { pages: d.page_count(), text, images, embedded_fonts, bytes: bytes.len() })
    }

    pub fn words(s: &str) -> Vec<String> {
        s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 3).map(str::to_string).collect()
    }

    /// The share of the words of `said` that the PDF text has again.
    pub fn found_back(said: &str, pdf_text: &str) -> (usize, usize) {
        let have = words(pdf_text);
        let w = words(said);
        (w.iter().filter(|x| have.contains(x)).count(), w.len())
    }

    struct NoWeb;
    impl iyakki_web::Fetch for NoWeb {
        fn get(&self, _url: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
            Err("no web in this test".into())
        }
    }

    /// The person's tools over the same world, wrapped the way the chat wraps them (PdfTools around UserTools).
    pub fn tools(w: &World) -> Arc<dyn Tools> {
        let mut policy = Role::User.template();
        policy.fs_read = vec![w.home.display().to_string()];
        policy.fs_write = vec![w.home.display().to_string()];
        policy.net_hosts = vec!["example.com".into()];
        let inner: Arc<dyn Tools> = Arc::new(UserTools::new("alice", policy.clone(), w.svc.clone(), w.host.clone(), vec![]).with_cwd(&w.home));
        let backend = crate::pdftools::RealBackend { policy, user: "alice".into(), web: Arc::new(NoWeb), downloads: w.home.join("downloads"), can_open_viewer: false };
        Arc::new(crate::pdftools::PdfTools::new(inner, Arc::new(backend)))
    }

    pub const SYSTEM: &str = "You are the assistant of IYAKKI. You can use tools. When asked to make a PDF, write the document in Markdown and call the tool pdf_create once with its title and content, then say in one sentence what you saved.";
}

#[test]
#[ignore = "live service"]
fn live_chat_qwen3_makes_a_pdf_with_pdf_create_and_a_real_picture() {
    let (svc, chat) = live::real("pdfc");
    let w = world("live-pdfc", Role::User, |_, _| {});
    // a real picture in the person's folder
    let (_, _, png, _) = svc.generate_image(Some(("lemon", "SD-Turbo")), "soft watercolor illustration, a small orange fox by a pond under the moon", "512x512").expect("SD-Turbo drew it");
    std::fs::write(w.home.join("fox.png"), &png).unwrap();
    let mut report = Vec::new();
    assert!(live::tools(&w).defs().iter().any(|d| d.spec.name == "pdf_create"), "pdf_create is offered");
    for attempt in 1..=3 {
        let name = format!("fox-story-{attempt}.pdf");
        let t0 = Instant::now();
        let ev = live::run(
            chat.clone(),
            live::tools(&w),
            &format!("{}{}", live::SYSTEM, crate::pdftools::PROMPT_RULE),
            &format!("Write a short bedtime story for children (three short paragraphs) about a small fox who finds the moon in a pond. Use a '# ' title, put the picture fox.png right after the title, and save it as a PDF called {name} in my folder. My folder is {home}. Put the picture in the text with the Markdown ![a fox]({home}/fox.png) right after the title (full paths).", home = w.home.display()),
            &["pdf_create"],
            300,
        );
        let res = live::results(&ev, "pdf_create");
        let args = ev.iter().find_map(|e| if let RunEvent::Model { calls, .. } = e { calls.iter().find(|c| c.name == "pdf_create").map(|c| c.args.clone()) } else { None });
        let ms = t0.elapsed().as_millis();
        let fin = live::final_text(&ev);
        report.push(format!("attempt {attempt}: {ms} ms, pdf_create calls {}, results {:?}, final {:?}; trace: {}", res.len(), res.iter().map(|r| r.1.chars().take(120).collect::<String>()).collect::<Vec<_>>(), fin.chars().take(120).collect::<String>(), live::trace(&ev)));
        let (Some(args), Some((true, _))) = (args, res.last().cloned()) else { continue };
        let content = args["content"].as_str().unwrap_or("").to_string();
        let leaks: Vec<&str> = ["<think", "</think", "```"].into_iter().filter(|n| content.contains(n)).collect();
        let Ok(bytes) = std::fs::read(w.home.join(&name)) else {
            report.push(format!("   the result said saved but {name} is not in the folder (title/path differ?)"));
            continue;
        };
        let pdf = live::check(&bytes).unwrap_or_else(|e| panic!("structure: {e}"));
        let (hit, all) = live::found_back(&content, &pdf.text);
        report.push(format!("   PDF {} bytes, {} pages, {} picture(s), words back {hit}/{all}, leaks in the content {leaks:?}; content starts {:?}", pdf.bytes, pdf.pages, pdf.images, content.chars().take(80).collect::<String>()));
        live::save("F-PDF-13", &name, &bytes);
        live::save("F-PDF-13", &format!("fox-story-{attempt}.md"), content.as_bytes());
        assert!(leaks.is_empty(), "{leaks:?}");
        assert!(pdf.pages >= 1 && hit * 10 >= all * 8, "words back {hit}/{all}");
        if pdf.images >= 1 {
            live::note("F-PDF-13", "pdf-create-qwen3", &report.join("\n"));
            return;
        }
        report.push("   no picture in the PDF although the story asked for fox.png".into());
    }
    live::note("F-PDF-13", "pdf-create-qwen3", &report.join("\n"));
    panic!("{}", report.join("\n"));
}

#[test]
#[ignore = "live service"]
fn live_chat_qwen3_greek_and_russian_stories_use_the_embedded_font_and_chinese_is_refused() {
    let (_svc, chat) = live::real("pdfg");
    let w = world("live-pdfg", Role::User, |_, _| {});
    let mut report = Vec::new();
    for (lang, name, ask, probe) in [
        ("Greek", "greek.pdf", "Write a very short story (three sentences) in Greek about a fox and the moon", 'α'),
        ("Russian", "russian.pdf", "Write a very short story (three sentences) in Russian about a fox and the moon", 'а'),
    ] {
        let mut ok = false;
        for attempt in 1..=3 {
            let file = format!("{attempt}-{name}");
            let ev = live::run(chat.clone(), live::tools(&w), &format!("{}{}", live::SYSTEM, crate::pdftools::PROMPT_RULE), &format!("{ask}, with a '# ' title, and save it as a PDF called {file} in my folder, which is {home} (give full paths).", home = w.home.display()), &["pdf_create"], 300);
            let res = live::results(&ev, "pdf_create");
            let content = ev.iter().find_map(|e| if let RunEvent::Model { calls, .. } = e { calls.iter().find(|c| c.name == "pdf_create").and_then(|c| c.args["content"].as_str().map(str::to_string)) } else { None }).unwrap_or_default();
            report.push(format!("{lang} attempt {attempt}: results {:?}", res.iter().map(|r| r.1.chars().take(140).collect::<String>()).collect::<Vec<_>>()));
            let Ok(bytes) = std::fs::read(w.home.join(&file)) else { continue };
            let pdf = live::check(&bytes).unwrap_or_else(|e| panic!("structure: {e}"));
            let letters = pdf.text.chars().filter(|c| c.is_alphabetic() && !c.is_ascii()).count();
            let (hit, all) = live::found_back(&content, &pdf.text);
            report.push(format!("   {} bytes, {} pages, embedded font streams {}, non-Latin letters read back {letters}, words back {hit}/{all}, first chars {:?}", pdf.bytes, pdf.pages, pdf.embedded_fonts, pdf.text.chars().take(50).collect::<String>()));
            live::save("F-PDF-11", &file, &bytes);
            live::save("F-PDF-11", &format!("{file}.md"), content.as_bytes());
            assert!(pdf.embedded_fonts >= 1, "the non-Latin text is drawn with an embedded font");
            assert!(pdf.text.contains(probe) && hit * 10 >= all * 7, "{lang}: words back {hit}/{all}; {:?}", pdf.text);
            ok = true;
            break;
        }
        assert!(ok, "{lang}: no PDF in 3 attempts:\n{}", report.join("\n"));
    }
    // Chinese is outside the supported alphabets: refused in words, nothing saved
    let direct = live::tools(&w).call("pdf_create", &json!({"title": "Fox", "content": "# Fox\n\n小狐狸在池塘里找到了月亮。", "path": "zh-direct.pdf"}));
    let e = direct.expect_err("refused");
    report.push(format!("Chinese straight to the tool: {e}"));
    assert!(e.contains("cannot be written in a PDF") && !w.home.join("zh-direct.pdf").exists(), "{e}");
    let ev = live::run(chat.clone(), live::tools(&w), &format!("{}{}", live::SYSTEM, crate::pdftools::PROMPT_RULE), "Write two short sentences in Chinese characters about a fox and save them as a PDF called zh.pdf in my folder.", &["pdf_create"], 300);
    let res = live::results(&ev, "pdf_create");
    report.push(format!("Chinese through the model: tool results {:?}; the model then said {:?}", res.iter().map(|r| (r.0, r.1.chars().take(140).collect::<String>())).collect::<Vec<_>>(), live::final_text(&ev).chars().take(200).collect::<String>()));
    assert!(!w.home.join("zh.pdf").exists() || res.iter().any(|r| r.0), "no PDF unless the tool accepted it");
    live::note("F-PDF-11", "greek-russian-chinese", &report.join("\n"));
}
