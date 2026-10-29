//! The files a profile has open, read by the engine each file is set to.

/// Which read of a file a store holds, so an unchanged file can be reused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub(crate) key: String,
    pub(crate) version: u64,
}
