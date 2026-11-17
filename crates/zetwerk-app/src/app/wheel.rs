//! What a wheel or a touchpad asks of the view.
//!
//! Pinching to zoom: winit 0.30 reports pinches (`PinchGesture`) on
//! macOS and iOS only. On Wayland they would come by the pointer
//! gestures protocol on the window's own connection, which winit does
//! not bind and which cannot be reached from here without unsafe code;
//! sway sends a pinch to no client that has not bound it. There, Ctrl
//! with two fingers (which a touchpad sends as Ctrl and the wheel) zooms
//! about the pointer, as Ctrl and the wheel do.
//!
//! A mouse wheel turns by notches (`LineDelta`), each a fixed distance; a
//! touchpad says how far the fingers went (`PixelDelta`), which winit
//! gives in device pixels already: the page goes as far as the fingers,
//! at any scale of the display. On Wayland a touchpad's swipe ends with
//! an event that moves nothing (its fingers were lifted): the window has
//! nothing to draw for it. Sway sends no momentum after the fingers are
//! lifted, so the page stops where they left it, as it does in a
//! terminal; where a compositor sends momentum, it comes as more of the
//! same events and is followed like them.

use winit::event::MouseScrollDelta;

/// How far `delta` scrolls, across and down, in device pixels, where a
/// notch of a wheel is `line` of them. With Shift held, a wheel that
/// turns only up and down scrolls across, as elsewhere.
pub(super) fn delta(delta: MouseScrollDelta, line: f64, shift: bool) -> (f64, f64) {
    let (dx, dy) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (f64::from(x) * line, f64::from(y) * line),
        MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
    };
    if shift && dx == 0.0 {
        (dy, 0.0)
    } else {
        (dx, dy)
    }
}

/// How much a pinch that grew by `delta` (of the distance between the
/// fingers; less than nothing for one that shrank) zooms: as much as the
/// fingers spread, and never to nothing or less, however fast.
pub(super) fn pinch(delta: f64) -> f64 {
    (1.0 + delta).clamp(0.5, 2.0)
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    #[test]
    fn a_notch_is_a_line_and_a_touchpad_goes_as_far_as_the_fingers() {
        let line = 90.0;
        assert_eq!(
            delta(MouseScrollDelta::LineDelta(0.0, -1.0), line, false),
            (0.0, -90.0)
        );
        let swipe = MouseScrollDelta::PixelDelta(PhysicalPosition::new(3.0, -12.5));
        assert_eq!(delta(swipe, line, false), (3.0, -12.5));
        // Lifting the fingers moves nothing.
        let lifted = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0));
        assert_eq!(delta(lifted, line, false), (0.0, 0.0));
    }

    #[test]
    fn shift_turns_the_wheel_sideways() {
        let line = 60.0;
        assert_eq!(
            delta(MouseScrollDelta::LineDelta(0.0, 1.0), line, true),
            (60.0, 0.0)
        );
        // A touchpad that goes sideways itself is left as it is.
        let swipe = MouseScrollDelta::PixelDelta(PhysicalPosition::new(4.0, 1.0));
        assert_eq!(delta(swipe, line, true), (4.0, 1.0));
    }

    #[test]
    fn a_pinch_zooms_as_far_as_the_fingers_spread() {
        assert!((pinch(0.1) - 1.1).abs() < 1e-12);
        assert!((pinch(-0.2) - 0.8).abs() < 1e-12);
        assert!((pinch(0.0) - 1.0).abs() < 1e-12);
        // A wild report zooms a step, not to nothing.
        assert!((pinch(-3.0) - 0.5).abs() < 1e-12);
    }
}
