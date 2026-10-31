/// Axial deadzone for a DualSense stick axis with raw range 0..=255.
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
        let normalized = raw as f32 / 255.0 * 2.0 - 1.0;
        let at_center = normalized.abs() <= self.deadzone;
        let repeated_center = at_center && self.previous_was_center;
        self.previous_was_center = at_center;

        if repeated_center {
            None
        } else {
            Some(if at_center { 128 } else { raw })
        }
    }
}
