//! Computing the displayed result: search, slicers, queries, totals, the
//! heat map, copying, and selection statistics.

use super::*;
use crate::application::pivot_editor::{self, AggregationEdit};
use crate::domain::export;
use crate::domain::query;
use crate::domain::statistics;
use crate::ui::aggregation_editor::AggregationPanelEvent;
use crate::ui::pivot_panel::PivotPanelEvent;

impl QueryCandidate {
    fn is_current(&self, view: &DataFrameView) -> bool {
        match (&self.editor, &view.dialog) {
            (CandidateEditor::Aggregation { panel, .. }, Some(Dialog::Aggregation(open))) => {
                open.id == *panel && open.can_finish(self.revision)
            }
            (CandidateEditor::Dimension { panel, .. }, Some(Dialog::Pivot(open))) => {
                open.id == *panel && open.can_finish(self.revision)
            }
            _ => false,
        }
    }

    fn finish_error(&self, view: &mut DataFrameView, error: String) {
        match (&self.editor, &mut view.dialog) {
            (CandidateEditor::Aggregation { panel, .. }, Some(Dialog::Aggregation(open))) if open.id == *panel => {
                open.finish_error(self.revision, error)
            }
            (CandidateEditor::Dimension { panel, .. }, Some(Dialog::Pivot(open))) if open.id == *panel => {
                open.finish_error(self.revision, error)
            }
            _ => {}
        }
    }

    fn finish_success(&self, view: &mut DataFrameView, cx: &mut Cx<DataFrameView>) {
        let mut focus = view.focus;
        match (&self.editor, &mut view.dialog) {
            (CandidateEditor::Aggregation { panel, preferred }, Some(Dialog::Aggregation(open))) if open.id == *panel => {
                open.finish_success(self.config.aggregations.clone(), preferred.clone(), &mut focus);
            }
            (CandidateEditor::Dimension { panel, kind, preferred }, Some(Dialog::Pivot(open))) if open.id == *panel => {
                let entries = match kind {
                    PivotFieldKind::GroupBy => self.config.group_by.clone(),
                    PivotFieldKind::PivotBy => self.config.pivot_by.clone(),
                };
                open.finish_success(entries, preferred.clone(), &mut focus);
            }
            _ => return,
        }
        view.set_focus(focus, cx);
        view.handle_panel_events(cx);
    }
}

impl DataFrameView {
    /// Handles an edit the group or pivot editor asked for.
    pub(crate) fn check_dimension(&mut self, kind: PivotFieldKind, panel: WidgetId, event: PivotPanelEvent, cx: &mut Cx<Self>) {
        let preferred = match &event.edit {
            pivot_editor::PivotEdit::Add(field) => Some(field.clone()),
            pivot_editor::PivotEdit::Replace { replacement, .. } => Some(replacement.clone()),
            pivot_editor::PivotEdit::Move { field, .. } => Some(field.clone()),
            pivot_editor::PivotEdit::Remove(_) => None,
        };
        let mut config = self.session.query_config();
        let candidate = match pivot_editor::apply(&mut config, kind, event.edit.clone()) {
            Err(error) => {
                if let Some(Dialog::Pivot(open)) = &mut self.dialog
                    && open.id == panel
                {
                    open.finish_error(event.revision, error);
                }
                return;
            }
            Ok(changed) => {
                let candidate = QueryCandidate {
                    editor: CandidateEditor::Dimension { panel, kind, preferred },
                    revision: event.revision,
                    config,
                };
                if !changed {
                    candidate.finish_success(self, cx);
                    return;
                }
                candidate
            }
        };
        self.submit_query_candidate(candidate, cx);
    }

    pub(crate) fn check_aggregation(&mut self, panel: WidgetId, event: AggregationPanelEvent, cx: &mut Cx<Self>) {
        let preferred = match &event.edit {
            AggregationEdit::Add(aggregation) => Some(aggregation.clone()),
            AggregationEdit::Replace { replacement, .. } => Some(replacement.clone()),
            AggregationEdit::Move { aggregation, .. } => Some(aggregation.clone()),
            AggregationEdit::Remove(_) => None,
        };
        let mut config = self.session.query_config();
        let candidate = QueryCandidate {
            editor: CandidateEditor::Aggregation { panel, preferred },
            revision: event.revision,
            config: QueryConfig::default(),
        };
        match pivot_editor::apply_aggregation(&mut config, event.edit.clone()) {
            Err(error) => {
                candidate.finish_error(self, error);
                return;
            }
            Ok(false) => {
                QueryCandidate { config, ..candidate }.finish_success(self, cx);
                return;
            }
            Ok(true) => {}
        }
        self.submit_query_candidate(QueryCandidate { config, ..candidate }, cx);
    }

    fn submit_query_candidate(&mut self, candidate: QueryCandidate, cx: &mut Cx<Self>) {
        let mut candidate_session = self.session.clone();
        if let Err(error) = candidate_session.set_query_config(candidate.config.clone(), self.suggested_name()) {
            candidate.finish_error(self, error);
            return;
        }
        let tokens = match query::parse(&self.session.search_text) {
            Ok(tokens) => tokens,
            Err(error) => {
                candidate.finish_error(self, error);
                return;
            }
        };
        let pending_setup = (candidate.config.group_by.is_empty() && !candidate.config.pivot_by.is_empty())
            .then(|| self.session.result.clone());
        let token = self.session.set_search_text(self.session.search_text.clone());
        self.computation.invalidate(token);
        self.busy = true;
        self.debouncing = false;
        let request = PipelineRequest {
            query_candidate: Some(candidate),
            pending_setup,
            query_limit: self.query_limit,
            source: self.session.source.clone(),
            tokens,
            sort: self.session.sort.clone(),
            definition: candidate_session.computation_definition(),
            query_cache: None,
            slicer_discovery: self.slicer_discovery.clone(),
            slicer_options: self.file_settings.slicers,
            queued_at: Instant::now(),
            reuse_table: None,
            table_trace: SqlTrace::default(),
        };
        self.submit_pipeline(token, request, cx);
        cx.notify();
    }

    pub(crate) fn schedule_search(&mut self, text: String, cx: &mut Cx<Self>) {
        self.schedule_computation(text, ScheduleReason::Immediate, cx);
    }

    pub(crate) fn schedule_computation(&mut self, text: String, reason: ScheduleReason, cx: &mut Cx<Self>) {
        if self.session.normalize_empty_query() {
            self.query_cache = None;
            self.sync_frozen_count();
        }
        let context = if self.is_table() {
            KeyContext::TableGrid
        } else {
            KeyContext::QueryGrid
        };
        self.grid.set_keymap(self.keymap.clone(), context);
        let token = self.session.set_search_text(text.clone());
        self.computation.invalidate(token);
        self.debouncing = reason.debounce().is_some();
        self.busy = true;
        self.error = None;
        if self.session.working.as_ref().is_some_and(|view| {
            matches!(&view.view, ViewKind::Query(config) if !config.pivot_by.is_empty() && config.group_by.is_empty())
        }) {
            self.debouncing = false;
            self.busy = self.computation.is_running();
            cx.notify();
            return;
        }
        let tokens = match query::parse(&text) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.rollback_navigation(cx);
                self.session
                    .publish_sql(self.session.token(), SqlTrace::default(), Some(error.clone()));
                self.error = Some(error);
                self.debouncing = false;
                self.busy = self.computation.is_running();
                cx.notify();
                return;
            }
        };
        let source = self.session.source.clone();
        let definition = self.session.computation_definition();
        let query_cache = match (definition.as_ref(), self.query_cache.as_ref()) {
            (Some(definition), Some(cache)) if cache.matches(&source, definition, self.query_limit) => {
                Some((cache.frame.clone(), cache.trace.clone()))
            }
            _ => None,
        };
        let request = PipelineRequest {
            query_candidate: None,
            pending_setup: None,
            query_limit: self.query_limit,
            source,
            tokens,
            sort: self.session.sort.clone(),
            definition,
            query_cache,
            slicer_discovery: self.slicer_discovery.clone(),
            slicer_options: self.file_settings.slicers,
            queued_at: Instant::now(),
            reuse_table: self
                .table_query
                .clone()
                .map(|(query, _)| (query, self.session.result.clone())),
            table_trace: self
                .table_query
                .as_ref()
                .map(|(_, trace)| trace.clone())
                .unwrap_or_default(),
        };
        cx.notify();
        if let Some(delay) = reason.debounce() {
            let start = Instant::now();
            cx.timer(delay, move |view, cx| {
                crate::infrastructure::performance::record("debounce_wait", start);
                view.submit_pipeline(token, request, cx);
            })
            .detach();
        } else {
            self.submit_pipeline(token, request, cx);
        }
    }

    fn submit_pipeline(&mut self, token: JobToken, mut request: PipelineRequest, cx: &mut Cx<Self>) {
        if !self.session.accepts(token) {
            return;
        }
        self.debouncing = false;
        request.queued_at = Instant::now();
        if let Some((token, request)) = self.computation.enqueue(token, request) {
            self.start_pipeline(token, request, cx);
        }
    }

    fn start_pipeline(&mut self, token: JobToken, mut request: PipelineRequest, cx: &mut Cx<Self>) {
        let slicer_cache = self.slicer_stage.clone();
        let query_candidate = request.query_candidate.take();
        let pending_setup = request.pending_setup.is_some();
        cx.spawn(
            move || run_pipeline(token, request, slicer_cache),
            move |view: &mut Self, cx, result| view.finish_pipeline(token, query_candidate, pending_setup, result, cx),
        )
        .detach();
    }

    fn finish_pipeline(
        &mut self,
        token: JobToken,
        query_candidate: Option<QueryCandidate>,
        pending_setup: bool,
        result: PipelineOutcome,
        cx: &mut Cx<Self>,
    ) {
        let view = self;
        let next = view.computation.finish(token);
        // Reuse discovery even when newer input invalidated this publication.
        if let Some(stage) = &result.1
            && stage.matches_source(&view.session.source)
        {
            view.slicer_stage = Some(stage.clone());
        }
        let candidate_valid = query_candidate
            .as_ref()
            .is_none_or(|candidate| candidate.is_current(view));
        if let Some(candidate) = &query_candidate
            && (!candidate_valid || !view.session.accepts(token))
        {
            candidate.finish_error(view, "The expression or view changed while checking. Apply again.".into());
        }
        if view.session.accepts(token) && view.computation.may_publish(token) && candidate_valid {
            let (mut result, slicer_stage, trace, query_trace) = result;
            if let Some(candidate) = &query_candidate
                && result.is_ok()
                && let Err(error) = view
                    .session
                    .set_query_config(candidate.config.clone(), view.suggested_name())
            {
                result = Err(error);
            }
            let publication_token = view.session.token();
            if !pending_setup || result.is_err() {
                view.session
                    .publish_sql(publication_token, trace, result.as_ref().err().cloned());
            }
            let cards = slicer_stage
                .as_ref()
                .map(|stage| stage.cards.clone())
                .unwrap_or_default();
            view.slicer_stage = slicer_stage;
            view.slicer_sidebar.set_cards(cards);
            view.update_slicer_picker(result.as_ref().err().cloned());
            match result {
                Ok((mut snapshot, pivot_frame, effective_sort, table_query)) => {
                    if pending_setup {
                        if let Some(candidate) = &query_candidate {
                            candidate.finish_success(view, cx);
                        }
                    } else {
                        snapshot.generation = publication_token.request_revision;
                        let _publication = crate::infrastructure::performance::Span::new("result_publication");
                        snapshot.result_revision = view.session.result.result_revision + 1;
                        if crate::infrastructure::performance::enabled() {
                            crate::infrastructure::performance::event(serde_json::json!({
                                "metric": "published_generation",
                                "session_generation": token.session_generation,
                                "source_revision": token.source_revision,
                                "request_revision": token.request_revision,
                                "result_revision": snapshot.result_revision,
                                "rows": snapshot.row_count(),
                            }));
                        }
                        if let Some(sort) = effective_sort
                            && sort != view.session.sort
                        {
                            view.session.sort = sort.clone();
                            if let Some(definition) = &mut view.session.working {
                                match &mut definition.view {
                                    ViewKind::Table(config) => config.sort = sort.clone(),
                                    ViewKind::Sql(config) => config.result_sort = sort.clone(),
                                    ViewKind::Query(config) => config.result_sort = sort.clone(),
                                }
                            }
                            view.session.dirty = true;
                        }
                        view.query_cache = match (&view.session.working, pivot_frame) {
                            (Some(definition), Some(frame)) => {
                                if let ViewKind::Query(config) = &definition.view {
                                    Some(QueryResultCache {
                                        query_limit: view.query_limit,
                                        source: view.session.source.clone(),
                                        config: config.clone(),
                                        slicers: definition.slicers.clone(),
                                        filters: definition.filters.clone(),
                                        frame,
                                        trace: query_trace.unwrap_or_default(),
                                    })
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };
                        view.table_query = table_query.map(|query| (query, view.session.sql.displayed.clone()));
                        view.session.result = snapshot.clone();
                        view.information_revision += 1;
                        view.information_busy = false;
                        let terms = query::highlight_terms(&view.session.search_text);
                        view.grid.set_sort(view.session.sort.clone());
                        view.grid.set_highlights(terms);
                        view.grid.replace_result(snapshot);
                        if view.navigation_backup.is_some() {
                            view.grid.reset_navigation_position();
                        }
                        view.sync_frozen_count();
                        view.sync_grid_widths(cx);
                        view.refresh_totals(cx);
                        view.refresh_heatmap(cx);
                        view.refresh_cell_detail();
                        let context = if view.is_table() {
                            KeyContext::TableGrid
                        } else {
                            KeyContext::QueryGrid
                        };
                        view.grid.set_keymap(view.keymap.clone(), context);
                        view.error = None;
                        if let Some(candidate) = &query_candidate {
                            candidate.finish_success(view, cx);
                        }
                        if view.navigation_backup.take().is_some() {
                            view.reset_sql_editor();
                            view.commit_navigation_history();
                            if let Err(error) = view.record_opened() {
                                eprintln!("history update failed: {error}");
                            }
                        } else {
                            view.remember_navigation_current();
                            view.undo_history.record(view.session.view_state());
                            view.schedule_auto_save(cx);
                        }
                    }
                }
                Err(error) => {
                    if let Some(candidate) = &query_candidate {
                        candidate.finish_error(view, error.clone());
                    }
                    if query_candidate.is_none() {
                        view.rollback_navigation(cx);
                        view.error = Some(error.to_string());
                    }
                }
            }
        }
        if let Some((next_token, request)) = next {
            view.start_pipeline(next_token, request, cx);
        }
        view.busy = view.computation.is_running() || view.debouncing;
        view.drain_grid_events(cx);
        cx.notify();
    }

    pub(crate) fn schedule_copy(&mut self, result: ResultSnapshot, selection: Selection, cx: &mut Cx<Self>) {
        self.schedule_copy_format(result, selection, CopyFormat::RawTsv, cx);
    }

    pub(crate) fn schedule_copy_format(&mut self, result: ResultSnapshot, selection: Selection, format: CopyFormat, cx: &mut Cx<Self>) {
        self.copy_revision += 1;
        let revision = self.copy_revision;
        let generation = self.session.token().session_generation;
        self.copy_status = Some("Copying…".into());
        cx.notify();
        cx.spawn(
            move || export::export(&result, selection, format),
            move |view: &mut Self, cx, text: Result<String, String>| {
                if view.copy_revision != revision || view.session.token().session_generation != generation {
                    return;
                }
                match text {
                    Ok(text) => {
                        crate::ui::platform::system::write_clipboard(&text);
                        view.copy_status = Some("Copied".into());
                    }
                    Err(error) => view.copy_status = Some(format!("Copy failed: {error}")),
                }
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn schedule_statistics(&mut self, result: ResultSnapshot, selection: Selection, cx: &mut Cx<Self>) {
        self.statistics_revision += 1;
        let revision = self.statistics_revision;
        let generation = self.session.token().session_generation;
        let result_revision = result.result_revision;
        let (rows, columns) = selection.rectangle();
        let row_count = rows.count();
        let column_count = columns.count();
        self.selection_counts = Some((row_count, column_count, selection.cell_count()));
        self.statistics = None;
        self.statistics_error = None;
        self.statistics_pending = true;
        self.pending_statistics = Some(PendingStatistics {
            result,
            selection,
            revision,
            generation,
            result_revision,
            deadline: cx.now() + Duration::from_millis(50),
        });
        cx.notify();
        if self.statistics_running {
            return;
        }
        self.statistics_running = true;
        self.run_statistics(cx);
    }

    /// Calculates the newest pending statistics once their pause has passed,
    /// one calculation at a time.
    fn run_statistics(&mut self, cx: &mut Cx<Self>) {
        let Some(deadline) = self.pending_statistics.as_ref().map(|pending| pending.deadline) else {
            self.statistics_running = false;
            return;
        };
        let now = cx.now();
        if deadline > now {
            cx.timer(deadline - now, |view, cx| view.run_statistics(cx)).detach();
            return;
        }
        let Some(pending) = self.pending_statistics.take() else {
            self.statistics_running = false;
            return;
        };
        let (revision, generation, result_revision) = (pending.revision, pending.generation, pending.result_revision);
        cx.spawn(
            move || statistics::calculate(&pending.result, pending.selection),
            move |view: &mut Self, cx, stats: Result<SelectionStatistics, String>| {
                if view.statistics_revision == revision
                    && view.session.token().session_generation == generation
                    && view.session.result.result_revision == result_revision
                {
                    view.statistics_pending = false;
                    match stats {
                        Ok(stats) => {
                            view.statistics = Some(stats);
                            view.statistics_error = None;
                        }
                        Err(error) => view.statistics_error = Some(error),
                    }
                    cx.notify();
                }
                view.run_statistics(cx);
            },
        )
        .detach();
    }

    pub(crate) fn recalculate_widths(&mut self, cx: &mut Cx<Self>) {
        let owner = self.width_owner();
        let source = self.is_table().then(|| self.session.source.clone());
        self.grid.reset_widths();
        self.grid.configure_widths(owner, source, Default::default(), cx);
    }

    pub(crate) fn toggle_totals(&mut self, cx: &mut Cx<Self>) {
        let has_groups = matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(config)) if !config.group_by.is_empty()
        );
        if !has_groups {
            self.error = Some("Totals apply to queries with a group-by".into());
            cx.notify();
            return;
        }
        if let Err(error) = self.session.edit_definition(|definition| {
            if let ViewKind::Query(config) = &mut definition.view {
                config.totals = !config.totals;
            }
        }) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    pub(crate) fn toggle_heatmap(&mut self, cx: &mut Cx<Self>) {
        if !matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(config)) if !config.group_by.is_empty()
        ) {
            self.error = Some("The heat map applies to queries with a group-by".into());
            cx.notify();
            return;
        }
        if let Err(error) = self.session.edit_definition(|definition| {
            if let ViewKind::Query(config) = &mut definition.view {
                config.heatmap = !config.heatmap;
            }
        }) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    /// Shades a query's measure columns when its heat map is on. Each
    /// column's range covers the whole query result, not only the rows a
    /// search leaves, so colors stay put while searching.
    fn refresh_heatmap(&mut self, cx: &mut Cx<Self>) {
        self.heat_revision += 1;
        let config = match self.session.working.as_ref().map(|view| &view.view) {
            Some(ViewKind::Query(config)) if config.heatmap => Some(config.clone()),
            _ => None,
        };
        let frame = self.query_cache.as_ref().map(|cache| cache.frame.clone());
        let (Some(config), Some(frame)) = (config, frame) else {
            self.heat_ranges = None;
            self.grid.set_heat_ranges(Default::default());
            return;
        };
        let store = std::sync::Arc::downgrade(&frame.store);
        if let Some((cached, ranges)) = &self.heat_ranges
            && cached.ptr_eq(&store)
        {
            self.grid.set_heat_ranges(ranges.clone());
            return;
        }
        let columns = frame
            .columns
            .iter()
            .zip(frame.dtypes.iter())
            .map(|(column, dtype)| (column.0.clone(), dtype.clone()))
            .collect::<Vec<_>>();
        let groups = crate::domain::heatmap::scale_groups(&config, &columns);
        let revision = self.heat_revision;
        cx.spawn(
            move || {
                let store = frame
                    .store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                let mut ranges = std::collections::HashMap::new();
                for group in groups {
                    let range = crate::domain::heatmap::combined(store.numeric_ranges(&group)?);
                    if let Some(range) = range {
                        ranges.extend(group.into_iter().map(|name| (name, range)));
                    }
                }
                Ok::<_, String>(ranges)
            },
            move |view: &mut Self, cx, ranges| {
                if view.heat_revision != revision {
                    return;
                }
                match ranges {
                    Ok(ranges) => {
                        view.heat_ranges = Some((store, ranges.clone()));
                        view.grid.set_heat_ranges(ranges);
                    }
                    Err(error) => view.error = Some(format!("Heat map failed: {error}")),
                }
                cx.notify();
            },
        )
        .detach();
    }

    /// Shows the grand total row under a query with totals, computing it from
    /// the source on a worker when the query, slicers, or filters changed.
    fn refresh_totals(&mut self, cx: &mut Cx<Self>) {
        let key = match self.session.working.as_ref() {
            Some(definition) => match &definition.view {
                ViewKind::Query(config) if config.totals && !config.group_by.is_empty() && !config.aggregations.is_empty() => {
                    Some(TotalsKey {
                        source_revision: self.session.source.source_revision,
                        store: std::sync::Arc::as_ptr(&self.session.source.store) as usize,
                        config: QueryConfig {
                            result_sort: Vec::new(),
                            ..config.clone()
                        },
                        slicers: self
                            .slicer_stage
                            .as_ref()
                            .map(|stage| stage.effective())
                            .unwrap_or_else(|| definition.slicers.clone()),
                        filters: definition.filters.clone(),
                    })
                }
                _ => None,
            },
            None => None,
        };
        self.totals_revision += 1;
        let Some(key) = key else {
            self.totals = None;
            self.grid.set_footer(None);
            return;
        };
        if self.totals.as_ref().is_some_and(|(cached, _)| cached == &key) {
            self.apply_totals();
            return;
        }
        let revision = self.totals_revision;
        let source = self.session.source.clone();
        let job = key.clone();
        cx.spawn(
            move || {
                let store = source
                    .store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                crate::domain::pivot::execute_totals(
                    &store,
                    &job.config,
                    &job.slicers,
                    &job.filters,
                    &crate::domain::pivot::EvaluationClock::system(),
                )
            },
            move |view: &mut Self, cx, totals| {
                if view.totals_revision != revision {
                    return;
                }
                match totals {
                    Ok(totals) => {
                        view.totals = Some((key, totals));
                        view.apply_totals();
                    }
                    Err(error) => view.error = Some(format!("Totals failed: {error}")),
                }
                cx.notify();
            },
        )
        .detach();
    }

    fn apply_totals(&mut self) {
        let Some((_, values)) = &self.totals else {
            return;
        };
        let narrowed = !self.session.search_text.trim().is_empty()
            || self
                .query_cache
                .as_ref()
                .is_some_and(|cache| cache.frame.cut_at.is_some());
        let footer = crate::ui::grid::GridFooter {
            // Totals cover every group, including any search or the limit hides.
            label: if narrowed {
                "Total (all groups)".into()
            } else {
                "Total".into()
            },
            values: values.clone(),
        };
        self.grid.set_footer(Some(footer));
    }

    /// A query result that stopped at the query limit is missing rows, so an
    /// export reruns the query without the limit.
    pub(crate) fn unlimited_query_rerun(&self) -> Option<UnlimitedQuery> {
        let definition = self.session.working.as_ref()?;
        let ViewKind::Query(config) = &definition.view else {
            return None;
        };
        let cache = self.query_cache.as_ref()?;
        if cache.frame.cut_at.is_none() {
            return None;
        }
        let tokens = query::parse(&self.session.search_text).ok()?;
        Some(UnlimitedQuery {
            source: self.session.source.clone(),
            config: config.clone(),
            slicers: self
                .slicer_stage
                .as_ref()
                .map(|stage| stage.effective())
                .unwrap_or_else(|| definition.slicers.clone()),
            filters: definition.filters.clone(),
            tokens,
            sort: self.session.sort.clone(),
        })
    }
}

type PipelineSuccess = (ResultSnapshot, Option<FrameSnapshot>, Option<Vec<SortKey>>, Option<ViewQuery>);
type PipelineOutcome = (Result<PipelineSuccess, String>, Option<SlicerStage>, SqlTrace, Option<SqlTrace>);

/// Runs a pipeline request on a worker: slicers, the query, search, and sort.
fn run_pipeline(token: JobToken, request: PipelineRequest, slicer_cache: Option<SlicerStage>) -> PipelineOutcome {
    let mut trace = SqlTrace::default();
    let mut query_trace = None;
    crate::infrastructure::performance::record("worker_queue_wait", request.queued_at);
    let _pipeline = crate::infrastructure::performance::Span::new("pipeline");
    let definitions = request
        .definition
        .as_ref()
        .map(|definition| definition.slicers.as_slice())
        .unwrap_or_default();
    if let Some(snapshot) = request.pending_setup {
        let result = (|| {
            let store = request
                .source
                .store
                .lock()
                .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
            if let Some(definition) = &request.definition
                && let ViewKind::Query(config) = &definition.view
            {
                crate::domain::pivot::validate_dimensions(&store, config)?;
            }
            Ok((snapshot, None, None, None))
        })();
        return (result, slicer_cache, trace, query_trace);
    }
    let slicer_stage = if request.slicer_discovery.is_some()
        || !definitions.is_empty()
        || slicer_cache
            .as_ref()
            .is_some_and(|stage| stage.matches_source(&request.source))
    {
        let value_filters = request
            .definition
            .as_ref()
            .map(|definition| definition.filters.as_slice())
            .unwrap_or_default();
        match SlicerStage::prepare(
            &request.source,
            definitions,
            value_filters,
            request.slicer_options,
            slicer_cache.as_ref(),
            request.slicer_discovery.as_deref(),
        ) {
            Ok(stage) => Some(stage),
            Err(error) => return (Err(error), None, trace, query_trace),
        }
    } else {
        None
    };
    let result = (|| {
        if let Some(stage) = &slicer_stage {
            stage.validate()?;
        }
        let effective_slicers = slicer_stage
            .as_ref()
            .map(|stage| stage.effective())
            .unwrap_or_else(|| definitions.to_vec());
        if let Some(definition) = &request.definition
            && let ViewKind::Query(config) = &definition.view
        {
            let pivot_source = if let Some((cached, cached_trace)) = request.query_cache.clone() {
                let cached_trace = cached_trace.cached();
                trace.append(&cached_trace);
                query_trace = Some(cached_trace);
                cached
            } else {
                let source_store = request
                    .source
                    .store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                let pivot_store = crate::domain::pivot::execute_filtered(
                    &source_store,
                    config,
                    &effective_slicers,
                    &definition.filters,
                    &crate::domain::pivot::EvaluationClock::system(),
                    request.query_limit,
                    &mut trace,
                )?;
                query_trace = Some(trace.clone());
                drop(source_store);
                FrameSnapshot::new(pivot_store, request.source.source_revision)
            };
            let pivot_store = pivot_source
                .store
                .lock()
                .map_err(|_| "DuckDB query store lock poisoned".to_owned())?;
            let schema = pivot_store.schema().to_vec();
            let sort = available_sort_keys(&request.sort, &schema);
            let view = ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: request.tokens,
                filters: Vec::new(),
                sort: sort.clone(),
            };
            // A result that stopped at the limit is missing rows, so
            // search and sort rerun inside the query to see all of them.
            // The unnarrowed result stays cached for the next request.
            let narrowed = if pivot_store.cut_at().is_some()
                && (!view.tokens.is_empty() || !view.sort.is_empty())
            {
                let source_store = request
                    .source
                    .store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                let store = crate::domain::pivot::execute_query(
                    &source_store,
                    config,
                    &effective_slicers,
                    &definition.filters,
                    &crate::domain::pivot::EvaluationClock::system(),
                    request.query_limit,
                    Some((&pivot_store, &view)),
                    &mut trace,
                )?;
                Some(FrameSnapshot::new(store, request.source.source_revision))
            } else {
                None
            };
            drop(pivot_store);
            let shown = narrowed.as_ref().unwrap_or(&pivot_source);
            let rows = shown
                .store
                .lock()
                .map_err(|_| "DuckDB query store lock poisoned".to_owned())?
                .query_view_rows_traced(&view, &mut trace)?;
            let snapshot = ResultSnapshot::from_view(shown, rows, schema, 0, token.request_revision);
            snapshot.prefetch_page(0)?;
            return Ok::<_, String>((snapshot, Some(pivot_source), Some(sort), None));
        }

        let store = request
            .source
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
        let filters = crate::application::slicers::filters(&effective_slicers);
        let all_columns = store.schema();
        let sort = available_sort_keys(&request.sort, all_columns);
        let hidden = match request.definition.as_ref().map(|definition| &definition.view) {
            // Hiding only changes rows while searching, so an
            // unsearched view can reuse its rows.
            Some(ViewKind::Table(config)) if !request.tokens.is_empty() => config.hidden.clone(),
            _ => Vec::new(),
        };
        let effective_query = ViewQuery {
            hidden,
            value_filters: request
                .definition
                .as_ref()
                .map(|definition| definition.filters.clone())
                .unwrap_or_default(),
            tokens: request.tokens,
            filters: filters
                .into_iter()
                .filter(|filter| {
                    !filter.values.is_empty()
                        || filter
                            .condition
                            .as_ref()
                            .is_some_and(|text| !text.trim().is_empty())
                })
                .collect(),
            sort: sort.clone(),
        };
        let reused = request.reuse_table.filter(|(query, result)| {
            query == &effective_query
                && result.source_revision == request.source.source_revision
                && std::sync::Arc::ptr_eq(&result.store, &request.source.store)
        });
        let rows = if reused.is_none() {
            Some(store.query_view_rows_traced(&effective_query, &mut trace)?)
        } else {
            None
        };
        if reused.is_some() {
            trace.append(&request.table_trace.cached());
        }
        let schema = match request.definition.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(config)) => {
                let available = all_columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect::<Vec<_>>();
                let order = crate::domain::table::visible_order(&available, config);
                order
                    .iter()
                    .filter_map(|name| all_columns.iter().find(|column| &column.name == name).cloned())
                    .collect()
            }
            _ => all_columns.to_vec(),
        };
        drop(store);
        let snapshot = if let Some((_, previous)) = reused {
            previous.reproject(&request.source, &schema, token.request_revision)
        } else {
            ResultSnapshot::from_view(
                &request.source,
                rows.expect("queried rows"),
                schema,
                0,
                token.request_revision,
            )
        };
        snapshot.prefetch_page(0)?;
        Ok::<_, String>((snapshot, None, Some(sort), Some(effective_query)))
    })();
    (result, slicer_stage, trace, query_trace)
}
