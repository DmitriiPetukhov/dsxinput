use std::{collections::HashSet, ffi::CString};

use hidapi::{HidApi, HidError};

const SONY_VENDOR_ID: u16 = 0x054c;
const DUALSENSE_PRODUCT_ID: u16 = 0x0ce6;
const DUALSENSE_EDGE_PRODUCT_ID: u16 = 0x0df2;

pub struct ControllerTracker {
    api: HidApi,
    connected: HashSet<CString>,
}

impl ControllerTracker {
    pub fn new() -> Result<Self, HidError> {
        Ok(Self {
            api: HidApi::new()?,
            connected: HashSet::new(),
        })
    }

    pub fn update(&mut self) -> Result<(), HidError> {
        self.api.refresh_devices()?;

        let current: HashSet<CString> = self
            .api
            .device_list()
            .filter(|device| is_dualsense(device))
            .map(|device| device.path().to_owned())
            .collect();

        for path in current.difference(&self.connected) {
            println!("Controller connected: {path:?}");
        }

        for path in self.connected.difference(&current) {
            println!("Controller disconnected: {path:?}");
        }

        self.connected = current;
        Ok(())
    }
}

fn is_dualsense(device: &hidapi::DeviceInfo) -> bool {
    device.vendor_id() == SONY_VENDOR_ID
        && matches!(
            device.product_id(),
            DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID
        )
}
