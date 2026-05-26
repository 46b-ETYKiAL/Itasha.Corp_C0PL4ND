//! C0PL4ND GPU renderer.
//!
//! Draws the terminal grid produced by `c0pl4nd-core` using a glyph atlas and
//! damage/dirty-region tracking so an idle terminal issues zero redraws
//! (render-on-input). The GPU backend is filled in during the renderer phase;
//! this crate currently exposes the frame-scheduling contract.

/// Frame-scheduling policy. C0PL4ND renders on input rather than on a fixed
/// clock — the single biggest perceived-latency and battery lever per the
/// best-in-class research.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FramePolicy {
    /// Redraw only when the grid is marked damaged (default).
    #[default]
    OnDamage,
    /// Redraw every vsync — used only for the optional CRT animation effect.
    Continuous,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_on_damage() {
        assert_eq!(FramePolicy::default(), FramePolicy::OnDamage);
    }
}
