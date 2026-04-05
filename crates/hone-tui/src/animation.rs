use std::time::Instant;

const SPINNER_FRAMES: &[char] = &['\u{2500}', '\\', '\u{2502}', '/'];

/// Tracks animation state for spinners, blink cursors, and elapsed time.
#[derive(Debug, Clone)]
pub struct AnimationState {
    start: Instant,
    frame: u64,
}

impl AnimationState {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            frame: 0,
        }
    }

    /// Advance one tick (call at 100ms intervals).
    pub fn tick(&mut self) {
        self.frame += 1;
    }

    /// Current frame number.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Current spinner character (4-frame cycle).
    pub fn spinner_char(&self) -> char {
        SPINNER_FRAMES[(self.frame as usize) % SPINNER_FRAMES.len()]
    }

    /// Whether the blink cursor should be visible (500ms on / 500ms off).
    pub fn blink_visible(&self) -> bool {
        (self.frame % 10) < 5
    }

    /// Seconds elapsed since animation started.
    pub fn elapsed_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    /// Format elapsed time as MM:SS.
    pub fn format_elapsed(&self) -> String {
        let s = self.elapsed_secs();
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

impl Default for AnimationState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_cycles_through_frames() {
        let mut anim = AnimationState::new();
        let mut chars = Vec::new();
        for _ in 0..4 {
            chars.push(anim.spinner_char());
            anim.tick();
        }
        assert_eq!(chars.len(), 4);
        assert_ne!(chars[0], chars[1]);
        assert_ne!(chars[1], chars[2]);
        assert_ne!(chars[2], chars[3]);
    }

    #[test]
    fn test_blink_toggles() {
        let mut anim = AnimationState::new();
        assert!(anim.blink_visible()); // frame 0: visible
        for _ in 0..5 {
            anim.tick();
        }
        assert!(!anim.blink_visible()); // frame 5: hidden
        for _ in 0..5 {
            anim.tick();
        }
        assert!(anim.blink_visible()); // frame 10: visible again
    }

    #[test]
    fn test_tick_increments_frame() {
        let mut anim = AnimationState::new();
        assert_eq!(anim.frame(), 0);
        anim.tick();
        assert_eq!(anim.frame(), 1);
        anim.tick();
        assert_eq!(anim.frame(), 2);
    }

    #[test]
    fn test_format_elapsed() {
        let anim = AnimationState::new();
        let formatted = anim.format_elapsed();
        assert_eq!(formatted, "00:00");
    }
}
