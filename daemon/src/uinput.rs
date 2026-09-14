use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, SwitchCode};
use std::io;

pub struct TabletSwitch(VirtualDevice);

impl TabletSwitch {
    pub fn new() -> io::Result<Self> {
        let mut switches = AttributeSet::<SwitchCode>::new();
        switches.insert(SwitchCode::SW_TABLET_MODE);

        let device = VirtualDevice::builder()?
            .name(b"minibookd tablet-mode switch")
            .with_switches(&switches)?
            .build()?;

        Ok(TabletSwitch(device))
    }

    pub fn set(&mut self, enabled: bool) -> io::Result<()> {
        let value = if enabled { 1 } else { 0 };
        let event = InputEvent::new(EventType::SWITCH.0, SwitchCode::SW_TABLET_MODE.0, value);
        self.0.emit(&[event])
    }
}
