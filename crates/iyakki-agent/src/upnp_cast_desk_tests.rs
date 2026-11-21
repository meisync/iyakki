use super::*;
use std::net::SocketAddr;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const POSITION: &str = "<RelTime>0:01:40</RelTime><TrackDuration>0:10:00</TrackDuration>";
const TRANSPORT: &str = "<CurrentTransportState>PLAYING</CurrentTransportState>";

struct FakeWire(Arc<Mutex<Vec<String>>>);

impl Wire for FakeWire {
    fn get(&self, _: SocketAddr, _: &str) -> Result<(u16, String), String> {
        Ok((200, String::new()))
    }
    fn soap(&self, _: SocketAddr, _: &str, action: &str, body: &str) -> Result<(u16, String), String> {
        let name = action.rsplit('#').next().unwrap_or("").trim_matches('"').to_string();
        self.0.lock().unwrap().push(format!("{name} {}", body.split("<Target>").nth(1).and_then(|t| t.split("</Target>").next()).unwrap_or("")));
        Ok((200, match name.as_str() {
            "GetPositionInfo" => POSITION.into(),
            "GetTransportInfo" => TRANSPORT.into(),
            _ => String::new(),
        }))
    }
}

struct FakeNet {
    log: Arc<Mutex<Vec<String>>>,
    server_on: bool,
}

fn tv() -> Renderer {
    Renderer { name: "Living Room TV".into(), udn: "uuid:1".into(), control: "192.168.1.20:49152".parse().unwrap(), control_path: "/avt".into() }
}

impl CastNet for FakeNet {
    fn find(&self) -> Result<Vec<Renderer>, String> {
        Ok(vec![tv()])
    }
    fn wire(&self) -> Box<dyn Wire> {
        Box::new(FakeWire(self.log.clone()))
    }
    fn source(&self, _: &str, id: &str, _: IpAddr) -> Option<(String, String, bool, String)> {
        (self.server_on && id == "a0").then(|| ("Song".to_string(), "audio/mpeg".to_string(), false, "http://192.168.1.5:8200/upnp/media/a0".to_string()))
    }
    fn items(&self, _: &str) -> Vec<(String, String, bool)> {
        if self.server_on { vec![("a0".into(), "Song".into(), false)] } else { Vec::new() }
    }
}

fn rig(server_on: bool) -> (std::sync::MutexGuard<'static, ()>, Arc<Mutex<Vec<String>>>) {
    let g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let log = Arc::new(Mutex::new(Vec::new()));
    use_net(Some(Arc::new(FakeNet { log: log.clone(), server_on })));
    (g, log)
}

#[test]
fn players_are_found_by_name_and_every_action_needs_a_name_from_the_last_search() {
    let (_g, log) = rig(true);
    assert!(play("u", "Living Room TV", "a0").unwrap_err().contains("look for players again"));
    assert_eq!(find().unwrap(), ["Living Room TV"]);
    assert_eq!(items("u"), [("a0".to_string(), "Song".to_string(), false)]);
    assert_eq!(play("u", "Living Room TV", "a0").unwrap(), "Living Room TV is playing Song.");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("SetAVTransportURI")) && log.lock().unwrap().iter().any(|l| l.starts_with("Play")));
    assert!(play("u", "Other", "a0").is_err());
    // an item the server does not have is refused before the player is touched
    let before = log.lock().unwrap().len();
    assert!(play("u", "Living Room TV", "v9").unwrap_err().contains("no such item"));
    assert_eq!(log.lock().unwrap().len(), before);
    assert_eq!(stop("Living Room TV").unwrap(), "Living Room TV was told to stop.");
    use_net(None);
}

#[test]
fn seek_moves_from_the_reported_position_never_before_the_start_and_progress_says_it_in_one_line() {
    let (_g, log) = rig(true);
    find().unwrap();
    let (line, pos) = progress("Living Room TV").unwrap();
    assert_eq!((line.as_str(), pos), ("Living Room TV: playing 0:01:40 of 0:10:00 (16 %)", Some(100)));
    control("Living Room TV", Move::By(30)).unwrap();
    control("Living Room TV", Move::By(-500)).unwrap();
    control("Living Room TV", Move::To(42)).unwrap();
    control("Living Room TV", Move::Pause).unwrap();
    control("Living Room TV", Move::Resume).unwrap();
    let seeks: Vec<String> = log.lock().unwrap().iter().filter(|l| l.starts_with("Seek")).cloned().collect();
    assert_eq!(seeks, ["Seek 0:02:10", "Seek 0:00:00", "Seek 0:00:42"]);
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("Pause")));
    assert!(control("Living Room TV", Move::To(86_400)).unwrap_err().contains("0 to 86399"));
    use_net(None);
}
