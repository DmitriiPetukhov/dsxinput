/// Scaled axial dead zone mapping DualSense 0..=255 to Xbox -32768..=32767.
pub struct StickAxisFilter {
    deadzone: f32,
    previous_was_center: bool,
}

impl StickAxisFilter {
    pub fn new(deadzone: f32) -> Self {
        assert!((0.0..1.0).contains(&deadzone));
        Self {
            deadzone,
            previous_was_center: false,
        }
    }

    pub fn apply(&mut self, raw: i32) -> Option<i32> {
        let offset = raw.clamp(0, 255) - 128;
        let normalized = if offset <= 0 {
            offset as f32 / 128.0
        } else {
            offset as f32 / 127.0
        };
        let at_center = normalized.abs() <= self.deadzone;
        let repeated_center = at_center && self.previous_was_center;
        self.previous_was_center = at_center;

        if repeated_center {
            return None;
        }

        let scaled = if at_center {
            0.0
        } else {
            normalized.signum() * (normalized.abs() - self.deadzone) / (1.0 - self.deadzone)
        };
        let output = if scaled <= 0.0 {
            scaled * 32768.0
        } else {
            scaled * 32767.0
        };

        Some(output.round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::StickAxisFilter;

    #[test]
    fn preserves_center_and_full_travel_without_deadzone() {
        let mut filter = StickAxisFilter::new(0.0);
        assert_eq!(filter.apply(0), Some(-32768));
        assert_eq!(filter.apply(128), Some(0));
        assert_eq!(filter.apply(255), Some(32767));
    }

    #[test]
    fn rescales_travel_outside_deadzone() {
        let mut filter = StickAxisFilter::new(0.5);
        assert_eq!(filter.apply(0), Some(-32768));
        assert_eq!(filter.apply(32), Some(-16384));
        assert_eq!(filter.apply(63), Some(-512));
        assert_eq!(filter.apply(64), Some(0));
        assert_eq!(filter.apply(191), None);
        assert_eq!(filter.apply(192), Some(258));
        assert_eq!(filter.apply(255), Some(32767));
    }

    #[test]
    fn suppresses_repeated_center_until_stick_leaves_deadzone() {
        let mut filter = StickAxisFilter::new(0.2);
        assert_eq!(filter.apply(128), Some(0));
        assert_eq!(filter.apply(129), None);
        assert!(filter.apply(200).unwrap() > 0);
        assert_eq!(filter.apply(128), Some(0));
    }

    #[test]
    fn clamps_out_of_range_input() {
        let mut filter = StickAxisFilter::new(0.2);
        assert_eq!(filter.apply(i32::MIN), Some(-32768));
        assert_eq!(filter.apply(i32::MAX), Some(32767));
    }
}
