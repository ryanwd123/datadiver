//! A small UI platform on Win32, Direct2D, and DirectWrite.

pub mod app;
pub mod geometry;
pub mod gfx;
pub mod headless;
pub mod keys;
pub mod system;
pub mod text;
pub mod window;

pub use app::{Cx, Event, MouseButton, Placement, Runtime, Task, View, WindowId, WindowOptions};
pub use geometry::{Color, Point, Rect, point, rect};
pub use gfx::{Canvas, Cursor, HitRegion};
pub use keys::{KeyEvent, Modifiers};
pub use text::{Align, Font, TextLayout, TextOptions, Weight};
