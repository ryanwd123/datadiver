//! Overlays drawn inside the editor: the find/replace widget and go to line.
//! Their state, geometry and hit testing live here; painting is in `render`.

pub mod find;
pub mod goto;
pub mod input;
pub mod suggest;

/// A rectangle in editor DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Rect {
    pub const fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Rect { l, t, r, b }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }

    pub fn w(&self) -> f32 {
        self.r - self.l
    }

    pub fn h(&self) -> f32 {
        self.b - self.t
    }
}

/// Which part of the editor has keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Text,
    Find,
    Replace,
    GoTo,
}
