//! Lets newer input stop a background job's DuckDB queries.
//!
//! A job runs inside [`Cancellation::scope`]. The store's long queries run
//! through [`interruptible`], which hands their connection to the job's
//! cancellation while they run, so cancelling interrupts them and fails every
//! query the job starts afterwards. Queries outside a job run as before.

use duckdb::{Connection, InterruptHandle};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The error a cancelled job's queries fail with.
pub const CANCELLED: &str = "The query was replaced by newer input";

#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    running: Mutex<Vec<Arc<InterruptHandle>>>,
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Cancellation>>> = const { RefCell::new(None) };
}

impl Cancellation {
    /// Interrupts the job's running query. DuckDB drops an interrupt that
    /// arrives before a query starts, so a query that registered but has not
    /// started yet still runs to the end; its result is discarded.
    pub fn cancel(&self) {
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        self.cancelled.store(true, Ordering::Release);
        for handle in running.iter() {
            handle.interrupt();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Runs `job` on this thread with this cancellation current.
    pub fn scope<T>(self: &Arc<Self>, job: impl FnOnce() -> T) -> T {
        struct Restore(Option<Arc<Cancellation>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|current| current.replace(Some(self.clone()))));
        job()
    }
}

fn current() -> Option<Arc<Cancellation>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Fails once the current job is cancelled. Work that turns query errors
/// into results, such as a slicer card's error, checks this before the
/// results are kept.
pub fn check() -> Result<(), String> {
    match current() {
        Some(cancellation) if cancellation.is_cancelled() => Err(CANCELLED.into()),
        _ => Ok(()),
    }
}

/// Runs `query` on `connection` so that cancelling the current job
/// interrupts it.
pub(crate) fn interruptible<T>(
    connection: &Connection,
    query: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let Some(cancellation) = current() else {
        return query();
    };
    {
        let mut running = cancellation.running.lock().unwrap_or_else(PoisonError::into_inner);
        if cancellation.is_cancelled() {
            return Err(CANCELLED.into());
        }
        running.push(connection.interrupt_handle());
    }
    let result = query();
    // Calls nest on one thread, so the last handle is this query's.
    cancellation
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pop();
    // An interrupted query fails with DuckDB's message; name the reason instead.
    match result {
        Err(_) if cancellation.is_cancelled() => Err(CANCELLED.into()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_count(connection: &Connection) -> Result<i64, String> {
        connection
            .query_row("SELECT count(*) FROM range(1000000000000) WHERE range % 7 = 3", [], |row| row.get(0))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn cancelling_interrupts_the_running_query_and_fails_later_ones() {
        let connection = Connection::open_in_memory().unwrap();
        let cancellation = Arc::new(Cancellation::default());
        let canceller = {
            let cancellation = cancellation.clone();
            std::thread::spawn(move || {
                while cancellation.running.lock().unwrap().is_empty() {
                    std::thread::yield_now();
                }
                // An interrupt before the query starts is dropped, so let it start.
                std::thread::sleep(std::time::Duration::from_millis(200));
                cancellation.cancel();
            })
        };
        let (first, second) = cancellation.scope(|| {
            (
                interruptible(&connection, || slow_count(&connection)),
                interruptible(&connection, || Ok(1)),
            )
        });
        canceller.join().unwrap();
        assert_eq!(first, Err(CANCELLED.to_owned()));
        assert_eq!(second, Err(CANCELLED.to_owned()));
        assert!(cancellation.running.lock().unwrap().is_empty());
        // The connection works afterwards, and outside the job nothing is cancelled.
        assert_eq!(check(), Ok(()));
        let one = interruptible(&connection, || {
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string())
        });
        assert_eq!(one, Ok(1));
    }

    #[test]
    fn check_fails_only_inside_a_cancelled_job() {
        let cancellation = Arc::new(Cancellation::default());
        assert_eq!(cancellation.scope(check), Ok(()));
        cancellation.cancel();
        assert_eq!(cancellation.scope(check), Err(CANCELLED.to_owned()));
        assert_eq!(check(), Ok(()));
    }
}
