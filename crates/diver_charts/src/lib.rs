//! Charts for DataDiver.
//!
//! A chart is a plain description: its data and options. `layout` turns it
//! into a [`Scene`] of rectangles, lines and text in device-independent
//! pixels, plus the marks the pointer can land on and their tooltips. The
//! crate draws nothing itself: the app draws scenes with Direct2D (see the
//! gallery example), and the same layout can serve the Python package with
//! another backend. Only text sizes come from the backend, through
//! [`Measure`].

pub mod bar;
pub mod format;
pub mod geom;
pub mod scale;
pub mod scene;
pub mod theme;

pub use bar::{BarChart, Orientation, Series};
pub use geom::{Color, Point, Rect};
pub use scene::{Anchor, Approx, Baseline, Corners, Item, Mark, Measure, Scene, Shape, Tooltip};
pub use theme::{Font, Theme};
