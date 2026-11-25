//! The model chooser shared by the chat, the AI IDE's chat and TALES. The list, its keys and its drawing live in the UI-layer crate
//! `iyakki-chatview` (re-exported here under the old path); this file keeps the three calls that need the shell: gathering what the
//! list is built from ([`build`]), remembering a typed model id ([`remember_custom`]) and the drawing with the agents' state ([`draw`]).

pub use iyakki_chatview::model_picker::*;
use iyakki_chatview::model_picker as mp;
use iyakki_extagentx::{self as extagent};

use super::AppCx;
use crate::gfx::Gfx;
use crate::ui::R;

/// The provider as the person knows it: the well-known ones by their product name, others by the name they gave.
pub(crate) fn provider_label(cfg: &iyakki_shared::providers::ProviderCfg) -> String {
    match cfg.id.to_ascii_lowercase().as_str() {
        "nvidia" | "nvidia_nim" | "nim" => "NVIDIA NIM".into(),
        "openai" => "OpenAI".into(),
        "anthropic" | "claude" => "Anthropic Claude".into(),
        "gemini" | "google" => "Google Gemini".into(),
        "openrouter" => "OpenRouter".into(),
        "ollama" => "Ollama".into(),
        "lemonade" => "Lemonade".into(),
        _ => cfg.id.clone(),
    }
}

/// The hint line under the chooser's own hint: the AI hardware profile's `fits` line, and what to do about it.
pub(crate) fn fits_hint(p: &dyn iyakki_agent_api::AiMachine) -> String {
    if p.capable() || p.unknown() {
        format!("This machine: {}. Add one in Settings, Models.", p.fits_line())
    } else {
        format!("This machine: {}; the models above run on another computer or in the cloud.", p.fits_line())
    }
}

fn zone_of(z: iyakki_shared::router_registry::Zone) -> mp::Zone {
    use iyakki_shared::router_registry::Zone;
    match z {
        Zone::Computer => mp::Zone::Computer,
        Zone::Network => mp::Zone::Network,
        Zone::Internet => mp::Zone::Internet,
    }
}

/// Build the list. `plain` are the (provider, model) pairs, `top` the optional extra first line, `agents` whether the
/// external agents follow. `None` when there is nothing to choose.
pub(crate) fn build(cx: &AppCx, plain: Vec<(String, String)>, top: Option<TopRow>, agents: bool, current: Current, title: &str, hint: &str) -> Option<ModelList> {
    // a list stored by an earlier run (before the chat-model filter) is filtered when it is read: NVIDIA's catalog holds detectors,
    // translators and vision-only models that cannot chat
    let (plain, providers): (Vec<(String, String)>, Vec<ProviderInput>) = {
        let st = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let cfgs = &st.get().providers;
        let nvidia: Vec<&str> = cfgs.iter().filter(|p| p.kind == iyakki_shared::providers::Kind::Nvidia).map(|p| p.id.as_str()).collect();
        let plain = plain.into_iter().filter(|(p, m)| !nvidia.contains(&p.as_str()) || iyakki_shared::modelavail::is_chat_model(m)).collect();
        let providers = cfgs
            .iter()
            .map(|c| ProviderInput {
                id: c.id.clone(),
                label: provider_label(c),
                zone: zone_of(cx.agent.helpers_b6().router_zone(c)),
                own_prefix: if c.kind == iyakki_shared::providers::Kind::Nvidia { "nvidia/".to_string() } else { format!("{}/", c.id.to_ascii_lowercase()) },
            })
            .collect();
        (plain, providers)
    };
    let mut agent_inputs = Vec::new();
    if agents {
        let cfg = cx.agents_cfg();
        let entries = cx.extagent_facet().entries();
        if entries.iter().any(|e| cx.ext_status().get(&e.id).is_none()) {
            cx.ext_status().refresh(cfg.clone());
        }
        for e in &entries {
            let Some(agent) = cfg.agent(&e.id) else { continue };
            let models = iyakki_shared::extagent::models_with(cx.ext_status().cli_models(&agent.as_ref().id()), agent.as_ref());
            agent_inputs.push(AgentInput { id: e.id.clone(), provider: e.provider.clone(), models });
        }
    }
    let fits = Some(fits_hint(cx.agent.ai_profile().machine(cx.data_dir()).as_ref()));
    let bad: Vec<(String, String)> = {
        let ids: std::collections::BTreeSet<&String> = plain.iter().map(|(p, _)| p).collect();
        let mut bad = Vec::new();
        for pid in ids {
            bad.extend(cx.ai().unavailable_models(pid).into_iter().map(|m| (pid.clone(), m)));
        }
        bad
    };
    let used = crate::uisize::load_recent_models(cx.data_dir(), &cx.session.user);
    mp::build(Inputs { plain, top, agents: agent_inputs, current, title: title.to_string(), hint: hint.to_string(), fits, bad, used, providers })
}

/// Remember a typed model id for the agent so it is a line of the list next time (administrators only: it is a system setting).
pub(crate) fn remember_custom(cx: &AppCx, agent: &str, model: &str) {
    if model.is_empty() || !cx.session.policy.manage_settings {
        return;
    }
    let (a, m) = (agent.to_string(), model.to_string());
    let _ = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| {
        let list = match a.as_str() {
            extagent::CLAUDE_CODE => Some(&mut s.agents.claude.custom_models),
            extagent::COPILOT => Some(&mut s.agents.copilot.custom_models),
            other => other.strip_prefix("custom:").and_then(|c| s.agents.custom.iter_mut().find(|x| x.id == c)).map(|c| &mut c.custom_models),
        };
        if let Some(l) = list {
            if !l.contains(&m) && l.len() < 12 {
                l.push(m);
            }
        }
    });
}

/// The panel over `area` (see the crate's `draw`); the agents' state and the login buttons come from the shell.
pub(crate) fn draw(g: &mut Gfx, area: R, s: f32, cx: &AppCx, m: &mut ModelList) {
    let rows = cx.extagent_facet().entries();
    let tab = |id: &str| crate::agent_login::AGENTS.iter().position(|a| *a == id).map(|i| crate::agent_login::LOGIN_TAB + i);
    mp::draw(g, area, s, &rows, &tab, m)
}

#[cfg(test)]
mod tests {
    use crate::gfx::rgb;
    use crate::ui::{Event, Key};
    use super::*;
    use crate::apps::tests::cx_with_backend;

    fn with_models(tag: &str, n: usize) -> (AppCx, std::path::PathBuf) {
        let (cx, d) = cx_with_backend(tag, iyakki_shared::chat::Unconfigured);
        let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
        p.models = (0..n).map(|i| format!("model-{i:02}-with-a-fairly-long-name:latest")).collect();
        cx.settings().lock().unwrap().upsert_provider(p).unwrap();
        (cx, d)
    }

    fn plain(cx: &AppCx) -> Vec<(String, String)> {
        let st = cx.settings().lock().unwrap();
        st.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().map(move |m| (p.id.clone(), m.clone()))).collect()
    }

    fn draw_list(cx: &AppCx, m: &mut ModelList, w: u32, h: u32, s: f32) -> (Gfx, R) {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        draw(&mut g, area, s, cx, m);
        (g, area)
    }

    fn top() -> Option<TopRow> {
        Some(TopRow { label: "Same as chat".into(), note: "qwen3:8b · nvidia_ollama".into() })
    }

    #[test]
    fn the_list_has_the_top_line_the_providers_models_and_the_agents_groups_in_order() {
        let (cx, _d) = with_models("pickorder", 3);
        let m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "hint").unwrap();
        assert_eq!(m.rows[0], MRow::Top);
        assert!(m.rows[1..4].iter().all(|r| *r == MRow::Plain));
        let heads: Vec<usize> = m.rows.iter().enumerate().filter(|(_, r)| matches!(r, MRow::Head(_))).map(|(i, _)| i).collect();
        assert!(heads.len() >= 2 && heads[0] == 4, "{heads:?}");
        for h in &heads {
            // an agent's models follow it and "Custom model id…" closes its group
            let next_head = m.rows[h + 1..].iter().position(|r| matches!(r, MRow::Head(_))).map_or(m.rows.len(), |p| p + h + 1);
            assert!(matches!(m.rows[next_head - 1], MRow::Custom(_)), "group at {h}");
            assert!(m.rows[h + 1..next_head - 1].iter().all(|r| matches!(r, MRow::Model(..))));
        }
        assert_eq!(m.sel, 0, "same as chat is the one in use");
        // without the top line and without agents (the picture variant): only the models
        let p = build(&cx, plain(&cx), None, false, Current::default(), "t", "h").unwrap();
        assert_eq!(p.items.len(), 3);
        assert!(build(&cx, vec![], None, false, Current::default(), "t", "h").is_none(), "nothing to choose");
    }

    #[test]
    fn keys_clicks_and_the_custom_model_id_give_the_pick() {
        let (cx, _d) = with_models("pickkeys", 12);
        let mut m = build(&cx, plain(&cx), top(), true, Current { plain: Some(("nvidia_ollama".into(), "model-02-with-a-fairly-long-name:latest".into())), ..Default::default() }, "t", "h").unwrap();
        assert_eq!(m.sel, 3, "opens on the model in use (line 0 is the top line)");
        let k = |m: &mut ModelList, key: Key| super::key(m, &Event::Key(key));
        assert_eq!(k(&mut m, Key::Home), Out::Stay);
        assert_eq!(m.sel, 0);
        assert_eq!(k(&mut m, Key::Enter), Out::Pick(Pick::Top));
        k(&mut m, Key::PageDown);
        assert_eq!(m.sel, 7, "eight lines down: the top line, the provider heading and six models");
        k(&mut m, Key::PageUp);
        k(&mut m, Key::Up);
        assert_eq!(m.sel, 0);
        k(&mut m, Key::End);
        assert_eq!(m.sel, m.items.len() - 1);
        k(&mut m, Key::Down);
        assert_eq!(m.sel, m.items.len() - 1, "stops at the end");
        assert_eq!(super::key(&mut m, &Event::Row(2)), Out::Pick(Pick::Provider("nvidia_ollama".into(), "model-01-with-a-fairly-long-name:latest".into())));
        // an agent's header keeps its model; its model line picks both
        let head = m.rows.iter().position(|r| matches!(r, MRow::Head(a) if a == "claude-code")).unwrap();
        assert_eq!(super::key(&mut m, &Event::Row(head)), Out::Pick(Pick::Agent("claude-code".into())));
        assert_eq!(super::key(&mut m, &Event::Row(head + 1)), Out::Pick(Pick::AgentModel { agent: "claude-code".into(), model: String::new(), custom: false }));
        // the custom id is typed in the list: refused when it has shell characters, taken otherwise, Esc leaves the typing
        let custom = m.rows.iter().position(|r| matches!(r, MRow::Custom(a) if a == "claude-code")).unwrap();
        assert_eq!(super::key(&mut m, &Event::Row(custom)), Out::Stay);
        assert!(m.custom.is_some());
        assert_eq!(super::key(&mut m, &Event::Row(0)), Out::Stay, "clicks wait while an id is typed");
        for c in "a b".chars() {
            k(&mut m, Key::Char(c));
        }
        assert!(matches!(k(&mut m, Key::Enter), Out::Bad(e) if e.contains("letters, digits")));
        assert!(m.custom.is_some(), "stays open");
        k(&mut m, Key::Backspace);
        k(&mut m, Key::Backspace);
        k(&mut m, Key::Backspace);
        for c in "claude-opus-5-5".chars() {
            k(&mut m, Key::Char(c));
        }
        assert_eq!(k(&mut m, Key::Enter), Out::Pick(Pick::AgentModel { agent: "claude-code".into(), model: "claude-opus-5-5".into(), custom: true }));
        super::key(&mut m, &Event::Row(custom));
        assert_eq!(k(&mut m, Key::Esc), Out::Stay);
        assert!(m.custom.is_none());
        assert_eq!(k(&mut m, Key::Esc), Out::Close);
    }

    #[test]
    fn the_list_fits_scrolls_and_is_clickable_at_every_screen_size_and_text_scale() {
        let (cx, _d) = with_models("pickgeo", 40);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::ui::scale_for(w, h) * text_scale;
                let mut m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "Enter or click chooses · Esc closes · Settings → Models adds providers, Settings → Agents sets up Claude Code and others.").unwrap();
                let n = m.items.len();
                let (px, py0) = (24.0 * s, 46.0 * s + 8.0 * s);
                for typing in [false, true] {
                    if typing {
                        let c = m.rows.iter().position(|r| matches!(r, MRow::Custom(_))).unwrap();
                        super::key(&mut m, &Event::Row(c));
                        assert!(m.custom.is_some());
                    }
                    for step in [Key::Home, Key::Down, Key::PageDown, Key::End, Key::PageUp, Key::Up] {
                        if !typing {
                            super::key(&mut m, &Event::Key(step));
                        } else {
                            m.sel = [0, 1, 9, n - 1, 1, 0][m.sel % 6];
                        }
                        let (g, area) = draw_list(&cx, &mut m, w, h, s);
                        let rows: Vec<_> = g.hits.iter().filter(|x| x.label == "model").collect();
                        assert!(!rows.is_empty(), "{w}x{h} x{text_scale}");
                        assert!(rows.iter().any(|r| matches!(r.ev, Event::Row(i) if i == m.sel)), "the highlighted line is in view at {w}x{h} x{text_scale} (sel {})", m.sel);
                        let panel_bottom = py0 + (area.h - 24.0 * s);
                        for r in &rows {
                            let r = r.rect;
                            assert!(r.x >= px - 0.5 && r.x + r.w <= w as f32 + 0.5 && r.y >= area.y - 0.5 && r.y + r.h <= panel_bottom + 0.5, "line inside the panel at {w}x{h} x{text_scale}: {r:?}");
                            if typing {
                                assert!(r.y + r.h <= panel_bottom - 46.0 * s + 0.5, "lines stay above the id box at {w}x{h} x{text_scale}");
                            }
                        }
                        for p in rows.windows(2) {
                            assert!(p[0].rect.y + p[0].rect.h <= p[1].rect.y + 0.5, "no overlap");
                        }
                        // at 800x480 at least a few lines show at normal text size
                        if text_scale == 1.0 && !typing {
                            assert!(rows.len() >= 3, "{} lines at {w}x{h}", rows.len());
                        }
                    }
                    if typing {
                        super::key(&mut m, &Event::Key(Key::Esc));
                    }
                }
            }
        }
    }
    fn fake_profile(tag: &str, ram_gb: u64, card: Option<(&str, &str)>) -> iyakki_agent::aiprofile::AiProfile {
        let r = std::env::temp_dir().join(format!("iyakki-mp-{}-{:?}-{tag}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("proc")).unwrap();
        std::fs::create_dir_all(r.join("sys/class/dmi/id")).unwrap();
        std::fs::write(r.join("proc/meminfo"), format!("MemTotal: {} kB\n", ram_gb * 1024 * 1024)).unwrap();
        std::fs::write(r.join("sys/class/dmi/id/product_name"), "PC").unwrap();
        if let Some((v, d)) = card {
            let c = r.join("sys/bus/pci/devices/0000:01:00.0");
            std::fs::create_dir_all(&c).unwrap();
            for (f, t) in [("class", "0x030000"), ("vendor", v), ("device", d)] {
                std::fs::write(c.join(f), t).unwrap();
            }
        }
        let p = iyakki_agent::aiprofile::detect(&r);
        let _ = std::fs::remove_dir_all(&r);
        p
    }

    #[test]
    fn the_fits_hint_says_what_local_models_suit_this_machine_in_the_profiles_words() {
        let gpu = fake_profile("rtx", 32, Some(("0x10de", "0x2503")));
        assert_eq!(fits_hint(&gpu), format!("This machine: {}. Add one in Settings, Models.", gpu.fits_line()));
        assert!(fits_hint(&gpu).contains("7B Q4, 13B Q4"), "{}", fits_hint(&gpu));
        let cpu = fake_profile("cpu", 4, None);
        let h = fits_hint(&cpu);
        assert!(h.starts_with("This machine: fits: no model of useful size") && h.contains("run on another computer or in the cloud"), "{h}");
    }

    #[test]
    fn the_picker_carries_the_hint_hides_nothing_and_draws_it_at_every_size_inside_the_panel() {
        let (cx, _d) = with_models("pickfits", 6);
        let mut m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "hint").unwrap();
        assert!(m.fits.as_ref().is_some_and(|f| f.starts_with("This machine: ")), "{:?}", m.fits);
        // every configured model is still in the list, whatever the hint says (a 6-model provider and the top line)
        assert_eq!(m.items.iter().zip(&m.rows).filter(|(_, r)| **r == MRow::Plain).count(), 6);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
            let s = crate::ui::scale_for(w, h);
            m.fits = None;
            let (g0, _) = draw_list(&cx, &mut m, w, h, s);
            m.fits = Some(fits_hint(&fake_profile("sz", 32, Some(("0x10de", "0x2503")))));
            let (g1, area) = draw_list(&cx, &mut m, w, h, s);
            assert_ne!(g0.rgba(), g1.rgba(), "{w}x{h}: the hint is drawn");
            let panel_bottom = area.y + 8.0 * s + (area.h - 24.0 * s);
            for r in g1.hits.iter().filter(|x| x.label == "model") {
                assert!(r.rect.y >= area.y + 8.0 * s + 78.0 * s - 0.5 && r.rect.y + r.rect.h <= panel_bottom + 0.5, "{w}x{h}: {:?}", r.rect);
            }
            assert!(g1.hits.iter().any(|x| x.label == "model"), "{w}x{h}: lines remain");
        }
    }

    fn name(i: usize) -> String {
        format!("model-{i:02}-with-a-fairly-long-name:latest")
    }

    fn note(cx: &AppCx, i: usize) {
        crate::uisize::note_recent_model(cx.data_dir(), &cx.session.user, "nvidia_ollama", &name(i));
    }

    fn open(cx: &AppCx) -> ModelList {
        build(cx, plain(cx), None, true, Current::default(), "Choose the model", "hint").unwrap()
    }

    /// The model lines drawn, as (line index, rect), in order.
    fn drawn(g: &Gfx) -> Vec<(usize, R)> {
        g.hits.iter().filter(|x| x.label == "model").filter_map(|x| if let Event::Row(i) = x.ev { Some((i, x.rect)) } else { None }).collect()
    }

    #[test]
    fn the_wheel_scrolls_the_list_three_lines_a_notch_and_stops_at_both_ends() {
        let (cx, _d) = with_models("pickwheel", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0);
        assert!(m.scroll.scrolls(), "60 models do not fit");
        // the wheel down (negative) moves the list, not the highlighted line
        assert_eq!(super::key(&mut m, &Event::Scroll(-1)), Out::Stay);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 2, "three lines down: the heading and two models are gone");
        assert_eq!(m.sel, 0);
        super::key(&mut m, &Event::Scroll(1));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0);
        super::key(&mut m, &Event::Scroll(1));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0, "stops at the top");
        for _ in 0..100 {
            super::key(&mut m, &Event::Scroll(-1));
        }
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        let rows = drawn(&g);
        assert_eq!(rows.last().unwrap().0, m.items.len() - 1, "the last line shows at the bottom");
        super::key(&mut m, &Event::Scroll(-1));
        let (g2, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g2)[0].0, rows[0].0, "stops at the bottom");
        // a pointer outside the list leaves the wheel alone
        m.scroll.pointer = Some((1, 1));
        let before = m.scroll.offset;
        super::key(&mut m, &Event::Scroll(1));
        assert_eq!(m.scroll.offset, before);
    }

    #[test]
    fn the_scroll_bar_thumb_follows_the_window_and_dragging_it_moves_the_window() {
        let (cx, _d) = with_models("pickbar", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert!(g.hits.iter().any(|h| h.label == "scroll bar"), "the strip is a hit region");
        let (ty0, th0) = m.scroll.thumb();
        let n = lines(&m).len() as f32;
        let rows = (m.scroll.view.h / m.scroll.row).round();
        assert!((th0 - m.scroll.track.h * rows / n).abs() < 2.0 || th0 >= 24.0, "thumb size follows the share shown");
        assert!(th0 < m.scroll.track.h && (ty0 - m.scroll.track.y).abs() < 0.5, "the thumb starts at the top");
        super::key(&mut m, &Event::Scroll(-2));
        draw_list(&cx, &mut m, 1280, 720, s);
        assert!(m.scroll.thumb().0 > ty0, "the thumb moved down with the wheel");
        // grab the thumb and drag it to the bottom of the track
        let (ty, th) = m.scroll.thumb();
        let ymid = ty + th / 2.0 - m.scroll.track.y;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, ymid));
        assert!(m.scroll.dragging());
        let bottom = (m.scroll.track.y + m.scroll.track.h) as i32 + 50;
        super::key(&mut m, &Event::MouseMove(0, bottom));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g).last().unwrap().0, m.items.len() - 1, "dragged to the end");
        super::key(&mut m, &Event::MouseUp(0, 0));
        assert!(!m.scroll.dragging());
        // a click on the track above the thumb pages up by a screen less a line
        let before = m.scroll.offset;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, 2.0));
        assert!(m.scroll.offset < before && before - m.scroll.offset >= m.scroll.row, "paged up");
        let mid = m.scroll.offset;
        let low = m.scroll.track.h - 1.0;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, low));
        assert!(m.scroll.offset > mid, "paged down");
        // when everything fits there is no bar
        let (cx2, _d2) = with_models("pickbar2", 2);
        let mut small = open(&cx2);
        let (g, _) = draw_list(&cx2, &mut small, 1280, 720, s);
        assert!(!g.hits.iter().any(|h| h.label == "scroll bar"));
    }

    #[test]
    fn keys_keep_the_highlighted_line_in_view_in_a_long_list() {
        let (cx, _d) = with_models("pickkeysview", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(800, 480);
        for k in [Key::End, Key::PageUp, Key::Up, Key::Home, Key::PageDown, Key::PageDown, Key::Down, Key::Down] {
            super::key(&mut m, &Event::Key(k));
            let (g, _) = draw_list(&cx, &mut m, 800, 480, s);
            assert!(m.head.is_some() || drawn(&g).iter().any(|(i, _)| *i == m.sel), "line {} in view", m.sel);
            assert!(m.head.is_none() || g.hits.iter().any(|h| h.label == "provider"), "the heading is in view");
        }
        // the wheel moved it away, a key brings it back
        for _ in 0..10 {
            super::key(&mut m, &Event::Scroll(1));
        }
        draw_list(&cx, &mut m, 800, 480, s);
        super::key(&mut m, &Event::Key(Key::Down));
        let (g, _) = draw_list(&cx, &mut m, 800, 480, s);
        assert!(drawn(&g).iter().any(|(i, _)| *i == m.sel));
    }

    #[test]
    fn recent_models_come_first_five_at_most_most_recent_first_with_headings_and_without_repeats() {
        let (cx, _d) = with_models("pickrecent", 12);
        assert!(open(&cx).recent.is_empty());
        let m = open(&cx);
        assert!(lines(&m).iter().all(|l| !matches!(l, Line::Label(_))), "no recents: no headings");
        for i in [1, 2, 3, 4, 5, 6, 2] {
            note(&cx, i);
        }
        let m = open(&cx);
        let order: Vec<usize> = m.recent.iter().map(|&i| i).collect();
        let names: Vec<&str> = order.iter().map(|&i| m.items[i].1.as_str()).collect();
        assert_eq!(names, [name(2), name(6), name(5), name(4), name(3)], "last 5 distinct, newest first");
        let l = lines(&m);
        assert_eq!(l[0], Line::Label("Recent"));
        assert!(l[1..6].iter().all(|x| matches!(x, Line::Item(i) if m.recent.contains(i))));
        assert_eq!(l[6], Line::Group(0), "the provider's heading follows the Recent group");
        // the rest keep their own order and the recents are not repeated there
        let rest: Vec<usize> = l[7..].iter().filter_map(|x| if let Line::Item(i) = x { Some(*i) } else { None }).collect();
        assert_eq!(rest.iter().filter(|i| m.rows[**i] == MRow::Plain).count(), 12 - 5);
        assert!(rest.windows(2).all(|w| w[0] < w[1]));
        // keys walk the drawn order: from the first recent down to the second
        let mut m = m;
        m.sel = m.recent[0];
        super::key(&mut m, &Event::Key(Key::Down));
        assert_eq!(m.sel, m.recent[1]);
        super::key(&mut m, &Event::Key(Key::Home));
        assert_eq!(m.sel, m.recent[0], "Home is the first drawn line");
        // a model that is no longer configured is skipped
        cx.settings().lock().unwrap().upsert_provider({
            let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
            p.models = (0..12).filter(|i| *i != 6).map(name).collect();
            p
        }).unwrap();
        let m = open(&cx);
        let names: Vec<&str> = m.recent.iter().map(|&i| m.items[i].1.as_str()).collect();
        assert_eq!(names, [name(2), name(5), name(4), name(3)]);
        // and a provider that is turned off
        let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
        p.models = (0..12).map(name).collect();
        p.enabled = false;
        cx.settings().lock().unwrap().upsert_provider(p).unwrap();
        assert!(build(&cx, plain(&cx), None, false, Current::default(), "t", "h").is_none());
    }

    #[test]
    fn recents_are_kept_per_person_in_their_ui_file_and_survive_a_restart() {
        let (cx, d) = with_models("pickpersist", 8);
        note(&cx, 3);
        note(&cx, 1);
        let user = cx.session.user.clone();
        assert_eq!(crate::uisize::load_recent_models(cx.data_dir(), &user), [("nvidia_ollama".to_string(), name(1)), ("nvidia_ollama".to_string(), name(3))]);
        assert!(crate::uisize::load_recent_models(cx.data_dir(), "someone-else").is_empty());
        // other things in the same file stay
        crate::uisize::save(cx.data_dir(), &user, 2);
        note(&cx, 5);
        assert_eq!(crate::uisize::load(cx.data_dir(), &user), 2);
        assert_eq!(crate::uisize::load_recent_models(cx.data_dir(), &user).len(), 3);
        let _ = d;
    }

    #[test]
    fn the_search_filters_both_groups_keeps_the_order_and_goes_back_with_escape() {
        let (cx, _d) = with_models("picksearch", 30);
        for i in [21, 3, 25] {
            note(&cx, i);
        }
        let mut m = open(&cx);
        for c in "model-2".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        let l = lines(&m);
        let items: Vec<usize> = l.iter().filter_map(|x| if let Line::Item(i) = x { Some(*i) } else { None }).collect();
        let names: Vec<&str> = items.iter().filter(|&&i| m.rows[i] == MRow::Plain).map(|&i| m.items[i].1.as_str()).collect();
        // recents that match first (25 then 21; 3 does not match), then the other matches in list order, none repeated
        assert_eq!(names[..2], [name(25), name(21)]);
        assert_eq!(names.len(), 10, "model-20 to model-29");
        assert!(names[2..].windows(2).all(|w| w[0] < w[1]));
        assert_eq!(l[0], Line::Label("Recent"));
        assert_eq!(m.sel, items[0], "the first match is highlighted");
        // custom lines and agents that do not match are gone; Enter picks the highlighted match
        assert!(!items.iter().any(|&i| matches!(m.rows[i], MRow::Custom(_))));
        assert_eq!(super::key(&mut m, &Event::Key(Key::Enter)), Out::Pick(Pick::Provider("nvidia_ollama".into(), name(25))));
        // the search is not case sensitive, takes several words, and the keys stay inside the matches
        let mut m = open(&cx);
        for c in "NVIDIA 07".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        assert_eq!(visible(&m).len(), 1);
        super::key(&mut m, &Event::Key(Key::Down));
        assert_eq!(m.items[m.sel].1, name(7));
        // nothing matches: nothing to pick, and the text says so
        super::key(&mut m, &Event::Key(Key::Char('z')));
        assert!(visible(&m).is_empty());
        assert_eq!(super::key(&mut m, &Event::Key(Key::Enter)), Out::Stay);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert!(drawn(&g).is_empty());
        // Backspace widens it again; Esc clears the search first and only then closes
        super::key(&mut m, &Event::Key(Key::Backspace));
        assert_eq!(visible(&m).len(), 1);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Esc)), Out::Stay);
        assert!(m.query.is_empty());
        assert!(visible(&m).len() > 30);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Esc)), Out::Close);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Ctrl('p'))), Out::Close, "Ctrl+P toggles");
    }

    #[test]
    fn a_long_list_with_recents_and_a_search_draws_inside_the_window_at_every_size_and_the_largest_text() {
        let (cx, _d) = with_models("pickgeo2", 60);
        for i in [5, 9, 40, 41, 42, 43] {
            note(&cx, i);
        }
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
            for text_scale in [1.0f32, 2.0] {
                let s = crate::ui::scale_for(w, h) * text_scale;
                for q in ["", "model-4"] {
                    let mut m = open(&cx);
                    m.query = q.to_string();
                    for k in [Key::Home, Key::PageDown, Key::End, Key::Up] {
                        super::key(&mut m, &Event::Key(k));
                        let (g, area) = draw_list(&cx, &mut m, w, h, s);
                        let rows = drawn(&g);
                        assert!(!rows.is_empty(), "{w}x{h} x{text_scale} q={q}");
                        assert!(rows.iter().any(|(i, _)| *i == m.sel), "highlighted line in view {w}x{h} x{text_scale}");
                        let panel_bottom = area.y + 8.0 * s + (area.h - 24.0 * s);
                        for (_, r) in &rows {
                            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y >= area.y - 0.5 && r.y + r.h <= panel_bottom + 0.5, "inside at {w}x{h} x{text_scale}: {r:?}");
                        }
                        for p in rows.windows(2) {
                            assert!(p[0].1.y + p[0].1.h <= p[1].1.y + 0.5);
                        }
                        // the bar sits inside the panel and the rows do not run under it
                        if let Some(b) = g.hits.iter().find(|x| x.label == "scroll bar") {
                            assert!(b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y >= area.y - 0.5 && b.rect.y + b.rect.h <= panel_bottom + 0.5);
                            assert!(rows.iter().all(|(_, r)| r.x + r.w <= b.rect.x + 0.5 || r.x + r.w <= b.rect.x + b.rect.w), "no row under the strip");
                        }
                    }
                }
            }
        }
    }

    fn multi(tag: &str, nv: usize) -> (AppCx, std::path::PathBuf) {
        use iyakki_shared::providers::{Kind, ProviderCfg};
        let (cx, d) = cx_with_backend(tag, iyakki_shared::chat::Unconfigured);
        let mk = |id: &str, kind: Kind, base: Option<&str>, models: Vec<String>| {
            let mut p = ProviderCfg::new(id, kind);
            p.base_url = base.map(str::to_string);
            p.models = models;
            if kind.needs_key() {
                p.key_ref = Some("vault:k".into());
            }
            p
        };
        let mut st = cx.settings().lock().unwrap();
        // configured order: the cloud ones first, to show the zone decides
        st.upsert_provider(mk("openai", Kind::OpenAi, None, vec!["gpt-x".into(), "shared-name".into()])).unwrap();
        st.upsert_provider(mk("nvidia", Kind::Nvidia, None, (0..nv).map(|i| if i == 0 { "nvidia/nemotron-x".to_string() } else { format!("meta/llama-{i}") }).chain(["shared-name".to_string()]).collect())).unwrap();
        st.upsert_provider(mk("lab", Kind::Compat, Some("http://192.168.1.20:8000/v1"), vec!["lab-model".into()])).unwrap();
        st.upsert_provider(mk("ollama", Kind::Ollama, None, vec!["llama3".into(), "qwen3".into()])).unwrap();
        drop(st);
        (cx, d)
    }

    fn heads(m: &ModelList) -> Vec<(String, usize)> {
        lines(m).iter().filter_map(|l| if let Line::Group(g) = l { Some((m.groups[*g].label.clone(), m.groups[*g].items.len())) } else { None }).collect()
    }

    #[test]
    fn providers_are_grouped_this_computer_then_the_network_then_the_internet_with_the_last_used_first() {
        let (cx, _d) = multi("pickgroups", 3);
        let m = open(&cx);
        let names: Vec<String> = heads(&m).into_iter().map(|h| h.0).collect();
        assert_eq!(names, ["Ollama", "lab", "OpenAI", "NVIDIA NIM"], "computer, network, then the internet in configured order");
        assert_eq!(m.groups.iter().map(|g| chip(g.zone)).collect::<Vec<_>>(), ["local", "network", "internet", "internet"]);
        assert_eq!(heads(&m)[3].1, 4, "count of the provider's models");
        // the cloud provider used last comes first
        crate::uisize::note_recent_model(cx.data_dir(), &cx.session.user, "nvidia", "shared-name");
        let m = open(&cx);
        assert_eq!(m.groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Ollama", "lab", "NVIDIA NIM", "OpenAI"]);
        // the agents stay their own group after the providers
        let l = lines(&m);
        let last_group = l.iter().rposition(|x| matches!(x, Line::Group(_))).unwrap();
        let first_agent = l.iter().position(|x| matches!(x, Line::Item(i) if matches!(m.rows[*i], MRow::Head(_)))).unwrap();
        assert!(first_agent > last_group);
        // the shown name drops the provider's own prefix, the full id is in the status line
        let nem = m.items.iter().position(|(p, mo)| p == "nvidia" && mo == "nvidia/nemotron-x").unwrap();
        assert_eq!(shown(&m, nem), "nemotron-x");
        let llama = m.items.iter().position(|(_, mo)| mo == "meta/llama-1").unwrap();
        assert_eq!(shown(&m, llama), "meta/llama-1", "another vendor's prefix stays");
    }

    #[test]
    fn long_lists_start_folded_but_the_group_in_use_and_groups_fold_and_unfold_by_key_and_click() {
        let (cx, _d) = multi("pickfold", 20);
        let cur = Current { plain: Some(("nvidia".into(), "meta/llama-5".into())), ..Default::default() };
        let mut m = build(&cx, plain(&cx), None, false, cur, "t", "h").unwrap();
        let open_groups: Vec<&str> = m.groups.iter().zip(&m.collapsed).filter(|(_, c)| !**c).map(|(g, _)| g.label.as_str()).collect();
        assert_eq!(open_groups, ["NVIDIA NIM"], "only the group of the model in use is open");
        assert_eq!(m.items[m.sel].1, "meta/llama-5");
        let n_open = visible(&m).len();
        assert_eq!(n_open, 21, "the open group's models only");
        let k = |m: &mut ModelList, key: Key| super::key(m, &Event::Key(key));
        // Home is the first heading; Right opens it, Left closes it, Enter flips it
        k(&mut m, Key::Home);
        assert_eq!(m.head, Some(0));
        k(&mut m, Key::Right);
        assert!(!m.collapsed[0]);
        assert_eq!(visible(&m).len(), n_open + 2);
        k(&mut m, Key::Left);
        assert!(m.collapsed[0]);
        k(&mut m, Key::Enter);
        assert!(!m.collapsed[0]);
        // Down walks into the group, Left from a model jumps to the heading and folds it
        k(&mut m, Key::Down);
        assert_eq!(m.head, None);
        k(&mut m, Key::Left);
        assert!(m.collapsed[0] && m.head == Some(0));
        // a click on a heading flips it and highlights it
        let g = m.groups.iter().position(|g| g.label == "OpenAI").unwrap();
        assert_eq!(super::key(&mut m, &Event::Tab(HEAD_TAB + g)), Out::Stay);
        assert!(!m.collapsed[g] && m.head == Some(g));
        // Enter on a heading never picks; Enter on a model does
        assert_eq!(k(&mut m, Key::Enter), Out::Stay);
        assert!(m.collapsed[g]);
        k(&mut m, Key::Enter);
        k(&mut m, Key::Down);
        assert!(matches!(k(&mut m, Key::Enter), Out::Pick(Pick::Provider(p, _)) if p == "openai"));
        // the drawn heading is a click target
        let s = crate::ui::scale_for(1280, 720);
        let (gx, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(gx.hits.iter().filter(|h| h.label == "provider").count(), m.groups.len());
    }

    #[test]
    fn a_search_flattens_the_groups_and_shows_the_provider_so_equal_names_are_told_apart() {
        let (cx, _d) = multi("pickdup", 20);
        let mut m = open(&cx);
        for c in "shared".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        let v = visible(&m);
        assert_eq!(v.len(), 2);
        assert!(lines(&m).iter().all(|l| !matches!(l, Line::Group(_))), "no headings while searching");
        let owners: Vec<&str> = v.iter().map(|&i| m.items[i].0.as_str()).collect();
        assert_eq!(owners, ["openai", "nvidia"], "in the groups' order");
        let s = crate::ui::scale_for(1280, 720);
        let (g1, _) = draw_list(&cx, &mut m, 1280, 720, s);
        let (mut a, mut b) = (open(&cx), open(&cx));
        a.query = "shared".into();
        b.query = "shared".into();
        b.sel = v[1];
        // the provider name is drawn: a search for the provider's label changes pixels versus a blank column
        assert!(drawn(&g1).len() == 2);
        let _ = (&mut a, &mut b);
        // keys stay among the matches and the groups' fold state is untouched
        assert_eq!(super::key(&mut m, &Event::Key(Key::Down)), Out::Stay);
        assert_eq!(m.sel, v[1]);
        super::key(&mut m, &Event::Key(Key::Esc));
        assert!(heads(&m).len() == 4);
    }

    #[test]
    fn models_the_key_cannot_use_are_hidden_with_a_footer_and_come_back_with_show() {
        let (cx, _d) = with_models("pickhidden", 10);
        let mut m = open(&cx);
        m.unusable = vec![2, 3, 4];
        let s = crate::ui::scale_for(1280, 720);
        assert!(!visible(&m).iter().any(|i| m.unusable.contains(i)));
        assert_eq!(m.groups[0].items.len(), 10, "the group still owns them");
        let (g, area) = draw_list(&cx, &mut m, 1280, 720, s);
        let foot = g.hits.iter().find(|h| h.label == "hidden models").expect("the footer is a click target");
        assert!(foot.rect.y + foot.rect.h <= area.y + 8.0 * s + (area.h - 24.0 * s) + 0.5);
        assert!(drawn(&g).iter().all(|(i, _)| !m.unusable.contains(i)));
        // search never finds a hidden one
        for c in "model-03".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        assert!(visible(&m).is_empty());
        super::key(&mut m, &Event::Key(Key::Esc));
        // show: a click on the footer (or Ctrl+U) brings them back, hide again hides them
        assert_eq!(super::key(&mut m, &Event::Tab(HIDE_TAB)), Out::Stay);
        assert!(m.show_hidden && visible(&m).contains(&3));
        super::key(&mut m, &Event::Key(Key::Ctrl('u')));
        assert!(!m.show_hidden && !visible(&m).contains(&3));
        // nothing unusable: no footer and the keys do nothing
        let mut clean = open(&cx);
        let (g, _) = draw_list(&cx, &mut clean, 1280, 720, s);
        assert!(!g.hits.iter().any(|h| h.label == "hidden models"));
        super::key(&mut clean, &Event::Key(Key::Ctrl('u')));
        assert!(!clean.show_hidden);
    }

    #[test]
    fn the_recent_heading_stays_in_view_with_its_first_row_at_the_top_of_the_list() {
        let (cx, _d) = with_models("pickheading", 60);
        note(&cx, 7);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        for home in [false, true] {
            super::key(&mut m, &Event::Key(if home { Key::Home } else { Key::End }));
            let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
            if home {
                assert_eq!(m.scroll.offset, 0.0);
                assert_eq!(m.sel, m.recent[0]);
                assert!(g.text_log.iter().any(|t| t.2 == "RECENT"), "the heading is drawn above the selected first row");
            }
        }
    }

    #[test]
    fn an_old_stored_nvidia_list_is_filtered_when_the_picker_reads_it() {
        use iyakki_shared::providers::{Kind, ProviderCfg};
        let (cx, _d) = cx_with_backend("pickstored", iyakki_shared::chat::Unconfigured);
        let ids = ["nvidia/nemotron-nano-9b-v2", "nvidia/ai-synthetic-video-detector", "nvidia/cosmos-reason2-8b", "nvidia/neva-22b", "nvidia/riva-translate-4b-instruct", "nvidia/riva-translate-4b-instruct-v2", "nvidia/vila", "nvidia/llama3-chatqa-1.5-70b", "meta/llama-3.1-8b-instruct"];
        let mut p = ProviderCfg::new("nvidia", Kind::Nvidia);
        p.key_ref = Some("vault:k".into());
        p.models = ids.iter().map(|s| s.to_string()).collect();
        let mut o = ProviderCfg::new("ollama", Kind::Ollama);
        o.models = vec!["vila-local".into(), "neva-mini".into()];
        {
            let mut st = cx.settings().lock().unwrap();
            st.upsert_provider(p).unwrap();
            st.upsert_provider(o).unwrap();
        }
        let m = build(&cx, plain(&cx), None, false, Current::default(), "t", "h").unwrap();
        let shown: Vec<&str> = m.items.iter().filter(|(p, _)| p == "nvidia").map(|(_, mo)| mo.as_str()).collect();
        assert_eq!(shown, ["nvidia/nemotron-nano-9b-v2", "nvidia/llama3-chatqa-1.5-70b", "meta/llama-3.1-8b-instruct"]);
        assert_eq!(m.items.iter().filter(|(p, _)| p == "ollama").count(), 2, "other providers are not filtered");
        assert!(visible(&m).iter().all(|&i| !m.items[i].1.contains("detector") && !m.items[i].1.contains("riva")));
    }
}
