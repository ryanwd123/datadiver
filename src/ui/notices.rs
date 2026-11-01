//! Status and error messages: a toast that pops up in a window's top-right
//! corner for a moment, and a log of every message behind a toolbar button.

use std::time::{Duration, Instant};

use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Color, Cursor, Point, Rect, TextOptions, point, rect, text};
use crate::ui::widgets::{ButtonStyle, Fonts, button, button_width, colors};

/// How long a toast stays up; errors stay a little longer.
const STATUS_TOAST: Duration = Duration::from_millis(2500);
const ERROR_TOAST: Duration = Duration::from_millis(4500);
/// The oldest messages are dropped past this many.
const LOG_LIMIT: usize = 200;
const TOAST_WIDTH: f32 = 420.0;
const LOG_WIDTH: f32 = 520.0;
const MAX_TOAST_LINES: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Status,
    Error,
}

impl NoticeKind {
    fn color(self) -> Color {
        match self {
            Self::Status => colors::SELECTED_STRONG,
            Self::Error => colors::DANGER,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
    /// The local time it was shown, as `HH:MM:SS`.
    pub time: String,
}

/// Where a window's messages come from. The view keeps each as a field that
/// holds the current message; [`Notices::observe`] notices when one changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeSource {
    Error,
    TablePreferences,
    Limit,
    Status,
    Statistics,
}

const SOURCES: usize = 5;

struct Toast {
    notice: Notice,
    until: Instant,
}

#[derive(Default)]
pub struct Notices {
    /// Oldest first.
    log: Vec<Notice>,
    /// The last message seen from each source.
    seen: [Option<String>; SOURCES],
    toast: Option<Toast>,
    pub log_open: bool,
    /// Errors logged since the log was last open.
    unread_errors: usize,
    scroll: f32,
}

impl Notices {
    pub fn log(&self) -> &[Notice] {
        &self.log
    }

    pub fn toast(&self) -> Option<&Notice> {
        self.toast.as_ref().map(|toast| &toast.notice)
    }

    pub fn unread_errors(&self) -> usize {
        self.unread_errors
    }

    /// Shows `message` from `source` if it is new since the last look.
    pub fn observe(&mut self, source: NoticeSource, message: Option<&str>, now: Instant) {
        let seen = &mut self.seen[source as usize];
        if seen.as_deref() == message {
            return;
        }
        *seen = message.map(str::to_owned);
        let Some(message) = message else {
            return;
        };
        // A status line can report that something failed.
        let kind = if source == NoticeSource::Status && !message.to_lowercase().contains("failed") {
            NoticeKind::Status
        } else {
            NoticeKind::Error
        };
        self.show(kind, message, now);
    }

    /// Pops up `message`, and logs it unless it only says work is under way.
    pub fn show(&mut self, kind: NoticeKind, message: &str, now: Instant) {
        let notice = Notice {
            kind,
            text: message.trim().to_owned(),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
        };
        if !message.trim_end().ends_with('…') {
            if kind == NoticeKind::Error && !self.log_open {
                self.unread_errors += 1;
            }
            self.log.push(notice.clone());
            if self.log.len() > LOG_LIMIT {
                self.log.remove(0);
            }
        }
        let duration = match kind {
            NoticeKind::Status => STATUS_TOAST,
            NoticeKind::Error => ERROR_TOAST,
        };
        self.toast = Some(Toast { notice, until: now + duration });
    }

    pub fn dismiss_toast(&mut self) {
        self.toast = None;
    }

    pub fn set_log_open(&mut self, open: bool) {
        self.log_open = open;
        self.scroll = 0.0;
        if open {
            self.unread_errors = 0;
            // The log shows the message; the toast would cover it.
            self.toast = None;
        }
    }

    pub fn clear(&mut self) {
        self.log.clear();
        self.unread_errors = 0;
        self.scroll = 0.0;
    }

    /// When the toast should go, so the window can redraw then.
    pub fn deadline(&self) -> Option<Instant> {
        self.toast.as_ref().map(|toast| toast.until)
    }

    /// Drops the toast once its time is up.
    pub fn expire(&mut self, now: Instant) {
        if self.toast.as_ref().is_some_and(|toast| toast.until <= now) {
            self.toast = None;
        }
    }

    /// Scrolls the open log; returns whether it moved.
    pub fn wheel(&mut self, delta: Point, line: f32) -> bool {
        if !self.log_open {
            return false;
        }
        let before = self.scroll;
        self.scroll = (self.scroll - delta.y * line * 3.0).max(0.0);
        self.scroll != before
    }

    /// The toolbar button that opens the log, with a count of unread errors.
    pub fn paint_button(&self, canvas: &mut Canvas<Hit>, r: Rect, font: &crate::ui::platform::Font) {
        let hit = Hit::Button(Button::NoticeLog);
        button(canvas, r, "", font, ButtonStyle::Ghost, true, hit.clone());
        let color = if self.unread_errors > 0 { colors::DANGER } else { colors::FOREGROUND };
        // Lines of a list, each with a bullet.
        let size = r.w.min(r.h) * 0.5;
        let (left, top) = (r.x + (r.w - size) / 2.0, r.y + (r.h - size) / 2.0);
        let stroke = (size * 0.14).max(1.5);
        for row in 0..3 {
            let y = top + size * (0.15 + 0.35 * row as f32);
            canvas.fill_rounded(rect(left, y - stroke / 2.0, stroke, stroke), stroke / 2.0, color);
            canvas.line(point(left + stroke * 2.2, y), point(left + size, y), color, stroke);
        }
        if self.unread_errors > 0 {
            let badge = (size * 0.55).max(7.0);
            canvas.fill_rounded(rect(r.right() - badge - 2.0, r.y + 2.0, badge, badge), badge / 2.0, colors::DANGER);
        }
        let tooltip = match self.unread_errors {
            0 => "Messages".to_owned(),
            1 => "Messages: 1 new error".to_owned(),
            count => format!("Messages: {count} new errors"),
        };
        canvas.hit_with(r, hit, Cursor::Arrow, Some(&tooltip));
    }

    /// The toast under the window's top-right corner, `top` being the
    /// toolbar's bottom. Hovering it keeps it up.
    pub fn paint_toast(&mut self, canvas: &mut Canvas<Hit>, bounds: Rect, top: f32, fonts: &Fonts, now: Instant) {
        let Some(toast) = &mut self.toast else {
            return;
        };
        let font = &fonts.ui;
        let line = text::line_height(font);
        let width = TOAST_WIDTH.min(bounds.w - 24.0).max(120.0);
        let text_width = width - 32.0;
        let layout = text::layout(&toast.notice.text, font, TextOptions::wrapped(text_width));
        let text_height = layout.height.min(line * MAX_TOAST_LINES);
        let r = rect(bounds.right() - 12.0 - width, top + 8.0, width, text_height + 20.0);
        if canvas.hovered(r) {
            toast.until = toast.until.max(now + Duration::from_millis(800));
        }
        canvas.shadow(r, 6.0);
        canvas.fill_rounded(r, 6.0, colors::POPOVER);
        canvas.stroke_rounded(r, 6.0, colors::BORDER, 1.0);
        canvas.fill_rounded(rect(r.x + 6.0, r.y + 8.0, 3.0, r.h - 16.0), 1.5, toast.notice.kind.color());
        let body = rect(r.x + 20.0, r.y + 10.0, text_width, text_height);
        canvas.push_clip(body);
        let color = match toast.notice.kind {
            NoticeKind::Status => colors::FOREGROUND,
            NoticeKind::Error => colors::DANGER,
        };
        canvas.draw_layout(&layout, body.origin(), color);
        canvas.pop_clip();
        canvas.hit_with(r, Hit::Button(Button::DismissNotice), Cursor::Arrow, Some("Click to dismiss"));
    }

    /// The log, newest first, under the window's top-right corner.
    pub fn paint_log(&mut self, canvas: &mut Canvas<Hit>, bounds: Rect, top: f32, fonts: &Fonts) {
        if !self.log_open {
            return;
        }
        let font = &fonts.ui;
        let small = &fonts.small;
        let line = text::line_height(font);
        let width = LOG_WIDTH.min(bounds.w - 24.0).max(160.0);
        let time_width = text::measure("00:00:00", small).0 + 12.0;
        let text_width = width - 32.0 - time_width;
        let layouts: Vec<_> = self
            .log
            .iter()
            .rev()
            .map(|notice| (notice, text::layout(&notice.text, font, TextOptions::wrapped(text_width))))
            .collect();
        let total: f32 = layouts.iter().map(|(_, layout)| layout.height + 8.0).sum::<f32>().max(line);
        // As tall as the messages, up to most of the window.
        let title_height = line + 16.0;
        let height = (title_height + total + 8.0).min((bounds.bottom() - top - 48.0).clamp(80.0, 420.0));
        let r = rect(bounds.right() - 12.0 - width, top + 4.0, width, height);
        canvas.shadow(r, 6.0);
        canvas.fill_rounded(r, 6.0, colors::POPOVER);
        canvas.stroke_rounded(r, 6.0, colors::BORDER, 1.0);
        canvas.hit(r, Hit::Surface);
        canvas.text(rect(r.x + 16.0, r.y + 8.0, width - 32.0, line), "Messages", &font.semibold(), colors::FOREGROUND, Align::Left);
        if !self.log.is_empty() {
            let clear = button_width("Clear", small);
            let button_rect = rect(r.right() - 12.0 - clear, r.y + 6.0, clear, line + 4.0);
            button(canvas, button_rect, "Clear", small, ButtonStyle::Ghost, true, Hit::Button(Button::ClearNotices));
        }
        let body = rect(r.x + 8.0, r.y + title_height, width - 16.0, r.h - title_height - 8.0);
        if self.log.is_empty() {
            canvas.text(rect(body.x + 8.0, body.y, body.w - 16.0, line), "No messages yet", font, colors::MUTED, Align::Left);
            return;
        }
        self.scroll = self.scroll.min((total - body.h).max(0.0));
        canvas.push_clip(body);
        let mut y = body.y - self.scroll;
        for (notice, layout) in &layouts {
            let entry = rect(body.x, y, body.w, layout.height + 8.0);
            if entry.bottom() > body.y && entry.y < body.bottom() {
                canvas.text(rect(entry.x + 8.0, y + 4.0, time_width, line), &notice.time, small, colors::MUTED, Align::Left);
                let color = match notice.kind {
                    NoticeKind::Status => colors::FOREGROUND,
                    NoticeKind::Error => colors::DANGER,
                };
                canvas.draw_layout(layout, point(entry.x + 8.0 + time_width, y + 4.0), color);
            }
            y += entry.h;
        }
        canvas.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_messages_are_logged_once_and_progress_is_not_logged() {
        let now = Instant::now();
        let mut notices = Notices::default();
        notices.observe(NoticeSource::Status, Some("Copying…"), now);
        assert_eq!(notices.toast().unwrap().text, "Copying…");
        assert!(notices.log().is_empty());
        notices.observe(NoticeSource::Status, Some("Copied"), now);
        notices.observe(NoticeSource::Status, Some("Copied"), now);
        assert_eq!(notices.log().len(), 1);
        assert_eq!(notices.log()[0].kind, NoticeKind::Status);
        // The same message again, after it was cleared, is new.
        notices.observe(NoticeSource::Status, None, now);
        notices.observe(NoticeSource::Status, Some("Copied"), now);
        assert_eq!(notices.log().len(), 2);
        notices.observe(NoticeSource::Status, Some("Export failed: disk full"), now);
        notices.observe(NoticeSource::Error, Some("Bad search"), now);
        assert_eq!(notices.log()[2].kind, NoticeKind::Error);
        assert_eq!(notices.unread_errors(), 2);
        notices.set_log_open(true);
        assert_eq!(notices.unread_errors(), 0);
        assert!(notices.toast().is_none());
    }

    #[test]
    fn toasts_go_after_their_time() {
        let now = Instant::now();
        let mut notices = Notices::default();
        notices.show(NoticeKind::Status, "Saved", now);
        notices.expire(now + Duration::from_millis(100));
        assert!(notices.toast().is_some());
        notices.expire(now + STATUS_TOAST);
        assert!(notices.toast().is_none());
        notices.show(NoticeKind::Error, "Failed", now);
        notices.expire(now + STATUS_TOAST);
        assert!(notices.toast().is_some());
        assert_eq!(notices.deadline(), Some(now + ERROR_TOAST));
    }
}
