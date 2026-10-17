//! Settings shared by every window of a profile. A window that changes one
//! bumps the revision; the others notice on their next frame and catch up.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::application::keymap::KeymapFile;
use crate::application::theme::ThemeSettings;
use crate::domain::formatting::NumericFormat;
use crate::ui::font_picker::FontPreferences;

pub(super) type ColumnFormats = BTreeMap<String, BTreeMap<String, NumericFormat>>;

#[derive(Default)]
pub(crate) struct Profiles {
    pub revision: u64,
    pub keymaps: HashMap<PathBuf, KeymapFile>,
    pub themes: HashMap<PathBuf, ThemeSettings>,
    pub fonts: HashMap<PathBuf, FontPreferences>,
    pub query_limits: HashMap<PathBuf, u64>,
    pub column_formats: HashMap<PathBuf, ColumnFormats>,
}

thread_local! {
    static PROFILES: RefCell<Profiles> = RefCell::new(Profiles::default());
}

/// Reads or changes the shared settings. Changes should bump `revision`
/// through [`changed`].
pub(crate) fn with<R>(f: impl FnOnce(&mut Profiles) -> R) -> R {
    PROFILES.with(|profiles| f(&mut profiles.borrow_mut()))
}

/// Changes the shared settings and tells other windows to catch up.
pub(crate) fn changed<R>(f: impl FnOnce(&mut Profiles) -> R) -> R {
    with(|profiles| {
        let result = f(profiles);
        profiles.revision += 1;
        result
    })
}

pub(crate) fn revision() -> u64 {
    with(|profiles| profiles.revision)
}
