use crate::infrastructure::json_store::WindowGeometry;
use crate::ui::platform::{Placement, Rect, rect};

pub(super) fn geometry(placement: Placement) -> WindowGeometry {
    WindowGeometry {
        x: placement.x,
        y: placement.y,
        width: placement.width,
        height: placement.height,
        maximized: placement.maximized,
    }
}

/// Where a window saved as `saved` opens, kept on one of `displays` (work
/// areas in screen pixels).
pub(super) fn restored_bounds(saved: &WindowGeometry, displays: &[Rect]) -> Option<Placement> {
    if ![saved.x, saved.y, saved.width, saved.height]
        .iter()
        .all(|value| value.is_finite())
        || saved.width <= 0.0
        || saved.height <= 0.0
    {
        return None;
    }
    let mut window = rect(saved.x, saved.y, saved.width, saved.height);
    if let Some(display) = displays
        .iter()
        .find(|display| display.contains(window.origin()))
        .or_else(|| displays.first())
    {
        window.w = window.w.min(display.w);
        window.h = window.h.min(display.h);
        window.x = window.x.clamp(display.x, display.x + display.w - window.w);
        window.y = window.y.clamp(display.y, display.y + display.h - window.h);
    }
    Some(Placement {
        x: window.x,
        y: window.y,
        width: window.w,
        height: window.h,
        maximized: saved.maximized,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trips_on_a_secondary_display() {
        let placement = Placement {
            x: -1200.,
            y: 80.,
            width: 900.,
            height: 600.,
            maximized: true,
        };
        let display = rect(-1600., 0., 1600., 900.);
        assert_eq!(
            restored_bounds(&geometry(placement), &[display]),
            Some(placement)
        );
    }

    #[test]
    fn removed_display_and_oversized_window_are_kept_on_screen() {
        let placement = Placement {
            x: -2500.,
            y: 2000.,
            width: 2000.,
            height: 1200.,
            maximized: false,
        };
        let display = rect(0., 0., 1280., 720.);
        assert_eq!(
            restored_bounds(&geometry(placement), &[display]),
            Some(Placement {
                x: 0.,
                y: 0.,
                width: 1280.,
                height: 720.,
                maximized: false
            })
        );
    }

    #[test]
    fn invalid_size_is_ignored() {
        let mut saved = WindowGeometry {
            x: 0.,
            y: 0.,
            width: 0.,
            height: 0.,
            maximized: false,
        };
        assert!(restored_bounds(&saved, &[]).is_none());
        saved.width = f32::NAN;
        assert!(restored_bounds(&saved, &[]).is_none());
    }
}
