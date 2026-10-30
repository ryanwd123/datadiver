//! Keeps a read into memory within what the machine can hold. Before a file
//! is read, its size in memory is estimated from the file and compared with
//! the limit, a share of the machine's memory, and with the memory free now.

use std::fmt;
use std::path::Path;

use duckdb::Connection;

use super::polars::readers::{FileKind, file_kind, is_gzip_csv};

/// The share of the machine's memory a read may take, by default.
pub const DEFAULT_LIMIT_PERCENT: u8 = 50;
/// The shares the settings cycle through.
pub const LIMIT_PERCENTS: [u8; 4] = [25, 50, 75, 90];
/// Memory kept free for the rest of the machine, as a share of its memory.
const RESERVE_PERCENT: u64 = 10;

/// The machine's memory, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
}

#[cfg(windows)]
pub fn system_memory() -> Option<Memory> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with its length set.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(Memory {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,
    })
}

#[cfg(not(windows))]
pub fn system_memory() -> Option<Memory> {
    None
}

/// Why a file is not read into memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryRefusal {
    pub file: String,
    pub estimate: u64,
    pub allowed: u64,
    /// The limit is what is free now, not the share of memory.
    pub free_now: bool,
    pub percent: u8,
    pub total: u64,
}

impl fmt::Display for MemoryRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} needs about {} in memory, ",
            self.file,
            format_bytes(self.estimate)
        )?;
        if self.free_now {
            write!(f, "but only {} is free", format_bytes(self.allowed))
        } else {
            write!(
                f,
                "over the {} limit ({}% of {})",
                format_bytes(self.allowed),
                self.percent,
                format_bytes(self.total)
            )
        }
    }
}

/// Refuses reading `path` into memory when its estimated size does not fit.
/// A file whose size cannot be estimated, or a machine whose memory cannot
/// be read, is allowed.
pub fn check_read(path: &Path, percent: u8) -> Result<(), MemoryRefusal> {
    let (Some(memory), Some(estimate)) = (system_memory(), estimate_read(path)) else {
        return Ok(());
    };
    check(path, estimate, memory, percent)
}

fn check(path: &Path, estimate: u64, memory: Memory, percent: u8) -> Result<(), MemoryRefusal> {
    let percent = percent.clamp(1, 100);
    let limit = memory.total.saturating_mul(u64::from(percent)) / 100;
    let free = memory
        .available
        .saturating_sub(memory.total.saturating_mul(RESERVE_PERCENT) / 100);
    let (allowed, free_now) = if free < limit { (free, true) } else { (limit, false) };
    if estimate <= allowed {
        return Ok(());
    }
    Err(MemoryRefusal {
        file: path
            .file_name()
            .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned()),
        estimate,
        allowed,
        free_now,
        percent,
        total: memory.total,
    })
}

/// About how much memory reading `path` takes at its peak. The factors lean
/// high: a read parses text into columns and may parse it twice when a late
/// value breaks the sampled types.
pub fn estimate_read(path: &Path) -> Option<u64> {
    #[cfg(test)]
    if let Some(&estimate) = test_estimates().lock().unwrap().get(path) {
        return Some(estimate);
    }
    let size = std::fs::metadata(path).ok()?.len();
    let estimate = match file_kind(path).ok()? {
        FileKind::Delimited(_) if is_gzip_csv(path) => size.saturating_mul(8),
        // A 444 MB CSV peaked near 1 GB in Polars, counting the mapped file.
        FileKind::Delimited(_) => size.saturating_mul(2),
        // Zipped XML, held as cells while it is read.
        FileKind::Xlsx => size.saturating_mul(10),
        FileKind::Parquet => parquet_uncompressed(path)
            .map_or_else(|| size.saturating_mul(4), |bytes| bytes.saturating_mul(5) / 4),
    };
    Some(estimate)
}

/// Estimates tests give their own files, to stand for files too big to write.
#[cfg(test)]
fn test_estimates() -> &'static std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, u64>> {
    static ESTIMATES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, u64>>> =
        std::sync::OnceLock::new();
    ESTIMATES.get_or_init(Default::default)
}

/// Makes `path` estimate as `bytes` in memory, in this test process.
#[cfg(test)]
pub fn set_test_estimate(path: &Path, bytes: u64) {
    test_estimates().lock().unwrap().insert(path.to_path_buf(), bytes);
}

/// The size of a Parquet file's columns uncompressed, from its footer.
fn parquet_uncompressed(path: &Path) -> Option<u64> {
    let connection = Connection::open_in_memory().ok()?;
    connection
        .query_row(
            "SELECT sum(total_uncompressed_size)::UBIGINT FROM parquet_metadata(?)",
            [path.to_string_lossy().as_ref()],
            |row| row.get::<_, Option<u64>>(0),
        )
        .ok()
        .flatten()
}

pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else {
        format!("{:.0} MB", (bytes / MB).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_read_must_fit_the_share_of_memory_and_what_is_free() {
        let path = Path::new("big.csv");
        let memory = Memory {
            total: 32 * GB,
            available: 28 * GB,
        };
        assert!(check(path, 15 * GB, memory, 50).is_ok());
        let refusal = check(path, 20 * GB, memory, 50).unwrap_err();
        assert!(!refusal.free_now);
        assert_eq!(refusal.allowed, 16 * GB);
        assert_eq!(
            refusal.to_string(),
            "big.csv needs about 20.0 GB in memory, over the 16.0 GB limit (50% of 32.0 GB)"
        );
        // A 90% limit still keeps a tenth of memory for the rest of the machine.
        let tight = Memory {
            total: 32 * GB,
            available: 10 * GB,
        };
        let refusal = check(path, 8 * GB, tight, 90).unwrap_err();
        assert!(refusal.free_now);
        assert!(refusal.to_string().ends_with("but only 6.8 GB is free"));
    }

    #[test]
    fn estimates_lean_on_the_file_format() {
        let home = tempfile::tempdir().unwrap();
        let csv = home.path().join("rows.csv");
        std::fs::write(&csv, "a,b\n".repeat(1000)).unwrap();
        assert_eq!(estimate_read(&csv), Some(8000));
        assert_eq!(estimate_read(&home.path().join("missing.csv")), None);
        assert_eq!(estimate_read(&home.path().join("notes.txt")), None);

        let parquet = home.path().join("rows.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute(
                "COPY (SELECT range AS n, range::VARCHAR AS s FROM range(100000)) TO ? (FORMAT parquet)",
                [parquet.to_string_lossy().as_ref()],
            )
            .unwrap();
        let uncompressed = parquet_uncompressed(&parquet).unwrap();
        assert!(uncompressed > std::fs::metadata(&parquet).unwrap().len() / 2);
        assert_eq!(estimate_read(&parquet), Some(uncompressed * 5 / 4));
    }

    #[test]
    fn the_machine_reports_its_memory() {
        let memory = system_memory().unwrap();
        assert!(memory.total > 0 && memory.available <= memory.total);
    }
}
