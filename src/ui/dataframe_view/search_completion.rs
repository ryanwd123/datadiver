use super::*;
use crate::ui::column_completion::{ColumnCompletion, ColumnCompletionEvent, ColumnSyntax};

impl DataFrameView {
    pub(super) fn new_search_completion(
        search_input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> (Entity<ColumnCompletion>, Subscription) {
        let completion =
            cx.new(|cx| ColumnCompletion::new(search_input.clone(), ColumnSyntax::Search, cx));
        let subscription = cx.subscribe(&completion, |view, _, event, cx| match event {
            ColumnCompletionEvent::Accepted => {
                let text = view.search_input.read(cx).value().to_string();
                if text != view.session.search_text {
                    view.schedule_computation(text, ScheduleReason::TextEdit, cx);
                }
            }
        });
        (completion, subscription)
    }

    /// Shown columns come first, then the rest of the source's columns.
    fn completion_columns(&self) -> Vec<String> {
        let mut names = self
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        for column in self.session.source.columns.iter() {
            if !names.contains(&column.0) {
                names.push(column.0.clone());
            }
        }
        names
    }

    /// Hands the column names to every `"` completion when the result or
    /// source columns change.
    pub(super) fn sync_completion_columns(&mut self, cx: &mut Context<Self>) {
        let key = (
            std::sync::Arc::as_ptr(&self.session.result.columns) as *const () as usize,
            std::sync::Arc::as_ptr(&self.session.source.columns) as *const () as usize,
        );
        if key == self.completion_columns_key {
            return;
        }
        self.completion_columns_key = key;
        let columns = self.completion_columns();
        self.search_completion
            .update(cx, |completion, _| completion.set_columns(columns.clone()));
        self.slicer_sidebar
            .update(cx, |sidebar, cx| sidebar.set_columns(columns, cx));
    }
}
