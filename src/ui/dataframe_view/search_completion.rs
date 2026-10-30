use super::*;

impl DataFrameView {
    /// Shown columns come first, then the rest of the source's columns.
    fn completion_columns(&self) -> Vec<String> {
        let mut names = self.result_column_names();
        for column in self.session.source.columns.iter() {
            if !names.contains(&column.0) {
                names.push(column.0.clone());
            }
        }
        names
    }

    /// Hands the column names to every `"` completion when the result or
    /// source columns change.
    pub(crate) fn sync_completion_columns(&mut self) {
        let key = (
            std::sync::Arc::as_ptr(&self.session.result.columns) as *const () as usize,
            std::sync::Arc::as_ptr(&self.session.source.columns) as *const () as usize,
        );
        if key == self.completion_columns_key {
            return;
        }
        self.completion_columns_key = key;
        let columns = self.completion_columns();
        self.search_completion.set_columns(columns.clone());
        self.slicer_sidebar.set_columns(columns);
        let dialect = crate::domain::expression_completion::dialect(self.session.source.engine);
        self.slicer_sidebar.set_dialect(dialect);
    }

    /// The search text changed by typing: refresh its column list and search.
    pub(crate) fn search_edited(&mut self, cx: &mut Cx<Self>) {
        self.sync_completion_columns();
        self.search_completion.refresh(&self.search_input);
        let text = self.search_input.text().to_owned();
        if text != self.session.search_text {
            self.schedule_computation(text, ScheduleReason::TextEdit, cx);
        }
    }
}
