use crate::domain::documents::{SlicerDefinition, ValueFilter};
use crate::domain::frame::FrameSnapshot;
use crate::domain::slicers::{self, SlicerCard, SlicerValue};
use crate::infrastructure::duckdb_store::{ColumnFilter, DuckDbStore};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct SlicerCatalog {
    pub columns: Vec<(String, String, Vec<SlicerValue>)>,
    /// The columns discovery examined, or `None` for every column. Value cards
    /// need only their own columns; listing new value cards needs them all.
    pub examined: Option<Vec<String>>,
}

/// A request to discover every column's values for the Add Slicer picker.
/// Discovery scans the whole source, so closing the picker or choosing a card
/// that needs no values cancels it, interrupting the query that is running.
#[derive(Default)]
pub struct SlicerDiscovery {
    cancelled: AtomicBool,
    running: Mutex<Option<Arc<duckdb::InterruptHandle>>>,
}

impl SlicerDiscovery {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // Holding the lock keeps discovery from finishing between the check and
        // the interrupt, which would stop an unrelated query.
        if let Ok(running) = self.running.lock()
            && let Some(handle) = running.as_ref()
        {
            handle.interrupt();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn run<T>(
        &self,
        store: &DuckDbStore,
        discover: impl FnOnce() -> Result<T, String>,
    ) -> Option<Result<T, String>> {
        {
            let mut running = self.running.lock().ok()?;
            if self.is_cancelled() {
                return None;
            }
            *running = Some(store.interrupt_handle());
        }
        let result = discover();
        if let Ok(mut running) = self.running.lock() {
            *running = None;
        }
        (!self.is_cancelled()).then_some(result)
    }
}

#[derive(Clone)]
pub struct SlicerStage {
    pub source: FrameSnapshot,
    pub catalog: Arc<SlicerCatalog>,
    pub definitions: Vec<SlicerDefinition>,
    /// The view's `=` / `-` filters, which also narrow slicer availability.
    pub value_filters: Vec<ValueFilter>,
    pub cards: Vec<SlicerCard>,
}

impl SlicerStage {
    pub fn matches_source(&self, source: &FrameSnapshot) -> bool {
        self.source.source_revision == source.source_revision
            && Arc::ptr_eq(&self.source.store, &source.store)
    }

    /// Discovers every column while `discovery` asks for it and is not
    /// cancelled; otherwise only the value cards' columns, which condition and
    /// SQL cards do not need.
    pub fn prepare(
        source: &FrameSnapshot,
        definitions: &[SlicerDefinition],
        value_filters: &[ValueFilter],
        cached: Option<&Self>,
        discovery: Option<&SlicerDiscovery>,
    ) -> Result<Self, String> {
        let discover_all = discovery.is_some_and(|discovery| !discovery.is_cancelled());
        let value_columns = definitions
            .iter()
            .filter(|definition| !definition.sql && definition.condition.is_none())
            .map(|definition| definition.column.clone())
            .collect::<Vec<_>>();
        let cached = cached.filter(|cached| cached.matches_source(source));
        let cached_catalog = cached
            .map(|cached| &cached.catalog)
            .filter(|catalog| match &catalog.examined {
                None => true,
                Some(examined) => {
                    !discover_all && value_columns.iter().all(|column| examined.contains(column))
                }
            });
        if let Some(cached) = cached
            && cached_catalog.is_some()
            && cached.definitions == definitions
            && cached.value_filters == value_filters
        {
            return Ok(cached.clone());
        }
        let store = source
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned")?;
        let all = match discovery.filter(|_| discover_all && cached_catalog.is_none()) {
            Some(discovery) => discovery
                .run(&store, || {
                    SlicerCatalog::discover(&store, None, &|| discovery.is_cancelled())
                })
                .transpose()?,
            None => None,
        };
        let catalog = match (all, cached_catalog) {
            (Some(all), _) => Arc::new(all),
            (None, Some(cached)) => cached.clone(),
            (None, None) => Arc::new(SlicerCatalog::discover(&store, Some(&value_columns), &|| false)?),
        };
        let cards = catalog.cards(&store, definitions, value_filters);
        Ok(Self {
            source: source.clone(),
            catalog,
            definitions: definitions.to_vec(),
            value_filters: value_filters.to_vec(),
            cards,
        })
    }

    /// Fails on a value card error. A condition card's error is shown on the card
    /// alone; the card is left out of filtering instead, see [`Self::effective`].
    pub fn validate(&self) -> Result<(), String> {
        if let Some(card) = self
            .cards
            .iter()
            .find(|card| card.error.is_some() && card.condition.is_none())
        {
            return Err(format!(
                "Slicer {}: {}",
                card.column,
                card.error.as_deref().unwrap_or_default()
            ));
        }
        Ok(())
    }

    /// The definitions that filter rows: condition cards with an error, such as a
    /// typo in `sql:`, are blanked so the rest of the view keeps working.
    pub fn effective(&self) -> Vec<SlicerDefinition> {
        usable(&self.definitions, &self.cards)
    }
}

fn usable(definitions: &[SlicerDefinition], cards: &[SlicerCard]) -> Vec<SlicerDefinition> {
    definitions
        .iter()
        .map(|definition| {
            let mut definition = definition.clone();
            let broken = cards
                .iter()
                .any(|card| card.column == definition.column && card.error.is_some());
            if broken && let Some(text) = &mut definition.condition {
                text.clear();
            }
            definition
        })
        .collect()
}

impl SlicerCatalog {
    /// Values for the columns with few enough of them, among `only` or every
    /// column. `stopped` is checked between queries.
    pub fn discover(
        store: &DuckDbStore,
        only: Option<&[String]>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let candidates = store
            .schema()
            .iter()
            .filter(|column| slicers::supported(&column.duckdb_type))
            .filter(|column| only.is_none_or(|only| only.contains(&column.name)))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(Self {
                columns: Vec::new(),
                examined: only.map(<[String]>::to_vec),
            });
        }
        let names = candidates
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let estimates = store.approximate_unique_counts(&names)?;
        let mut columns = Vec::new();
        for (column, (_, estimate)) in candidates.into_iter().zip(estimates) {
            // Exact discovery hashes every distinct value, which is costly for ids and
            // free text. Estimates overshoot by at most ~1.3x in practice, so this
            // margin only skips columns that are certainly over the limit.
            if estimate > slicers::MAX_UNIQUE_VALUES * 4 {
                continue;
            }
            if stopped() {
                return Err("slicer discovery was cancelled".into());
            }
            let values = store.slicer_values(&column.name, &[], &[])?;
            if values.len() <= slicers::MAX_UNIQUE_VALUES {
                columns.push((column.name.clone(), column.duckdb_type.clone(), values));
            }
        }
        Ok(Self {
            columns,
            examined: only.map(<[String]>::to_vec),
        })
    }

    /// Cards for `definitions`. A value is available when some source row has it
    /// and matches the other cards' selections and the view's value filters.
    pub fn cards(
        &self,
        store: &DuckDbStore,
        definitions: &[SlicerDefinition],
        value_filters: &[ValueFilter],
    ) -> Vec<SlicerCard> {
        // Condition cards come first so a broken one does not filter value cards.
        let conditions = definitions
            .iter()
            .filter_map(|definition| {
                let text = definition.condition.as_ref()?;
                Some(condition_card(store, definition, text))
            })
            .collect::<Vec<_>>();
        let usable = usable(definitions, &conditions);
        definitions
            .iter()
            .map(|definition| {
                if let Some(card) = conditions
                    .iter()
                    .find(|card| card.column == definition.column)
                {
                    return card.clone();
                }
                let discovered = self
                    .columns
                    .iter()
                    .find(|(name, _, _)| name == &definition.column);
                let mut error = match discovered {
                    Some((_, dtype, _))
                        if definition
                            .selected
                            .iter()
                            .any(|key| !slicers::compatible(key, dtype)) =>
                    {
                        Some("Column type is incompatible with saved selections".to_owned())
                    }
                    Some(_) => None,
                    None if store
                        .schema()
                        .iter()
                        .any(|column| column.name == definition.column) =>
                    {
                        Some(format!(
                            "Column is unsupported or has more than {} unique values",
                            slicers::MAX_UNIQUE_VALUES
                        ))
                    }
                    None => Some("Source column is missing".to_owned()),
                };
                let mut values = discovered
                    .map(|(_, _, values)| values.clone())
                    .unwrap_or_default();
                let filters = filters(
                    &usable
                        .iter()
                        .filter(|other| other.column != definition.column)
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                if error.is_none() {
                    match store.slicer_values(&definition.column, &filters, value_filters) {
                        Ok(available) => {
                            for value in &mut values {
                                value.available =
                                    available.iter().any(|other| other.key == value.key);
                            }
                        }
                        Err(message) => error = Some(message),
                    }
                }
                if error.is_some() {
                    for value in &mut values {
                        value.available = false;
                    }
                }
                for key in &definition.selected {
                    if !values.iter().any(|value| &value.key == key) {
                        values.push(SlicerValue {
                            key: key.clone(),
                            label: slicers::label(key),
                            available: false,
                        });
                    }
                }
                SlicerCard {
                    sql: false,
                    condition: None,
                    bounds: None,
                    column: definition.column.clone(),
                    values,
                    selected: definition.selected.clone(),
                    error,
                }
            })
            .collect()
    }
}

fn condition_card(store: &DuckDbStore, definition: &SlicerDefinition, text: &str) -> SlicerCard {
    let (bounds, error) = if definition.sql {
        (None, store.validate_sql(text).err())
    } else {
        match store.column_bounds(&definition.column) {
            Ok(bounds) => (
                bounds,
                store.validate_condition(&definition.column, text).err(),
            ),
            Err(error) => (None, Some(error)),
        }
    };
    SlicerCard {
        sql: definition.sql,
        column: definition.column.clone(),
        values: Vec::new(),
        selected: Vec::new(),
        error,
        condition: Some(text.to_owned()),
        bounds,
    }
}

pub fn filters(definitions: &[SlicerDefinition]) -> Vec<ColumnFilter> {
    definitions
        .iter()
        .map(|card| ColumnFilter {
            sql: card.sql,
            condition: card.condition.clone(),
            column: card.column.clone(),
            values: card.selected.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::{Aggregation, AggregationOp, QueryConfig, ScalarKey};
    use crate::infrastructure::duckdb_store::ViewQuery;
    use duckdb::types::Value;

    /// Prepares as the open Add Slicer picker does, discovering every column.
    fn prepare(
        source: &FrameSnapshot,
        definitions: &[SlicerDefinition],
        value_filters: &[ValueFilter],
        cached: Option<&SlicerStage>,
    ) -> Result<SlicerStage, String> {
        let discovery = SlicerDiscovery::default();
        SlicerStage::prepare(source, definitions, value_filters, cached, Some(&discovery))
    }

    fn sql_frame(sql: &str) -> FrameSnapshot {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!("CREATE TABLE qp_data AS ({sql})"))
            .unwrap();
        FrameSnapshot::new(DuckDbStore::from_connection(connection).unwrap(), 1)
    }

    fn card(column: &str, selected: &[&str]) -> SlicerDefinition {
        SlicerDefinition {
            sql: false,
            condition: None,
            column: column.into(),
            selected: selected
                .iter()
                .map(|value| ScalarKey::String((*value).into()))
                .collect(),
        }
    }

    fn source() -> FrameSnapshot {
        FrameSnapshot::new(
            DuckDbStore::from_rows(
                &[
                    ("region".into(), "VARCHAR".into()),
                    ("color".into(), "VARCHAR".into()),
                    ("sales".into(), "BIGINT".into()),
                ],
                [
                    vec![
                        Value::Text("East".into()),
                        Value::Text("Red".into()),
                        Value::BigInt(10),
                    ],
                    vec![
                        Value::Text("East".into()),
                        Value::Text("Blue".into()),
                        Value::BigInt(20),
                    ],
                    vec![
                        Value::Text("West".into()),
                        Value::Text("Red".into()),
                        Value::BigInt(30),
                    ],
                ],
            )
            .unwrap(),
            1,
        )
    }

    #[test]
    fn slicer_discovery_cutoff_counts_null_and_excludes_nested_values() {
        for count in [999, 1_000, 1_001] {
            let source = sql_frame(&format!(
                "SELECT CASE WHEN range % {count} = {count} - 1 THEN NULL \
                 ELSE range % {count} END AS scalar, [range] AS nested FROM range({})",
                count * 2
            ));
            let catalog = SlicerCatalog::discover(&source.store.lock().unwrap(), None, &|| false).unwrap();
            if count <= 1_000 {
                assert_eq!(catalog.columns.len(), 1);
                assert_eq!(catalog.columns[0].2.len(), count);
                assert_eq!(catalog.columns[0].2.last().unwrap().key, ScalarKey::Null);
            } else {
                assert!(catalog.columns.is_empty());
            }
        }
        let source = sql_frame("SELECT range AS id, range % 3 AS small FROM range(100000)");
        let catalog = SlicerCatalog::discover(&source.store.lock().unwrap(), None, &|| false).unwrap();
        assert_eq!(
            catalog
                .columns
                .iter()
                .map(|(name, _, values)| (name.as_str(), values.len()))
                .collect::<Vec<_>>(),
            [("small", 3)]
        );
        let source = sql_frame("SELECT range AS n FROM range(10000)");
        assert_eq!(
            source
                .store
                .lock()
                .unwrap()
                .slicer_values("n", &[], &[])
                .unwrap()
                .len(),
            1_001
        );
    }

    #[test]
    fn slicer_at_limit_keeps_all_values_and_filters_the_last_value() {
        let source = sql_frame("SELECT range % 1000 AS n FROM range(2000)");
        let definitions = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "n".into(),
            selected: vec![ScalarKey::Signed("999".into())],
        }];
        let stage = prepare(&source, &definitions, &[], None).unwrap();
        stage.validate().unwrap();
        assert_eq!(stage.cards[0].values.len(), 1_000);
        assert_eq!(
            stage.cards[0].values.last().unwrap().key,
            ScalarKey::Signed("999".into())
        );
        assert!(stage.cards[0].values.iter().all(|value| value.available));
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                sort: vec![],
                filters: filters(&definitions),
            })
            .unwrap();
        assert_eq!(rows, [999, 1_999]);
    }

    #[test]
    fn slicer_null_empty_and_literal_labels_have_separate_identity_and_filters() {
        let source =
            sql_frame("SELECT * FROM (VALUES (NULL), (''), ('(Blank)'), ('(Empty)')) t(name)");
        let stage = prepare(&source, &[], &[], None).unwrap();
        let values = &stage.catalog.columns[0].2;
        assert_eq!(values.len(), 4);
        assert_eq!(
            values
                .iter()
                .filter(|value| value.label == "(Blank)")
                .count(),
            2
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| value.label == "(Empty)")
                .count(),
            2
        );
        for value in values {
            let rows = source
                .store
                .lock()
                .unwrap()
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![],
                    sort: vec![],
                    filters: filters(&[SlicerDefinition {
                        sql: false,
                        condition: None,
                        column: "name".into(),
                        selected: vec![value.key.clone()],
                    }]),
                })
                .unwrap();
            assert_eq!(rows.len(), 1);
        }
    }

    #[test]
    fn slicer_or_and_availability_excludes_own_selection_and_search_reuses_stage() {
        let source = source();
        let definitions = [card("region", &["East", "West"]), card("color", &["Blue"])];
        let stage = prepare(&source, &definitions, &[], None).unwrap();
        stage.validate().unwrap();
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                sort: vec![],
                filters: filters(&definitions),
            })
            .unwrap();
        assert_eq!(rows, [1]);
        let regions = &stage.cards[0];
        assert!(
            regions
                .values
                .iter()
                .find(|value| value.label == "East")
                .unwrap()
                .available
        );
        assert!(
            !regions
                .values
                .iter()
                .find(|value| value.label == "West")
                .unwrap()
                .available
        );
        assert!(regions.selectable(1)); // Selected unavailable values remain removable.
        assert!(stage.cards[1].values.iter().all(|value| value.available));
        let cached = prepare(&source, &definitions, &[], Some(&stage)).unwrap();
        assert!(Arc::ptr_eq(&cached.catalog, &stage.catalog));
        assert_eq!(cached.cards, stage.cards);
        let changed = prepare(
            &source,
            &[card("region", &["West"]), card("color", &[])],
            &[],
            Some(&stage),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&changed.catalog, &stage.catalog));
        assert!(
            !changed.cards[1]
                .values
                .iter()
                .find(|value| value.label == "Blue")
                .unwrap()
                .available
        );
    }

    #[test]
    fn condition_cards_filter_rows_narrow_value_cards_and_report_errors() {
        let source = source();
        let condition = |text: &str| SlicerDefinition {
            sql: false,
            column: "sales".into(),
            selected: Vec::new(),
            condition: Some(text.into()),
        };
        // Sales 20..30 keeps East/Blue and West/Red, so Red stays available
        // through West and Blue through East.
        let definitions = [card("region", &["East"]), condition("20..30")];
        let stage = prepare(&source, &definitions, &[], None).unwrap();
        stage.validate().unwrap();
        let sales = &stage.cards[1];
        assert_eq!(sales.condition.as_deref(), Some("20..30"));
        assert!(sales.values.is_empty());
        assert_eq!(sales.bounds, Some(("10".into(), "30".into())));
        let regions = &stage.cards[0];
        assert!(regions.values.iter().all(|value| value.available));
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                filters: filters(&definitions),
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(rows, [1]);
        // >25 leaves only West, so East greys out.
        let narrowed =
            prepare(&source, &[card("region", &[]), condition(">25")], &[], None)
                .unwrap();
        let east = narrowed.cards[0]
            .values
            .iter()
            .find(|value| value.label == "East")
            .unwrap();
        assert!(!east.available);

        // A broken condition reports on its card and stops filtering, so the
        // value card beside it stays fully available.
        let broken = prepare(
            &source,
            &[card("region", &[]), condition(">=abc")],
            &[],
            None,
        )
        .unwrap();
        assert!(broken.cards[1].error.as_deref().unwrap().contains("abc"));
        assert!(broken.cards[0].values.iter().all(|value| value.available));
        broken.validate().unwrap();
        assert_eq!(broken.effective()[1].condition.as_deref(), Some(""));
    }

    #[test]
    fn sql_cards_filter_any_column_and_isolate_errors() {
        let source = source();
        let sql = |text: &str| SlicerDefinition {
            condition: Some(text.into()),
            ..SlicerDefinition::sql_filter()
        };
        // Sales over 25 leaves only the West row, so East greys out.
        let stage = prepare(
            &source,
            &[card("region", &[]), sql("sales > 25")],
            &[],
            None,
        )
        .unwrap();
        stage.validate().unwrap();
        let card_sql = &stage.cards[1];
        assert!(card_sql.sql && card_sql.error.is_none() && card_sql.bounds.is_none());
        let available = |stage: &SlicerStage, label: &str| {
            stage.cards[0]
                .values
                .iter()
                .find(|value| value.label == label)
                .unwrap()
                .available
        };
        assert!(available(&stage, "West"));
        assert!(!available(&stage, "East"));
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                filters: filters(&stage.effective()),
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(rows, [2]);

        let broken =
            prepare(&source, &[card("region", &[]), sql("nope > 1")], &[], None)
                .unwrap();
        assert!(broken.cards[1].error.as_deref().unwrap().contains("nope"));
        assert!(available(&broken, "East"));
        broken.validate().unwrap();
        assert_eq!(broken.effective()[1].condition.as_deref(), Some(""));
    }

    #[test]
    fn value_filters_narrow_availability_and_refresh_the_cached_stage() {
        let source = source();
        let definitions = [card("color", &[])];
        let stage = prepare(&source, &definitions, &[], None).unwrap();
        assert!(stage.cards[0].values.iter().all(|value| value.available));
        // Excluding East leaves only the West row, which is Red.
        let exclude_east = [ValueFilter {
            target: "region".into(),
            value: Some("East".into()),
            exclude: true,
        }];
        let filtered =
            prepare(&source, &definitions, &exclude_east, Some(&stage)).unwrap();
        assert!(Arc::ptr_eq(&filtered.catalog, &stage.catalog));
        let available = |stage: &SlicerStage, label: &str| {
            stage.cards[0]
                .values
                .iter()
                .find(|value| value.label == label)
                .unwrap()
                .available
        };
        assert!(available(&filtered, "Red"));
        assert!(!available(&filtered, "Blue"));
        let cleared = prepare(&source, &definitions, &[], Some(&filtered)).unwrap();
        assert!(available(&cleared, "Blue"));
    }

    #[test]
    fn slicer_reload_retains_disappeared_values_and_rejects_invalid_columns_even_unfiltered() {
        let original = source();
        let definitions = [card("region", &["East"])];
        let stage = prepare(&original, &definitions, &[], None).unwrap();
        let refreshed = sql_frame("SELECT 'West' AS region");
        let refreshed = prepare(&refreshed, &definitions, &[], Some(&stage)).unwrap();
        refreshed.validate().unwrap();
        assert!(!Arc::ptr_eq(&refreshed.catalog, &stage.catalog));
        let missing = refreshed.cards[0]
            .values
            .iter()
            .position(|value| value.label == "East")
            .unwrap();
        assert!(!refreshed.cards[0].values[missing].available);
        assert_eq!(
            slicers::click_selection(&refreshed.cards[0], missing, false),
            Vec::<ScalarKey>::new()
        );
        for source in [
            sql_frame("SELECT 1 AS other"),
            sql_frame("SELECT range AS region FROM range(1001)"),
            sql_frame("SELECT [1] AS region"),
        ] {
            let stage = prepare(&source, &[card("region", &[])], &[], None).unwrap();
            assert!(stage.validate().is_err());
        }
        let incompatible = sql_frame("SELECT 1 AS region");
        assert!(
            prepare(&incompatible, &definitions, &[], None)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn slicer_source_column_filters_pivot_measures_without_being_in_result() {
        let source = source();
        let pivot = QueryConfig {
            aggregations: vec![Aggregation {
                sql: None,
                source_column: "sales".into(),
                operation: AggregationOp::Sum,
                name: Some("total".into()),
                conditions: vec![],
                percentile: None,
                interpolation: None,
                ddof: None,
            }],
            ..Default::default()
        };
        let result = crate::domain::pivot::execute(
            &source.store.lock().unwrap(),
            &pivot,
            &[card("region", &["East"])],
        )
        .unwrap();
        assert_eq!(result.schema().len(), 1);
        assert_eq!(
            result
                .fetch_row_ids(&result.initial_row_ids().unwrap())
                .unwrap()[0]
                .values[0]
                .as_ref()
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            30.0
        );
    }

    #[test]
    fn slicer_decimal_temporal_and_large_integer_values_filter_exactly() {
        for sql in [
            "SELECT * FROM (VALUES (TRUE), (FALSE)) t(v)",
            "SELECT * FROM (VALUES ('z'::ENUM('z', 'a')), ('a'::ENUM('z', 'a'))) t(v)",
            "SELECT * FROM (VALUES (1.25::FLOAT), (2.5::FLOAT)) t(v)",
            "SELECT * FROM (VALUES (9007199254740993::BIGINT), (9007199254740994::BIGINT)) t(v)",
            "SELECT * FROM (VALUES ('0.123456789012345678901234567890'::DECIMAL(38,30)), ('0.123456789012345678901234567891'::DECIMAL(38,30))) t(v)",
            "SELECT * FROM (VALUES (TIME '12:34:56.123456'), (TIME '12:34:56.123457')) t(v)",
            "SELECT * FROM (VALUES (TIMESTAMP_NS '2025-01-01 00:00:00.000000001'), (TIMESTAMP_NS '2025-01-01 00:00:00.000000002')) t(v)",
            "SELECT * FROM (VALUES (TIMESTAMPTZ '2025-01-01 00:00:00+03'), (TIMESTAMPTZ '2025-01-01 00:00:00+04')) t(v)",
            "SELECT * FROM (VALUES (INTERVAL '1 month 2 days 3000001 microseconds'), (INTERVAL '1 month 2 days 3000002 microseconds')) t(v)",
            "SELECT * FROM (VALUES (DATE '2025-01-01'), (DATE '2025-01-02')) t(v)",
            "SELECT * FROM (VALUES ('18446744073709551616'::UHUGEINT), ('18446744073709551617'::UHUGEINT)) t(v)",
            "SELECT * FROM (VALUES ('NaN'::DOUBLE), ('Infinity'::DOUBLE), (1.25::DOUBLE)) t(v)",
        ] {
            let source = sql_frame(sql);
            let stage = prepare(&source, &[], &[], None).unwrap();
            assert_eq!(stage.catalog.columns.len(), 1, "{sql}");
            for value in &stage.catalog.columns[0].2 {
                assert_eq!(
                    serde_json::from_str::<ScalarKey>(&serde_json::to_string(&value.key).unwrap())
                        .unwrap(),
                    value.key
                );
                let definition = SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "v".into(),
                    selected: vec![value.key.clone()],
                };
                prepare(
                    &source,
                    std::slice::from_ref(&definition),
                    &[],
                    Some(&stage),
                )
                .unwrap()
                .validate()
                .unwrap();
                let rows = source
                    .store
                    .lock()
                    .unwrap()
                    .query_row_ids(&ViewQuery {
                        hidden: Vec::new(),
                        value_filters: Vec::new(),
                        tokens: vec![],
                        sort: vec![],
                        filters: filters(&[definition]),
                    })
                    .unwrap();
                assert_eq!(rows.len(), 1, "{sql}: {}", value.label);
            }
        }
    }

    #[test]
    fn slicer_decimal_reload_does_not_round_a_saved_selection_into_another_value() {
        let source = sql_frame("SELECT '0.123456789012345678901234567890'::DECIMAL(38,30) AS v");
        let original = prepare(&source, &[], &[], None).unwrap();
        let definitions = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "v".into(),
            selected: vec![original.catalog.columns[0].2[0].key.clone()],
        }];
        let refreshed = sql_frame("SELECT '0.123456789012345679'::DECIMAL(38,18) AS v");
        let stage = prepare(&refreshed, &definitions, &[], Some(&original)).unwrap();
        assert!(!stage.cards[0].values.last().unwrap().available);
        let rows = refreshed
            .store
            .lock()
            .unwrap()
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                sort: vec![],
                filters: filters(&definitions),
            })
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn discovery_covers_only_value_cards_unless_the_picker_asks() {
        let source = source();
        let condition = SlicerDefinition {
            sql: false,
            column: "sales".into(),
            selected: Vec::new(),
            condition: Some(">15".into()),
        };
        let lazy = |definitions: &[SlicerDefinition], cached: Option<&SlicerStage>| {
            SlicerStage::prepare(&source, definitions, &[], cached, None).unwrap()
        };
        // Condition and SQL cards discover nothing.
        let stage = lazy(&[condition.clone(), SlicerDefinition::sql_filter()], None);
        assert_eq!(stage.catalog.examined.as_deref(), Some(&[][..]));
        assert!(stage.catalog.columns.is_empty());
        assert!(stage.validate().is_ok());
        // A value card discovers its own column alone.
        let definitions = [condition, card("region", &[])];
        let stage = lazy(&definitions, None);
        assert_eq!(stage.catalog.examined.as_deref(), Some(&["region".to_owned()][..]));
        assert_eq!(stage.cards[1].values.len(), 2);
        assert!(Arc::ptr_eq(&lazy(&definitions, Some(&stage)).catalog, &stage.catalog));
        // The picker replaces a partial catalog with every column.
        let full = prepare(&source, &definitions, &[], Some(&stage)).unwrap();
        assert!(full.catalog.examined.is_none());
        assert_eq!(full.catalog.columns.len(), 3);
        assert!(Arc::ptr_eq(&lazy(&definitions, Some(&full)).catalog, &full.catalog));
        // A cancelled request falls back to the value cards' columns.
        let cancelled = SlicerDiscovery::default();
        cancelled.cancel();
        let stage =
            SlicerStage::prepare(&source, &definitions, &[], None, Some(&cancelled)).unwrap();
        assert_eq!(stage.catalog.examined.as_deref(), Some(&["region".to_owned()][..]));
    }

    #[test]
    fn cancelling_discovery_interrupts_the_running_query() {
        let source = sql_frame("SELECT 1 AS n");
        let store = source.store.lock().unwrap();
        let discovery = Arc::new(SlicerDiscovery::default());
        let canceller = {
            let discovery = discovery.clone();
            std::thread::spawn(move || {
                while discovery.running.lock().unwrap().is_none() {
                    std::thread::yield_now();
                }
                // An interrupt before the query starts is dropped, so let it start.
                std::thread::sleep(std::time::Duration::from_millis(200));
                discovery.cancel();
            })
        };
        // A filter whose subquery would scan for hours.
        let slow = ColumnFilter {
            sql: true,
            condition: Some("(SELECT count(*) FROM range(1000000000000)) > 0".into()),
            column: String::new(),
            values: Vec::new(),
        };
        let result = discovery.run(&store, || store.slicer_values("n", &[slow], &[]));
        canceller.join().unwrap();
        assert!(result.is_none());
        // The connection works afterwards.
        assert_eq!(store.slicer_values("n", &[], &[]).unwrap().len(), 1);
    }
}
