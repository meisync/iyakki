use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::events::{EventActions, EventRouter, EventsPort};
use iyakki_ui_sdk::size::TypeRole;
use std::cell::RefCell;
use std::rc::Rc;

pub struct NetApp {
    port: Box<dyn EventsPort>,
    acts: Box<dyn EventActions>,
    router: EventRouter,
    network: Rc<RefCell<String>>,
    permissions: Rc<RefCell<String>>,
}

impl NetApp {
    pub fn new(mut port: Box<dyn EventsPort>, mut acts: Box<dyn EventActions>) -> NetApp {
        let network = Rc::new(RefCell::new("not heard yet".to_string()));
        let (net, missed) = (network.clone(), network.clone());
        let permissions = Rc::new(RefCell::new(String::new()));
        let perm = permissions.clone();
        let mut router = EventRouter::new()
            .on("connectivity.changed", move |e, acts| {
                *net.borrow_mut() = e.text("state").unwrap_or("?").to_string();
                // an event that arrives `late` happened while the app was away: update the screen, but do not ring
                if !e.late && e.text("state") == Some("offline") {
                    let _ = acts.notify("The network went away");
                }
            })
            // what the app may do changed (always told, no registration needed): stop using what was taken away, read the set again
            .on_permissions(move |change, _, _| *perm.borrow_mut() = format!("{} held, {} taken away by {}", change.held, change.revoked.len(), change.by))
            // events were dropped: the mirrored state may be stale, so say so until the next one
            .on_missed(move |_, _| *missed.borrow_mut() = "unknown (events were missed)".to_string());
        // the core refuses an event the app holds no permission for, with a sentence the app may show
        let _refusals = router.register_all(port.as_mut());
        // what waited while the app was away is handled here, before the first frame
        router.begin(port.as_mut(), acts.as_mut());
        NetApp { port, acts, router, network, permissions }
    }
}

impl UiApp for NetApp {
    fn id(&self) -> &'static str { "net-demo" }
    fn title(&self) -> String { "Network demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => { self.router.pump(self.port.as_mut(), self.acts.as_mut()); }
            UiEvent::Key(UiKey::Esc) => { self.port.stop(); return Flow::Back; }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let m = cx.metrics();
        let g = m.gutter();
        let size = m.size(TypeRole::Body);
        c.text(g, g, size, cx.colors().text, &format!("Network: {}", self.network.borrow()), TextStyle::Regular);
        c.text(g, g + m.leading(size), size, cx.colors().muted, &format!("Permissions: {}", self.permissions.borrow()), TextStyle::Regular);
    }
}
