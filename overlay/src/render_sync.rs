use ash::vk;

/// Tracks the application-visible image cycle and the overlay's private resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImagePhase {
    Available,
    Acquired,
    Submitted,
    Presented,
    Poisoned,
}

pub(crate) struct ImageSync {
    phase: ImagePhase,
    command_in_flight: bool,
    output_waiting_for_reacquire: bool,
}

impl ImageSync {
    pub(crate) fn new() -> Self {
        Self {
            phase: ImagePhase::Available,
            command_in_flight: false,
            output_waiting_for_reacquire: false,
        }
    }

    /// An acquire confirms the preceding present consumed the image's output semaphore.
    pub(crate) fn acquired(&mut self) -> bool {
        if self.phase == ImagePhase::Poisoned {
            return false;
        }
        if !matches!(self.phase, ImagePhase::Available | ImagePhase::Presented) {
            return false;
        }
        self.phase = ImagePhase::Acquired;
        self.output_waiting_for_reacquire = false;
        true
    }

    pub(crate) fn begin_submission(&mut self) -> bool {
        if !self.can_submit() {
            return false;
        }
        self.phase = ImagePhase::Submitted;
        self.command_in_flight = true;
        self.output_waiting_for_reacquire = true;
        true
    }

    pub(crate) fn is_acquired(&self) -> bool {
        self.phase == ImagePhase::Acquired
    }

    pub(crate) fn can_submit(&self) -> bool {
        self.phase == ImagePhase::Acquired
            && !self.command_in_flight
            && !self.output_waiting_for_reacquire
    }

    pub(crate) fn fence_complete(&mut self) {
        self.command_in_flight = false;
    }

    pub(crate) fn presented(&mut self, result: vk::Result) -> bool {
        if self.phase != ImagePhase::Submitted {
            self.poison();
            return false;
        }
        if matches!(result, vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR) {
            self.phase = ImagePhase::Presented;
            true
        } else {
            self.poison();
            false
        }
    }

    pub(crate) fn presented_without_submission(&mut self, result: vk::Result) {
        if !matches!(result, vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR) {
            self.poison();
        } else if self.phase == ImagePhase::Acquired {
            self.phase = ImagePhase::Presented;
        }
    }

    pub(crate) fn submission_failed(&mut self) {
        self.poison();
    }

    fn poison(&mut self) {
        self.phase = ImagePhase::Poisoned;
        self.command_in_flight = true;
        self.output_waiting_for_reacquire = true;
    }

    pub(crate) fn command_reusable(&self) -> bool {
        !self.command_in_flight && self.phase != ImagePhase::Poisoned
    }

    pub(crate) fn output_reusable(&self) -> bool {
        !self.output_waiting_for_reacquire && self.phase != ImagePhase::Poisoned
    }

    pub(crate) fn teardown_ready(&self) -> bool {
        !self.command_in_flight
    }

    pub(crate) fn device_idle(&mut self) {
        self.command_in_flight = false;
    }
}

/// Gives a whole present batch's application waits to exactly one queue submission.
pub(crate) struct BatchWaits<'a> {
    input_waits: &'a [vk::Semaphore],
    consumed: bool,
}

impl<'a> BatchWaits<'a> {
    pub(crate) fn new(input_waits: &'a [vk::Semaphore]) -> Self {
        Self {
            input_waits,
            consumed: false,
        }
    }

    pub(crate) fn take_for_submission(&mut self) -> Option<&'a [vk::Semaphore]> {
        if self.consumed {
            return None;
        }
        self.consumed = true;
        Some(self.input_waits)
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchWaits, ImageSync};
    use ash::vk::{self, Handle};

    #[test]
    fn batch_input_wait_semaphores_are_claimed_for_one_submission_only() {
        let waits = [
            vk::Semaphore::from_raw(0x101),
            vk::Semaphore::from_raw(0x102),
        ];
        let mut batch = BatchWaits::new(&waits);

        assert_eq!(batch.take_for_submission(), Some(waits.as_slice()));
        assert_eq!(batch.take_for_submission(), None);
    }

    #[test]
    fn command_buffer_reuse_waits_for_its_submission_fence() {
        let mut image = ImageSync::new();

        assert!(image.acquired());
        assert!(image.begin_submission());
        assert!(image.presented(vk::Result::SUCCESS));
        assert!(image.acquired());
        assert!(!image.can_submit());
        assert!(!image.command_reusable());
        image.fence_complete();
        assert!(image.can_submit());
        assert!(image.command_reusable());
    }

    #[test]
    fn output_semaphore_reuse_waits_for_image_reacquisition() {
        let mut image = ImageSync::new();

        assert!(image.acquired());
        assert!(image.begin_submission());
        image.fence_complete();
        assert!(image.presented(vk::Result::SUCCESS));
        assert!(!image.output_reusable());

        assert!(image.acquired());
        assert!(image.output_reusable());
    }

    #[test]
    fn submission_and_presentation_errors_poison_image_synchronization() {
        let mut failed_submission = ImageSync::new();
        assert!(failed_submission.acquired());
        assert!(failed_submission.begin_submission());
        failed_submission.submission_failed();
        assert!(!failed_submission.command_reusable());
        assert!(!failed_submission.output_reusable());
        assert!(!failed_submission.acquired());

        let mut failed_present = ImageSync::new();
        assert!(failed_present.acquired());
        assert!(failed_present.begin_submission());
        failed_present.fence_complete();
        assert!(!failed_present.presented(vk::Result::ERROR_OUT_OF_DATE_KHR));
        assert!(!failed_present.command_reusable());
        assert!(!failed_present.output_reusable());
        assert!(!failed_present.acquired());
    }

    #[test]
    fn teardown_waits_for_failed_submission_to_reach_device_idle() {
        let mut image = ImageSync::new();
        assert!(image.acquired());
        assert!(image.begin_submission());
        image.submission_failed();

        assert!(!image.teardown_ready());
        image.device_idle();
        assert!(image.teardown_ready());
        assert!(!image.command_reusable());
    }
}
