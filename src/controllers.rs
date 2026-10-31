use hidapi::{HidApi, HidError};

const SONY_VENDOR_ID: u16 = 0x054c;
const DUALSENSE_PRODUCT_ID: u16 = 0x0ce6;
const DUALSENSE_EDGE_PRODUCT_ID: u16 = 0x0df2;

pub fn detect() -> Result<(), HidError> {
    let api = HidApi::new()?;

    for device in api.device_list() {
        if device.vendor_id() == SONY_VENDOR_ID
            && matches!(
                device.product_id(),
                DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID
            )
        {
            println!(
                "Found: {} ({:?})",
                device.product_string().unwrap_or("Unknown DualSense"),
                device.bus_type(),
            )
        }
    }

    Ok(())
}
