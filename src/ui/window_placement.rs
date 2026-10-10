use gpui_kit::{Bounds, Pixels, WindowBounds, point, px, size};

use crate::infrastructure::json_store::WindowGeometry;

pub(super) fn geometry(bounds: WindowBounds) -> WindowGeometry {
    let rectangle = bounds.get_bounds();
    WindowGeometry {
        x: rectangle.origin.x.into(),
        y: rectangle.origin.y.into(),
        width: rectangle.size.width.into(),
        height: rectangle.size.height.into(),
        maximized: matches!(bounds, WindowBounds::Maximized(_)),
    }
}

pub(super) fn restored_bounds(
    saved: &WindowGeometry,
    displays: &[Bounds<Pixels>],
) -> Option<WindowBounds> {
    if ![saved.x, saved.y, saved.width, saved.height]
        .iter()
        .all(|value| value.is_finite())
        || saved.width <= 0.0
        || saved.height <= 0.0
    {
        return None;
    }
    let mut rectangle = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    if let Some(display) = displays
        .iter()
        .find(|display| display.contains(&rectangle.origin))
        .or_else(|| displays.first())
    {
        rectangle.size = rectangle.size.min(&display.size);
        rectangle.origin.x = rectangle.origin.x.clamp(
            display.origin.x,
            display.origin.x + display.size.width - rectangle.size.width,
        );
        rectangle.origin.y = rectangle.origin.y.clamp(
            display.origin.y,
            display.origin.y + display.size.height - rectangle.size.height,
        );
    }
    Some(if saved.maximized {
        WindowBounds::Maximized(rectangle)
    } else {
        WindowBounds::Windowed(rectangle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trips_on_a_secondary_display() {
        let bounds = WindowBounds::Maximized(Bounds::new(
            point(px(-1200.), px(80.)),
            size(px(900.), px(600.)),
        ));
        let display = Bounds::new(point(px(-1600.), px(0.)), size(px(1600.), px(900.)));
        assert_eq!(restored_bounds(&geometry(bounds), &[display]), Some(bounds));
    }

    #[test]
    fn removed_display_and_oversized_window_are_kept_on_screen() {
        let bounds = WindowBounds::Windowed(Bounds::new(
            point(px(-2500.), px(2000.)),
            size(px(2000.), px(1200.)),
        ));
        let display = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(720.)));
        assert_eq!(
            restored_bounds(&geometry(bounds), &[display]),
            Some(WindowBounds::Windowed(display))
        );
    }

    #[test]
    fn invalid_size_is_ignored() {
        let mut saved = geometry(WindowBounds::default());
        assert!(restored_bounds(&saved, &[]).is_none());
        saved.width = f32::NAN;
        assert!(restored_bounds(&saved, &[]).is_none());
    }
}
