use ash::vk;
use std::{env, ffi::OsStr, sync::OnceLock};

const DEFAULT_HDR_WHITE_NITS: f32 = 203.0;
const MIN_HDR_WHITE_NITS: f32 = 80.0;
const MAX_HDR_WHITE_NITS: f32 = 1000.0;
#[cfg(test)]
const PQ_M1: f32 = 2610.0 / 16384.0;
#[cfg(test)]
const PQ_M2: f32 = 2523.0 / 32.0;
#[cfg(test)]
const PQ_C1: f32 = 3424.0 / 4096.0;
#[cfg(test)]
const PQ_C2: f32 = 2413.0 / 128.0;
#[cfg(test)]
const PQ_C3: f32 = 2392.0 / 128.0;
#[cfg(test)]
const SCRGB_WHITE_NITS: f32 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputColorMode {
    Sdr,
    Hdr10Pq,
    ScRgbLinear,
}

pub(crate) fn classify_output_color_mode(
    format: vk::Format,
    color_space: vk::ColorSpaceKHR,
) -> Option<OutputColorMode> {
    if is_existing_sdr_color_space(color_space) {
        return Some(OutputColorMode::Sdr);
    }

    match (format, color_space) {
        (
            vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::A2R10G10B10_UNORM_PACK32,
            vk::ColorSpaceKHR::HDR10_ST2084_EXT,
        ) => Some(OutputColorMode::Hdr10Pq),
        (vk::Format::R16G16B16A16_SFLOAT, vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT) => {
            Some(OutputColorMode::ScRgbLinear)
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn pq_decode_to_nits(code: f32) -> f32 {
    let p = code.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    let numerator = (p - PQ_C1).max(0.0);
    let denominator = PQ_C2 - PQ_C3 * p;
    10000.0 * (numerator / denominator).powf(1.0 / PQ_M1)
}

#[cfg(test)]
pub(crate) fn pq_encode_from_nits(nits: f32) -> f32 {
    let y = (nits / 10000.0).clamp(0.0, 1.0);
    let y_m1 = y.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y_m1) / (1.0 + PQ_C3 * y_m1)).powf(PQ_M2)
}

#[cfg(test)]
pub(crate) fn bt709_to_bt2020(linear_bt709: [f32; 3]) -> [f32; 3] {
    let [red, green, blue] = linear_bt709;
    [
        0.627404 * red + 0.329283 * green + 0.043313 * blue,
        0.069097 * red + 0.919540 * green + 0.011362 * blue,
        0.016391 * red + 0.088013 * green + 0.895595 * blue,
    ]
}

#[cfg(test)]
pub(crate) fn compose_hdr10_pixel(
    scene_pq: [f32; 3],
    ui_premultiplied_bt709: [f32; 3],
    ui_alpha: f32,
    white_nits: f32,
) -> [f32; 3] {
    let alpha = ui_alpha.clamp(0.0, 1.0);
    if alpha == 0.0 {
        return scene_pq;
    }
    let ui_bt2020_nits =
        bt709_to_bt2020(ui_premultiplied_bt709).map(|channel| channel * white_nits);
    std::array::from_fn(|channel| {
        let scene_nits = pq_decode_to_nits(scene_pq[channel]);
        pq_encode_from_nits(ui_bt2020_nits[channel] + scene_nits * (1.0 - alpha))
    })
}

#[cfg(test)]
pub(crate) fn compose_scrgb_pixel(
    scene_scrgb: [f32; 3],
    ui_premultiplied_bt709: [f32; 3],
    ui_alpha: f32,
    white_nits: f32,
) -> [f32; 3] {
    let alpha = ui_alpha.clamp(0.0, 1.0);
    if alpha == 0.0 {
        return scene_scrgb;
    }
    let white_scale = white_nits / SCRGB_WHITE_NITS;
    std::array::from_fn(|channel| {
        ui_premultiplied_bt709[channel] * white_scale + scene_scrgb[channel] * (1.0 - alpha)
    })
}

fn is_existing_sdr_color_space(color_space: vk::ColorSpaceKHR) -> bool {
    matches!(
        color_space,
        vk::ColorSpaceKHR::SRGB_NONLINEAR
            | vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT
            | vk::ColorSpaceKHR::DISPLAY_P3_LINEAR_EXT
            | vk::ColorSpaceKHR::DCI_P3_NONLINEAR_EXT
            | vk::ColorSpaceKHR::BT709_LINEAR_EXT
            | vk::ColorSpaceKHR::BT709_NONLINEAR_EXT
            | vk::ColorSpaceKHR::BT2020_LINEAR_EXT
            | vk::ColorSpaceKHR::ADOBERGB_LINEAR_EXT
            | vk::ColorSpaceKHR::ADOBERGB_NONLINEAR_EXT
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvalidHdrWhiteNits;

pub(crate) fn parse_hdr_white_nits(value: Option<&str>) -> Result<f32, InvalidHdrWhiteNits> {
    let Some(value) = value else {
        return Ok(DEFAULT_HDR_WHITE_NITS);
    };
    let nits = value.parse::<f32>().map_err(|_| InvalidHdrWhiteNits)?;
    if nits.is_finite() && (MIN_HDR_WHITE_NITS..=MAX_HDR_WHITE_NITS).contains(&nits) {
        Ok(nits)
    } else {
        Err(InvalidHdrWhiteNits)
    }
}

fn parse_hdr_white_nits_os(value: Option<&OsStr>) -> Result<f32, InvalidHdrWhiteNits> {
    let Some(value) = value else {
        return Ok(DEFAULT_HDR_WHITE_NITS);
    };
    let value = value.to_str().ok_or(InvalidHdrWhiteNits)?;
    parse_hdr_white_nits(Some(value))
}

/// Read only from HDR renderer creation; the setting is process-wide and read once.
#[allow(dead_code)] // Used when the HDR renderer is connected to swapchain creation.
pub(crate) fn hdr_white_nits() -> f32 {
    static WHITE_NITS: OnceLock<f32> = OnceLock::new();
    *WHITE_NITS.get_or_init(|| {
        let value = env::var_os("DSXINPUT_HDR_WHITE_NITS");
        match parse_hdr_white_nits_os(value.as_deref()) {
            Ok(nits) => nits,
            Err(_) => {
                eprintln!(
                    "dsxinput-overlay: invalid DSXINPUT_HDR_WHITE_NITS; using {DEFAULT_HDR_WHITE_NITS} nits"
                );
                DEFAULT_HDR_WHITE_NITS
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        bt709_to_bt2020, classify_output_color_mode, compose_hdr10_pixel, compose_scrgb_pixel,
        parse_hdr_white_nits, pq_decode_to_nits, pq_encode_from_nits, OutputColorMode,
    };
    use ash::vk;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn classifies_hdr_modes_only_for_supported_format_color_space_pairs() {
        assert_eq!(
            classify_output_color_mode(
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            Some(OutputColorMode::Hdr10Pq)
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::A2R10G10B10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            Some(OutputColorMode::Hdr10Pq)
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
            ),
            Some(OutputColorMode::ScRgbLinear)
        );
    }

    #[test]
    fn preserves_existing_sdr_spaces_and_does_not_infer_hdr_from_format() {
        let sdr_spaces = [
            vk::ColorSpaceKHR::SRGB_NONLINEAR,
            vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT,
            vk::ColorSpaceKHR::DISPLAY_P3_LINEAR_EXT,
            vk::ColorSpaceKHR::DCI_P3_NONLINEAR_EXT,
            vk::ColorSpaceKHR::BT709_LINEAR_EXT,
            vk::ColorSpaceKHR::BT709_NONLINEAR_EXT,
            vk::ColorSpaceKHR::BT2020_LINEAR_EXT,
            vk::ColorSpaceKHR::ADOBERGB_LINEAR_EXT,
            vk::ColorSpaceKHR::ADOBERGB_NONLINEAR_EXT,
        ];
        for color_space in sdr_spaces {
            assert_eq!(
                classify_output_color_mode(vk::Format::A2B10G10R10_UNORM_PACK32, color_space),
                Some(OutputColorMode::Sdr),
                "expected existing SDR behavior for color space {}",
                color_space.as_raw()
            );
        }
        assert_eq!(
            classify_output_color_mode(
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            Some(OutputColorMode::Hdr10Pq)
        );
    }

    #[test]
    fn rejects_unsupported_hdr_and_unknown_color_space_pairs() {
        assert_eq!(
            classify_output_color_mode(
                vk::Format::R8G8B8A8_UNORM,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            None
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            None
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_HLG_EXT,
            ),
            None
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::SRGB_NONLINEAR,
            ),
            Some(OutputColorMode::Sdr)
        );
        assert_eq!(
            classify_output_color_mode(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::from_raw(i32::MAX),
            ),
            None
        );
    }

    #[test]
    fn hdr_white_nits_defaults_and_accepts_inclusive_boundaries_and_fractions() {
        assert_eq!(parse_hdr_white_nits(None), Ok(203.0));
        assert_eq!(parse_hdr_white_nits(Some("80")), Ok(80.0));
        assert_eq!(parse_hdr_white_nits(Some("1000")), Ok(1000.0));
        assert_eq!(parse_hdr_white_nits(Some("203.5")), Ok(203.5));
    }

    #[test]
    fn hdr_white_nits_rejects_empty_nonfinite_invalid_and_out_of_range_values() {
        for value in ["", "NaN", "inf", "-inf", "abc", "79.99", "1000.01"] {
            assert!(
                parse_hdr_white_nits(Some(value)).is_err(),
                "expected invalid luminance {value:?} to be rejected"
            );
        }
        assert!(
            super::parse_hdr_white_nits_os(Some(std::ffi::OsStr::from_bytes(b"\xff"))).is_err()
        );
    }

    #[test]
    fn pq_reference_round_trips_required_luminances_monotonically() {
        let luminances = [0.0, 80.0, 203.0, 1000.0, 10000.0];
        let mut previous_code = -1.0;
        for nits in luminances {
            let code = pq_encode_from_nits(nits);
            assert!(
                code >= previous_code,
                "PQ code must be monotonic at {nits} nits"
            );
            assert!(
                (pq_decode_to_nits(code) - nits).abs() < 0.1,
                "PQ round trip failed for {nits} nits"
            );
            previous_code = code;
        }
        let mut previous_nits = -1.0;
        for step in 0..=1000 {
            let nits = pq_decode_to_nits(step as f32 / 1000.0);
            assert!(nits >= previous_nits, "PQ decode must be monotonic");
            previous_nits = nits;
        }
        assert_eq!(pq_encode_from_nits(-1.0), pq_encode_from_nits(0.0));
        assert_eq!(pq_encode_from_nits(10001.0), 1.0);
    }

    #[test]
    fn hdr10_reference_composes_in_linear_nits_and_preserves_transparent_pixels() {
        let scene_80_nits = [pq_encode_from_nits(80.0); 3];
        assert_eq!(
            compose_hdr10_pixel(scene_80_nits, [0.0; 3], 0.0, 203.0),
            scene_80_nits,
            "transparent UI must avoid a lossy PQ round trip"
        );

        let white = compose_hdr10_pixel(scene_80_nits, [1.0; 3], 1.0, 203.0);
        for code in white {
            assert!((pq_decode_to_nits(code) - 203.0).abs() < 0.1);
        }

        let half_alpha_black = compose_hdr10_pixel(scene_80_nits, [0.0; 3], 0.5, 203.0);
        for code in half_alpha_black {
            assert!((pq_decode_to_nits(code) - 40.0).abs() < 0.1);
        }

        let half_alpha_white = compose_hdr10_pixel(scene_80_nits, [0.5; 3], 0.5, 203.0);
        for code in half_alpha_white {
            assert!((pq_decode_to_nits(code) - 141.5).abs() < 0.1);
        }
    }

    #[test]
    fn bt709_to_bt2020_reference_uses_the_design_matrix() {
        let basis_vectors = [
            ([1.0, 0.0, 0.0], [0.627404, 0.069097, 0.016391]),
            ([0.0, 1.0, 0.0], [0.329283, 0.919540, 0.088013]),
            ([0.0, 0.0, 1.0], [0.043313, 0.011362, 0.895595]),
        ];
        for (input, expected) in basis_vectors {
            let converted = bt709_to_bt2020(input);
            for (actual, expected) in converted.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.000001);
            }
        }
    }

    #[test]
    fn scrgb_reference_scales_white_and_preserves_extended_range_values() {
        let ui_white = compose_scrgb_pixel([0.0; 3], [1.0; 3], 1.0, 203.0);
        for channel in ui_white {
            assert!((channel - 2.5375).abs() < 0.000001);
        }

        let scene = [-2.0, 1.5, 4.0];
        assert_eq!(
            compose_scrgb_pixel(scene, [0.0; 3], 0.0, 203.0),
            scene,
            "transparent UI must preserve scRGB values outside 0..1"
        );
        let half_alpha_black = compose_scrgb_pixel(scene, [0.0; 3], 0.5, 203.0);
        assert_eq!(half_alpha_black, [-1.0, 0.75, 2.0]);

        let half_alpha_white = compose_scrgb_pixel([0.0; 3], [0.5; 3], 0.5, 203.0);
        for channel in half_alpha_white {
            assert!((channel - 1.26875).abs() < 0.000001);
        }

        let half_alpha_white_over_scene = compose_scrgb_pixel(scene, [0.5; 3], 0.5, 203.0);
        let expected = [0.26875, 2.01875, 3.26875];
        for (actual, expected) in half_alpha_white_over_scene.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.000001);
        }
    }
}
