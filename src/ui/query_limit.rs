use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct QueryLimitRuntime {
    pub profiles: HashMap<PathBuf, u64>,
}

impl gpui_kit::Global for QueryLimitRuntime {}
