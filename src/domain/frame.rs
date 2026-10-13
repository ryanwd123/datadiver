use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::infrastructure::duckdb_store::{
    CachedRow, ColumnSchema, DuckDbStore, MaterializedViewRows, RowIds, ViewQuery, ViewRowMap,
};

pub const PAGE_SIZE: usize = 250;
const MAX_CACHED_PAGES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ColumnKey(pub String);

#[derive(Clone)]
pub struct FrameSnapshot {
    pub(crate) schema: Arc<[ColumnSchema]>,
    pub store: Arc<Mutex<DuckDbStore>>,
    pub source_revision: u64,
    pub(crate) database_view_revision: Option<u64>,
    pub columns: Arc<[ColumnKey]>,
    pub dtypes: Arc<[String]>,
    pub dtype_labels: Option<Arc<[String]>>,
    pub(crate) persistent_identity:
        Option<crate::infrastructure::source_database::PersistentSourceIdentity>,
    /// Read once, so the UI never waits on the store lock that a long query on
    /// a worker holds.
    row_count: usize,
}

impl FrameSnapshot {
    pub fn new(store: DuckDbStore, source_revision: u64) -> Self {
        let (columns, dtypes) = schema_snapshot(store.schema());
        let persistent_identity = store.persistent_identity();
        let database_view_revision = store.database_view_revision();
        let row_count = store.row_count();
        Self {
            schema: store.schema().to_vec().into(),
            store: Arc::new(Mutex::new(store)),
            source_revision,
            database_view_revision,
            columns,
            dtypes,
            dtype_labels: None,
            persistent_identity,
            row_count,
        }
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }
}

#[derive(Clone)]
pub struct ResultSnapshot {
    pub store: Arc<Mutex<DuckDbStore>>,
    /// Display position maps through these to DuckDB's rowid.
    pub row_ids: RowIds,
    pub result_revision: u64,
    pub source_revision: u64,
    pub generation: u64,
    pub columns: Arc<[ColumnKey]>,
    pub dtypes: Arc<[String]>,
    pub column_indices: Arc<[usize]>,
    pub(crate) row_map: Option<Arc<ViewRowMap>>,
    page_cache: Arc<Mutex<PageCache>>,
    /// Rows in `store`, before search and filters.
    store_row_count: usize,
}

#[derive(Default)]
struct PageCache {
    pages: HashMap<usize, Arc<[CachedRow]>>,
    order: VecDeque<usize>,
}

impl ResultSnapshot {
    /// Reorder presentation without acquiring the store lock or rebuilding row pages.
    pub fn reordered(&self, order: &[String], result_revision: u64, generation: u64) -> Self {
        let positions = order
            .iter()
            .filter_map(|name| self.columns.iter().position(|column| column.0 == *name))
            .collect::<Vec<_>>();
        let mut result = self.clone();
        result.columns = positions
            .iter()
            .map(|index| self.columns[*index].clone())
            .collect::<Vec<_>>()
            .into();
        result.dtypes = positions
            .iter()
            .map(|index| self.dtypes[*index].clone())
            .collect::<Vec<_>>()
            .into();
        result.column_indices = positions
            .iter()
            .map(|index| self.column_indices[*index])
            .collect::<Vec<_>>()
            .into();
        result.result_revision = result_revision;
        result.generation = generation;
        result
    }
    /// A query-free placeholder for a viewer whose first result is prepared on a worker.
    pub fn pending(source: &FrameSnapshot) -> Self {
        Self {
            store: source.store.clone(),
            row_ids: RowIds::Listed(Arc::from([])),
            result_revision: 0,
            source_revision: source.source_revision,
            generation: 0,
            columns: source.columns.clone(),
            dtypes: source.dtypes.clone(),
            column_indices: (0..source.columns.len()).collect::<Vec<_>>().into(),
            row_map: None,
            page_cache: Arc::new(Mutex::new(PageCache::default())),
            store_row_count: source.row_count(),
        }
    }

    /// Pages contain all source columns, so changing display order can share them.
    pub fn reproject(
        &self,
        source: &FrameSnapshot,
        columns: &[ColumnSchema],
        generation: u64,
    ) -> Self {
        let mut result = self.clone();
        let (names, dtypes) = schema_snapshot(columns);
        result.columns = names;
        result.dtypes = dtypes;
        result.column_indices = columns
            .iter()
            .map(|column| {
                source
                    .columns
                    .iter()
                    .position(|key| key.0 == column.name)
                    .expect("projected column belongs to the source")
            })
            .collect::<Vec<_>>()
            .into();
        result.generation = generation;
        result
    }

    pub fn from_store(
        store: DuckDbStore,
        source_revision: u64,
        result_revision: u64,
        generation: u64,
    ) -> Self {
        let source = FrameSnapshot::new(store, source_revision);
        Self::from_source(&source, result_revision, generation)
    }

    pub fn from_source(source: &FrameSnapshot, result_revision: u64, generation: u64) -> Self {
        let rows = source.store.lock().ok().and_then(|store| {
            store
                .query_view_rows(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: Vec::new(),
                    filters: Vec::new(),
                    sort: Vec::new(),
                })
                .ok()
        });
        let (row_ids, row_map) = rows
            .map(|rows| (rows.row_ids, rows.row_map))
            .unwrap_or_else(|| {
                let row_ids = source
                    .store
                    .lock()
                    .map(|store| store.initial_row_ids().unwrap_or_default())
                    .unwrap_or_default();
                (RowIds::Listed(row_ids.into()), None)
            });
        Self {
            store: source.store.clone(),
            row_ids,
            result_revision,
            source_revision: source.source_revision,
            generation,
            columns: source.columns.clone(),
            dtypes: source.dtypes.clone(),
            column_indices: (0..source.columns.len()).collect::<Vec<_>>().into(),
            row_map,
            page_cache: Arc::new(Mutex::new(PageCache::default())),
            store_row_count: source.row_count(),
        }
    }

    pub fn from_view(
        source: &FrameSnapshot,
        rows: MaterializedViewRows,
        columns: Vec<ColumnSchema>,
        result_revision: u64,
        generation: u64,
    ) -> Self {
        let store_schema = source
            .store
            .lock()
            .map(|store| store.schema().to_vec())
            .unwrap_or_default();
        let column_indices = columns
            .iter()
            .filter_map(|column| {
                store_schema
                    .iter()
                    .position(|source| source.name == column.name)
            })
            .collect::<Vec<_>>();
        let (columns, dtypes) = schema_snapshot(&columns);
        Self {
            store: source.store.clone(),
            row_ids: rows.row_ids,
            result_revision,
            source_revision: source.source_revision,
            generation,
            columns,
            dtypes,
            column_indices: column_indices.into(),
            row_map: rows.row_map,
            page_cache: Arc::new(Mutex::new(PageCache::default())),
            store_row_count: source.row_count(),
        }
    }

    pub fn row_count(&self) -> usize {
        self.row_ids.len()
    }

    pub fn store_row_count(&self) -> usize {
        self.store_row_count
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn page_is_cached(&self, page_index: usize) -> bool {
        self.page_cache
            .lock()
            .map(|cache| cache.pages.contains_key(&page_index))
            .unwrap_or(false)
    }

    pub fn prefetch_page(&self, page_index: usize) -> Result<(), String> {
        if self.page_is_cached(page_index) {
            return Ok(());
        }
        let start = page_index * PAGE_SIZE;
        let rows = self
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?
            .fetch_view_page(self.row_map.as_deref(), &self.row_ids, start, PAGE_SIZE)?;
        let rows: Arc<[CachedRow]> = rows.into();
        let mut cache = self
            .page_cache
            .lock()
            .map_err(|_| "row cache lock poisoned".to_owned())?;
        cache.pages.insert(page_index, rows);
        cache.order.retain(|index| *index != page_index);
        cache.order.push_back(page_index);
        while cache.order.len() > MAX_CACHED_PAGES {
            if let Some(evicted) = cache.order.pop_front() {
                cache.pages.remove(&evicted);
            }
        }
        Ok(())
    }

    pub fn cached_rows(&self, start: usize, count: usize) -> Option<Vec<CachedRow>> {
        let end = start.saturating_add(count).min(self.row_ids.len());
        if start >= end {
            return Some(Vec::new());
        }
        let cache = self.page_cache.lock().ok()?;
        let first_page = start / PAGE_SIZE;
        let last_page = (end - 1) / PAGE_SIZE;
        let mut output = Vec::with_capacity(end - start);
        for page_index in first_page..=last_page {
            let page_start = page_index * PAGE_SIZE;
            let page = cache.pages.get(&page_index)?;
            output.extend(page_rows(page, page_start, start, end).iter().cloned());
        }
        Some(output)
    }

    /// Writes every displayed row and column, in display order, to `path`.
    /// Returns the number of rows written.
    pub fn export_to_file(
        &self,
        path: &std::path::Path,
        format: crate::domain::export::FileFormat,
    ) -> Result<usize, String> {
        self.store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?
            .export_view(
                self.row_map.as_deref(),
                &self.row_ids,
                &self.column_indices,
                path,
                format,
            )
    }

    pub fn fetch_rows(&self, start: usize, count: usize) -> Result<Vec<CachedRow>, String> {
        if let Some(rows) = self.cached_rows(start, count) {
            return Ok(rows);
        }
        // Copies and exports read their whole range in one query, leaving the page
        // cache to the grid so the pages on screen are not evicted.
        self.store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?
            .fetch_view_page(self.row_map.as_deref(), &self.row_ids, start, count)
    }
}

/// Rows of one page within display positions `start..end`. A page can hold fewer
/// rows than requested when rows vanished from a lazily evaluated database view.
fn page_rows(page: &[CachedRow], page_start: usize, start: usize, end: usize) -> &[CachedRow] {
    let local_start = start.saturating_sub(page_start);
    let local_end = (end - page_start).min(page.len());
    page.get(local_start..local_end).unwrap_or_default()
}

fn schema_snapshot(schema: &[ColumnSchema]) -> (Arc<[ColumnKey]>, Arc<[String]>) {
    let columns = schema
        .iter()
        .map(|column| ColumnKey(column.name.clone()))
        .collect::<Vec<_>>()
        .into();
    let dtypes = schema
        .iter()
        .map(|column| column.duckdb_type.clone())
        .collect::<Vec<_>>()
        .into();
    (columns, dtypes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;

    fn source() -> DuckDbStore {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('first'), ('second'), ('third')) t(name)",
            )
            .unwrap();
        DuckDbStore::from_connection(connection).unwrap()
    }

    /// Rendering reads these counts while a worker may hold the store for a
    /// long query.
    #[test]
    fn row_counts_do_not_wait_for_the_store() {
        let frame = FrameSnapshot::new(source(), 1);
        let result = ResultSnapshot::pending(&frame);
        let _busy = frame.store.lock().unwrap();
        assert_eq!(frame.row_count(), 3);
        assert_eq!(result.store_row_count(), 3);
    }

    #[test]
    fn pages_missing_vanished_rows_do_not_panic() {
        let frame = FrameSnapshot::new(source(), 7);
        let schema = frame.store.lock().unwrap().schema().to_vec();
        let rows = frame
            .store
            .lock()
            .unwrap()
            .materialize_view_rows_from_ids(&[0, 1, 2, 99, 98])
            .unwrap();
        let result = ResultSnapshot::from_view(&frame, rows, schema, 1, 1);
        result.prefetch_page(0).unwrap();
        assert_eq!(result.cached_rows(4, 1).unwrap(), Vec::new());
        assert_eq!(result.fetch_rows(3, 2).unwrap(), Vec::new());
        assert_eq!(result.fetch_rows(0, 5).unwrap().len(), 3);
    }

    #[test]
    fn gui_positions_resolve_through_ordered_row_ids_and_cache_a_page() {
        let frame = FrameSnapshot::new(source(), 7);
        let schema = frame.store.lock().unwrap().schema().to_vec();
        let rows = frame
            .store
            .lock()
            .unwrap()
            .materialize_view_rows_from_ids(&[2, 0])
            .unwrap();
        let result = ResultSnapshot::from_view(&frame, rows, schema, 1, 1);
        assert!(result.cached_rows(0, 1).is_none());
        result.prefetch_page(0).unwrap();
        let display_row_zero = result.cached_rows(0, 1).unwrap();
        assert_eq!(display_row_zero[0].rowid, 2);
        assert_eq!(display_row_zero[0].values[0].as_deref(), Some("third"));
        assert_eq!(result.fetch_rows(0, 1).unwrap(), display_row_zero);
        assert_eq!(result.page_cache.lock().unwrap().pages.len(), 1);
    }

    #[test]
    fn presentation_reuses_pages_and_row_map_with_correct_exports_and_statistics() {
        use crate::domain::documents::SortKey;
        use crate::domain::selection::{Cell, Selection};
        let store = DuckDbStore::from_rows(
            &[
                ("name".into(), "VARCHAR".into()),
                ("n".into(), "BIGINT".into()),
            ],
            [
                vec![
                    duckdb::types::Value::Text("a".into()),
                    duckdb::types::Value::BigInt(10),
                ],
                vec![
                    duckdb::types::Value::Text("b".into()),
                    duckdb::types::Value::BigInt(20),
                ],
            ],
        )
        .unwrap();
        let frame = FrameSnapshot::new(store, 7);
        let (rows, mut schema) = {
            let store = frame.store.lock().unwrap();
            (
                store
                    .query_view_rows(&ViewQuery {
                        hidden: Vec::new(),
                        value_filters: Vec::new(),
                        tokens: vec![],
                        filters: vec![],
                        sort: vec![SortKey {
                            column: "n".into(),
                            descending: true,
                        }],
                    })
                    .unwrap(),
                store.schema().to_vec(),
            )
        };
        let original = ResultSnapshot::from_view(&frame, rows, schema.clone(), 3, 4);
        original.prefetch_page(0).unwrap();
        schema.reverse();
        let projected = original.reproject(&frame, &schema, 5);
        assert!(original.row_ids.same(&projected.row_ids));
        assert!(Arc::ptr_eq(&original.page_cache, &projected.page_cache));
        assert!(Arc::ptr_eq(
            original.row_map.as_ref().unwrap(),
            projected.row_map.as_ref().unwrap()
        ));
        assert_eq!(&*projected.column_indices, &[1, 0]);
        assert_eq!(projected.generation, 5);
        drop(original);
        let selection = Selection {
            anchor: Cell { row: 0, column: 0 },
            active: Cell { row: 1, column: 1 },
        };
        assert_eq!(
            crate::domain::export::export(
                &projected,
                selection,
                crate::domain::export::CopyFormat::RawTsv
            )
            .unwrap(),
            "20\tb\n10\ta\n"
        );
        let numeric = Selection {
            anchor: Cell { row: 0, column: 0 },
            active: Cell { row: 1, column: 0 },
        };
        assert_eq!(
            crate::domain::statistics::calculate(&projected, numeric)
                .unwrap()
                .numeric_sum
                .as_deref(),
            Some("30")
        );
    }
}
