pub mod application;
pub mod cli;
pub mod domain;
pub mod infrastructure;
pub mod ui;

#[cfg(test)]
mod stress;

pub use domain::frame::{FrameSnapshot, ResultSnapshot};
pub use ui::dataframe_view::DataFrameView;
