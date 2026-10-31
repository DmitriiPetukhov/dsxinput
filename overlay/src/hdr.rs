use ash::vk;
use std::{env, ffi::OsStr, sync::OnceLock};

const DEFAULT_HDR_WHITE_NITS: f32 = 203.0;
const MIN_HDR_WHITE_NITS: f32 = 80.0;
const MAX_HDR_WHITE_NITS: f32 = 1000.0;

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
    use super::{classify_output_color_mode, parse_hdr_white_nits, OutputColorMode};
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
}
