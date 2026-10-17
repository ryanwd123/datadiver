//! Scroll offsets and the always-visible scroll bars drawn beside content.

use crate::ui::platform::{Canvas, Color, Rect, rect};

pub const BAR_WIDTH: f32 = 12.0;
const THUMB_WIDTH: f32 = 6.0;
const MIN_THUMB: f32 = 24.0;

/// One axis of scrolling: how far the content has moved, and a thumb drag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub offset: f32,
    /// Where the thumb was grabbed, from the thumb's start, while dragging.
    grab: Option<f32>,
}

/// Where the thumb sits in a track of `length`, as (start, size).
fn thumb(length: f32, viewport: f32, content: f32, offset: f32) -> Option<(f32, f32)> {
    if content <= viewport + 0.5 || viewport <= 0.0 || length <= 0.0 {
        return None;
    }
    let size = (length * viewport / content).clamp(MIN_THUMB.min(length), length);
    let max = content - viewport;
    let start = if max > 0.0 {
        offset.clamp(0.0, max) / max * (length - size)
    } else {
        0.0
    };
    Some((start, size))
}

impl Scroll {
    pub fn max(viewport: f32, content: f32) -> f32 {
        (content - viewport).max(0.0)
    }

    pub fn clamp(&mut self, viewport: f32, content: f32) {
        self.offset = self.offset.clamp(0.0, Self::max(viewport, content));
    }

    /// Scrolls by `delta` content pixels. Returns whether the offset moved.
    pub fn scroll_by(&mut self, delta: f32, viewport: f32, content: f32) -> bool {
        let before = self.offset;
        self.offset = (self.offset + delta).clamp(0.0, Self::max(viewport, content));
        self.offset != before
    }

    /// Moves the offset the least needed to show `start..start + size`.
    pub fn reveal(&mut self, start: f32, size: f32, viewport: f32) {
        if start < self.offset {
            self.offset = start;
        } else if start + size > self.offset + viewport {
            self.offset = start + size - viewport;
        }
        self.offset = self.offset.max(0.0);
    }

    pub fn dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// Starts dragging at `at` along a track of `length`. A press outside the
    /// thumb centers the thumb there first.
    pub fn press(&mut self, at: f32, length: f32, viewport: f32, content: f32) {
        let Some((start, size)) = thumb(length, viewport, content, self.offset) else {
            return;
        };
        if at >= start && at <= start + size {
            self.grab = Some(at - start);
        } else {
            self.grab = Some(size / 2.0);
            self.drag(at, length, viewport, content);
        }
    }

    pub fn drag(&mut self, at: f32, length: f32, viewport: f32, content: f32) {
        let Some(grab) = self.grab else {
            return;
        };
        let Some((_, size)) = thumb(length, viewport, content, self.offset) else {
            return;
        };
        let travel = (length - size).max(0.0);
        if travel <= 0.0 {
            return;
        }
        let start = (at - grab).clamp(0.0, travel);
        self.offset = start / travel * Self::max(viewport, content);
    }

    pub fn release(&mut self) -> bool {
        self.grab.take().is_some()
    }
}

/// Draws a vertical bar in `track` and answers presses on it with `hit`.
pub fn vertical<H: Clone>(canvas: &mut Canvas<H>, track: Rect, viewport: f32, content: f32, scroll: &Scroll, hit: H) {
    let Some((start, size)) = thumb(track.h, viewport, content, scroll.offset) else {
        return;
    };
    let hovered = canvas.hovered(track) || scroll.dragging();
    let thumb = rect(
        track.x + (track.w - THUMB_WIDTH) / 2.0,
        track.y + start,
        THUMB_WIDTH,
        size,
    );
    canvas.fill_rounded(
        thumb,
        3.0,
        Color::rgba(if hovered { 0x00000066 } else { 0x00000033 }),
    );
    canvas.hit(track, hit);
}

pub fn horizontal<H: Clone>(canvas: &mut Canvas<H>, track: Rect, viewport: f32, content: f32, scroll: &Scroll, hit: H) {
    let Some((start, size)) = thumb(track.w, viewport, content, scroll.offset) else {
        return;
    };
    let hovered = canvas.hovered(track) || scroll.dragging();
    let thumb = rect(
        track.x + start,
        track.y + (track.h - THUMB_WIDTH) / 2.0,
        size,
        THUMB_WIDTH,
    );
    canvas.fill_rounded(
        thumb,
        3.0,
        Color::rgba(if hovered { 0x00000066 } else { 0x00000033 }),
    );
    canvas.hit(track, hit);
}

pub fn has_bar(viewport: f32, content: f32) -> bool {
    content > viewport + 0.5
}
