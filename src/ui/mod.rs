pub mod actions_editor;
pub mod ai_settings;
pub mod aggregation_editor;
pub mod cell_detail;
pub mod column_completion;
mod command_palette;
pub mod database_tree;
pub mod dataframe_view;
mod font_picker;
pub mod grid;
mod help;
pub mod info;
pub mod input_with_autocompletion;
pub mod keymap_editor;
pub mod order_by_panel;
pub mod pivot_panel;
mod query_limit;
pub mod slicer_sidebar;
pub mod space_menu;
mod sql_panel;
mod status;
pub mod theme_editor;
mod window_placement;

// Only the icons the app uses are embedded; others load as nothing.
gpui_kit::assets::icon_assets!(pub AppIcons, [Search]);
