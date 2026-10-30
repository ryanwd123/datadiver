//! Auto-save: a saved view with it on, and the startup script, store each
//! change a moment after it is made, and before the window moves on.

use super::*;

/// How long after the last change auto-save waits, so typing and dragging
/// save once rather than on every step.
const AUTO_SAVE_DELAY: Duration = Duration::from_millis(2000);

impl DataFrameView {
    fn auto_saves(&self) -> bool {
        matches!(
            self.session.identity,
            SessionIdentity::SavedTable(_)
                | SessionIdentity::SavedQuery(_)
                | SessionIdentity::SavedSql(_)
                | SessionIdentity::StartupSql
        ) && self.session.working.as_ref().is_some_and(|definition| definition.auto_save)
    }

    /// Whether the view has Auto-save on.
    pub(crate) fn auto_save(&self) -> bool {
        self.session.working.as_ref().is_some_and(|definition| definition.auto_save)
    }

    /// Turns Auto-save on or off. A saved view stores the switch at once; a
    /// draft asks for a name first.
    pub(crate) fn set_auto_save(&mut self, enabled: bool, cx: &mut Cx<Self>) {
        self.ensure_working_view();
        let was_dirty = self.session.dirty;
        if let Some(definition) = self.session.working.as_mut() {
            definition.auto_save = enabled;
            self.session.dirty = true;
        }
        self.auto_save_task = None;
        match self.session.identity {
            SessionIdentity::StartupSql => {
                let result = self.repository.load_settings().and_then(|mut settings| {
                    settings.startup_sql_auto_save = enabled;
                    self.repository.save_settings(&settings)
                });
                if let Err(error) = result {
                    self.error = Some(format!("Cannot save startup SQL settings: {error}"));
                } else {
                    self.session.dirty = was_dirty;
                    if enabled {
                        self.persist_view();
                    }
                }
            }
            SessionIdentity::SavedTable(_) | SessionIdentity::SavedQuery(_) | SessionIdentity::SavedSql(_) => {
                self.persist_view()
            }
            _ if enabled => self.open_save_dialog(cx),
            _ => {}
        }
        cx.notify();
    }

    /// Writes the working view over its saved copy, or the startup script
    /// to its file, when either has unsaved changes.
    pub(crate) fn persist_view(&mut self) {
        if !self.session.dirty
            || !matches!(
                self.session.identity,
                SessionIdentity::SavedTable(_)
                    | SessionIdentity::SavedQuery(_)
                    | SessionIdentity::SavedSql(_)
                    | SessionIdentity::StartupSql
            )
        {
            return;
        }
        let Some(mut definition) = self.session.working.clone() else {
            return;
        };
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            let ViewKind::Sql(config) = &definition.view else {
                return;
            };
            match self.repository.save_startup_sql(&config.text) {
                Ok(()) => {
                    self.session.dirty = false;
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("Cannot save {}: {error}", crate::infrastructure::source_database::STARTUP_SCRIPT)),
            }
            return;
        }
        match self.repository.save_view(&mut definition) {
            Ok(_) => {
                self.session.saved(definition);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Auto-save failed: {error}")),
        }
    }

    /// Saves a pending auto-save now, as the view is left or the window closes.
    pub(crate) fn flush_auto_save(&mut self) {
        self.auto_save_task = None;
        if self.auto_saves() {
            self.persist_view();
        }
    }

    /// Saves after [`AUTO_SAVE_DELAY`] without another change.
    pub(crate) fn schedule_auto_save(&mut self, cx: &mut Cx<Self>) {
        self.auto_save_task = None;
        if !self.auto_saves() || !self.session.dirty {
            return;
        }
        self.auto_save_task = Some(cx.timer(AUTO_SAVE_DELAY, |view, cx| {
            view.persist_view();
            cx.notify();
        }));
    }
}
