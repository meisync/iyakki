//! The sound output state of the fake machine: a device, a change counter and a gain a test sets; a device change bumps the counter by itself.

use std::sync::Mutex;

use iyakki_core_api::soundctl::OutputState;

pub struct FakeOutputState {
    state: Mutex<(Option<String>, u64, u32)>,
}

impl Default for FakeOutputState {
    fn default() -> FakeOutputState {
        FakeOutputState { state: Mutex::new((None, 0, 100)) }
    }
}

impl FakeOutputState {
    /// Another device plays now (the counter moves when the device really changed).
    pub fn set_device(&self, device: Option<&str>) {
        let mut s = self.state.lock().unwrap();
        if s.0.as_deref() != device {
            s.0 = device.map(str::to_string);
            s.1 += 1;
        }
    }
    pub fn set_gain(&self, gain: u32) {
        self.state.lock().unwrap().2 = gain;
    }
}

impl OutputState for FakeOutputState {
    fn device(&self) -> Option<String> {
        self.state.lock().unwrap().0.clone()
    }
    fn generation(&self) -> u64 {
        self.state.lock().unwrap().1
    }
    fn gain(&self) -> u32 {
        self.state.lock().unwrap().2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counter_moves_only_when_the_device_changes() {
        let f = FakeOutputState::default();
        assert_eq!((f.device(), f.generation(), f.gain()), (None, 0, 100));
        f.set_device(Some("plughw:0,3"));
        f.set_device(Some("plughw:0,3"));
        assert_eq!(f.generation(), 1);
        f.set_gain(40);
        assert_eq!(f.gain(), 40);
        assert_eq!(f.generation(), 1, "a volume change is not a device change");
        f.set_device(None);
        assert_eq!((f.device(), f.generation()), (None, 2));
    }
}
