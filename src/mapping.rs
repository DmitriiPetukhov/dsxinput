//! Pure axis scaling and supported control mapping.
use crate::linux::{
    InputAbsInfo, InputEvent, ABS_HAT0X, ABS_HAT0Y, ABS_RX, ABS_RY, ABS_RZ, ABS_X, ABS_Y, ABS_Z,
    BTN_EAST, BTN_MODE, BTN_NORTH, BTN_SELECT, BTN_SOUTH, BTN_START, BTN_THUMBL, BTN_THUMBR,
    BTN_TL, BTN_TR, BTN_WEST, EV_ABS, EV_KEY, EV_SYN, SYN_REPORT,
};

/// Scale and clamp a physical stick range to the signed Xbox range.
fn axis_to_signed(raw: i32, info: &InputAbsInfo) -> i32 {
    let span = (i64::from(info.maximum) - i64::from(info.minimum)).max(1);
    (((i64::from(raw) - i64::from(info.minimum)) * 65535 / span) - 32768).clamp(-32768, 32767)
        as i32
}

/// Scale and clamp a physical trigger range to 0..=255.
fn axis_to_trigger(raw: i32, info: &InputAbsInfo) -> i32 {
    let span = (i64::from(info.maximum) - i64::from(info.minimum)).max(1);
    ((i64::from(raw) - i64::from(info.minimum)) * 255 / span).clamp(0, 255) as i32
}

/// Keep only axes exposed by the virtual controller.
fn map_axis(code: u16) -> Option<u16> {
    match code {
        ABS_X | ABS_Y | ABS_RX | ABS_RY | ABS_Z | ABS_RZ | ABS_HAT0X | ABS_HAT0Y => Some(code),
        _ => None,
    }
}

/// Map Sony face buttons to Xbox codes; triggers are handled as analog axes.
fn map_key(code: u16) -> Option<u16> {
    match code {
        BTN_NORTH => Some(BTN_WEST), // Triangle -> Xbox Y
        BTN_WEST => Some(BTN_NORTH), // Square -> Xbox X
        BTN_SOUTH | BTN_EAST | BTN_TL | BTN_TR | BTN_SELECT | BTN_START | BTN_MODE | BTN_THUMBL
        | BTN_THUMBR => Some(code),
        _ => None,
    }
}

/// Translate a supported physical event; ignore unrelated input.
pub(crate) fn translate_event(
    event: InputEvent,
    axes: &[InputAbsInfo; 18],
) -> Option<(u16, u16, i32)> {
    match event.kind {
        EV_KEY if (0..=2).contains(&event.value) => {
            map_key(event.code).map(|code| (EV_KEY, code, event.value))
        }
        EV_ABS => map_axis(event.code).map(|code| {
            let value = match code {
                ABS_X | ABS_Y | ABS_RX | ABS_RY => {
                    axis_to_signed(event.value, &axes[code as usize])
                }
                ABS_Z | ABS_RZ => axis_to_trigger(event.value, &axes[code as usize]),
                _ => event.value.clamp(-1, 1),
            };
            (EV_ABS, code, value)
        }),
        EV_SYN if event.code == SYN_REPORT => Some((EV_SYN, SYN_REPORT, 0)),
        _ => None,
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    fn range(minimum: i32, maximum: i32) -> InputAbsInfo {
        InputAbsInfo {
            minimum,
            maximum,
            ..Default::default()
        }
    }

    #[test]
    fn scales_stick_endpoints_midpoint_and_clamps() {
        let info = range(0, 255);
        for (raw, expected) in [
            (-1, -32768),
            (0, -32768),
            (127, -129),
            (128, 128),
            (255, 32767),
            (256, 32767),
        ] {
            assert_eq!(axis_to_signed(raw, &info), expected, "raw={raw}");
        }
    }

    #[test]
    fn scales_offset_trigger_range_and_clamps() {
        let info = range(-100, 100);
        for (raw, expected) in [(-101, 0), (-100, 0), (0, 127), (100, 255), (101, 255)] {
            assert_eq!(axis_to_trigger(raw, &info), expected, "raw={raw}");
        }
    }

    #[test]
    fn degenerate_ranges_do_not_divide_by_zero() {
        for info in [range(10, 10), range(10, 0)] {
            assert_eq!(axis_to_signed(10, &info), -32768);
            assert_eq!(axis_to_trigger(10, &info), 0);
            assert_eq!(axis_to_signed(11, &info), 32767);
            assert_eq!(axis_to_trigger(11, &info), 255);
        }
    }

    #[test]
    fn allows_only_supported_axes() {
        let supported = [
            ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y,
        ];
        for code in 0..=63 {
            assert_eq!(map_axis(code), supported.contains(&code).then_some(code));
        }
        assert_eq!(map_axis(u16::MAX), None);
    }

    #[test]
    fn allows_only_supported_buttons() {
        let supported = [
            BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
            BTN_MODE, BTN_THUMBL, BTN_THUMBR,
        ];
        for code in 0..=0x2ff {
            assert_eq!(map_key(code).is_some(), supported.contains(&code));
        }
        assert_eq!(map_key(u16::MAX), None);
    }
}

#[cfg(test)]
mod axis_overflow_tests {
    use super::*;

    #[test]
    fn scales_full_i32_range_without_overflow() {
        let info = InputAbsInfo {
            minimum: i32::MIN,
            maximum: i32::MAX,
            ..Default::default()
        };
        for (raw, signed, trigger) in [(i32::MIN, -32768, 0), (0, -1, 127), (i32::MAX, 32767, 255)]
        {
            assert_eq!(axis_to_signed(raw, &info), signed);
            assert_eq!(axis_to_trigger(raw, &info), trigger);
        }
    }

    #[test]
    fn clamps_extreme_samples_without_overflow() {
        let info = InputAbsInfo {
            minimum: 1,
            maximum: 255,
            ..Default::default()
        };
        assert_eq!(axis_to_signed(i32::MIN, &info), -32768);
        assert_eq!(axis_to_trigger(i32::MIN, &info), 0);
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use crate::linux::{BTN_TL2, BTN_TR2, EV_FF};

    fn translate(kind: u16, code: u16, value: i32) -> Option<(u16, u16, i32)> {
        let event = InputEvent {
            time: unsafe { std::mem::zeroed() },
            kind,
            code,
            value,
        };
        let axes = [InputAbsInfo {
            minimum: 0,
            maximum: 255,
            ..Default::default()
        }; 18];
        translate_event(event, &axes)
    }

    #[test]
    fn forwards_button_press_release_and_repeat() {
        for value in [0, 1, 2] {
            assert_eq!(
                translate(EV_KEY, BTN_SOUTH, value),
                Some((EV_KEY, BTN_SOUTH, value))
            );
        }
    }

    #[test]
    fn maps_face_buttons_to_xbox_positions() {
        // Xbox X uses 0x133 and Y uses 0x134, opposite the Sony north/west codes.
        for (source, target) in [
            (BTN_SOUTH, 0x130), // Cross -> A
            (BTN_EAST, 0x131),  // Circle -> B
            (BTN_WEST, 0x133),  // Square -> X
            (BTN_NORTH, 0x134), // Triangle -> Y
        ] {
            for value in [0, 1, 2] {
                assert_eq!(
                    translate(EV_KEY, source, value),
                    Some((EV_KEY, target, value))
                );
            }
        }
    }

    #[test]
    fn scales_all_sticks_and_triggers() {
        for code in [ABS_X, ABS_Y, ABS_RX, ABS_RY] {
            assert_eq!(translate(EV_ABS, code, 255), Some((EV_ABS, code, 32767)));
        }
        for code in [ABS_Z, ABS_RZ] {
            assert_eq!(translate(EV_ABS, code, 128), Some((EV_ABS, code, 128)));
        }
    }

    #[test]
    fn uses_each_axes_own_calibration() {
        let mut axes = [InputAbsInfo::default(); 18];
        axes[ABS_Y as usize] = InputAbsInfo {
            minimum: -100,
            maximum: 100,
            ..Default::default()
        };
        let event = InputEvent {
            time: unsafe { std::mem::zeroed() },
            kind: EV_ABS,
            code: ABS_Y,
            value: 100,
        };
        assert_eq!(translate_event(event, &axes), Some((EV_ABS, ABS_Y, 32767)));
    }

    #[test]
    fn forwards_hat_values_without_scaling() {
        for code in [ABS_HAT0X, ABS_HAT0Y] {
            for value in [-1, 0, 1] {
                assert_eq!(translate(EV_ABS, code, value), Some((EV_ABS, code, value)));
            }
        }
    }

    #[test]
    fn normalizes_syn_report_value() {
        assert_eq!(
            translate(EV_SYN, SYN_REPORT, 42),
            Some((EV_SYN, SYN_REPORT, 0))
        );
    }

    #[test]
    fn ignores_other_events_before_indexing_axis_info() {
        for (kind, code) in [
            (EV_KEY, BTN_TL2),
            (EV_KEY, BTN_TR2),
            (EV_ABS, u16::MAX),
            (EV_SYN, 3),
            (EV_FF, 0),
        ] {
            assert_eq!(translate(kind, code, 1), None);
        }
    }
}

#[cfg(test)]
mod invariant_tests {
    use super::*;

    #[test]
    fn scaling_is_bounded_monotone_and_reaches_endpoints() {
        for (min, max) in [
            (0, 255),
            (-32768, 32767),
            (-100, 100),
            (10, 11),
            (i32::MIN, i32::MAX),
        ] {
            let info = InputAbsInfo {
                minimum: min,
                maximum: max,
                ..Default::default()
            };
            let mut last = (-32768, 0);
            for step in 0..=4096i64 {
                let raw = (i64::from(min) + (i64::from(max) - i64::from(min)) * step / 4096) as i32;
                let current = (axis_to_signed(raw, &info), axis_to_trigger(raw, &info));
                assert!((-32768..=32767).contains(&current.0));
                assert!((0..=255).contains(&current.1));
                assert!(current.0 >= last.0 && current.1 >= last.1);
                last = current;
            }
            assert_eq!(last, (32767, 255));
        }
    }

    #[test]
    fn arbitrary_event_codes_and_values_never_panic_or_escape_virtual_ranges() {
        let axes = [InputAbsInfo {
            minimum: 0,
            maximum: 255,
            ..Default::default()
        }; 18];
        for kind in [EV_KEY, EV_ABS, EV_SYN, u16::MAX] {
            for code in 0..=u16::MAX {
                for value in [i32::MIN, -1, 0, 1, 2, 255, i32::MAX] {
                    let event = InputEvent {
                        time: unsafe { std::mem::zeroed() },
                        kind,
                        code,
                        value,
                    };
                    if let Some((kind, code, value)) = translate_event(event, &axes) {
                        match kind {
                            EV_ABS => assert!(match code {
                                ABS_Z | ABS_RZ => (0..=255).contains(&value),
                                ABS_HAT0X | ABS_HAT0Y => (-1..=1).contains(&value),
                                _ => (-32768..=32767).contains(&value),
                            }),
                            EV_KEY => assert!((0..=2).contains(&value)),
                            EV_SYN => assert_eq!((code, value), (SYN_REPORT, 0)),
                            _ => panic!("unsupported output"),
                        }
                    }
                }
            }
        }
    }
}
