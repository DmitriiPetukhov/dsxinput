use std::io;

use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, InputId, KeyCode, UinputAbsSetup,
    uinput::VirtualDevice,
};

const MICROSOFT_VENDOR_ID: u16 = 0x045e;
const XBOX_360_PRODUCT_ID: u16 = 0x028e;
const DEVICE_VERSION: u16 = 0x0110;

struct AxisDefinition {
    code: AbsoluteAxisCode,
    initial: i32,
    min: i32,
    max: i32,
}

struct GamepadDefinition {
    name: &'static str,
    bus: BusType,
    vendor: u16,
    product: u16,
    version: u16,
    buttons: &'static [KeyCode],
    axes: &'static [AxisDefinition],
}

const GAMEPAD_DEFINITION: GamepadDefinition = GamepadDefinition {
    name: "dsxinput Virtual Gamepad",
    bus: BusType::BUS_USB,
    vendor: MICROSOFT_VENDOR_ID,
    product: XBOX_360_PRODUCT_ID,
    version: DEVICE_VERSION,
    buttons: &[
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_NORTH,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,
        KeyCode::BTN_TR,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
    ],
    axes: &[
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_X,
            initial: 0,
            min: -32768,
            max: 32767,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_Y,
            initial: 0,
            min: -32768,
            max: 32767,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_RX,
            initial: 0,
            min: -32768,
            max: 32767,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_RY,
            initial: 0,
            min: -32768,
            max: 32767,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_Z,
            initial: 0,
            min: 0,
            max: 255,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_RZ,
            initial: 0,
            min: 0,
            max: 255,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_HAT0X,
            initial: 0,
            min: -1,
            max: 1,
        },
        AxisDefinition {
            code: AbsoluteAxisCode::ABS_HAT0Y,
            initial: 0,
            min: -1,
            max: 1,
        },
    ],
};

pub struct VirtualGamepad {
    _device: VirtualDevice,
}

impl VirtualGamepad {
    pub fn new(name: &str) -> io::Result<Self> {
        let definition = &GAMEPAD_DEFINITION;
        let mut buttons = AttributeSet::<KeyCode>::new();
        for &button in definition.buttons {
            buttons.insert(button);
        }

        let device_name = format!("{} {}", definition.name, name);
        let mut builder = VirtualDevice::builder()?
            .name(&device_name)
            .input_id(InputId::new(
                definition.bus,
                definition.vendor,
                definition.product,
                definition.version,
            ))
            .with_keys(&buttons)?;

        for axis in definition.axes {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(
                axis.code,
                AbsInfo::new(axis.initial, axis.min, axis.max, 0, 0, 0),
            ))?;
        }

        Ok(Self {
            _device: builder.build()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn advertises_xbox_360_identity() {
        let definition = &GAMEPAD_DEFINITION;
        assert_eq!(definition.name, "dsxinput Virtual Gamepad");
        assert_eq!(definition.bus, BusType::BUS_USB);
        assert_eq!(definition.vendor, 0x045e);
        assert_eq!(definition.product, 0x028e);
        assert_eq!(definition.version, 0x0110);
    }

    #[test]
    fn declares_expected_buttons_without_duplicates() {
        let expected = HashSet::from([
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ]);
        let actual: HashSet<_> = GAMEPAD_DEFINITION.buttons.iter().copied().collect();
        assert_eq!(actual, expected);
        assert_eq!(GAMEPAD_DEFINITION.buttons.len(), actual.len());
    }

    #[test]
    fn declares_exactly_eight_unique_axes() {
        let axes = GAMEPAD_DEFINITION.axes;
        let codes: HashSet<_> = axes.iter().map(|axis| axis.code).collect();
        assert_eq!(axes.len(), 8);
        assert_eq!(codes.len(), axes.len());
    }

    fn assert_axes(codes: &[AbsoluteAxisCode], min: i32, max: i32) {
        for code in codes {
            let axis = GAMEPAD_DEFINITION
                .axes
                .iter()
                .find(|axis| axis.code == *code)
                .expect("expected gamepad axis");
            assert_eq!(
                (axis.initial, axis.min, axis.max),
                (0, min, max),
                "axis {code:?}"
            );
        }
    }

    #[test]
    fn sticks_are_centered_with_signed_ranges() {
        assert_axes(
            &[
                AbsoluteAxisCode::ABS_X,
                AbsoluteAxisCode::ABS_Y,
                AbsoluteAxisCode::ABS_RX,
                AbsoluteAxisCode::ABS_RY,
            ],
            -32768,
            32767,
        );
    }

    #[test]
    fn triggers_are_independent_and_initially_released() {
        assert_axes(&[AbsoluteAxisCode::ABS_Z, AbsoluteAxisCode::ABS_RZ], 0, 255);
    }

    #[test]
    fn dpad_axes_are_initially_neutral() {
        assert_axes(
            &[AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y],
            -1,
            1,
        );
    }
}
