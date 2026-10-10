use crate::application::commands::Command;
use crate::application::keymap::{ACTIONS, KeymapFile};

pub(super) const PALETTE_WIDTH: f32 = 896.0;

pub(super) fn matches_terms(searchable: &str, terms: &[&str]) -> bool {
    let searchable = searchable.to_lowercase();
    terms.iter().all(|term| searchable.contains(term))
}

pub(super) fn matching_commands(
    commands: &[&'static Command],
    keymap: &KeymapFile,
    query: &str,
) -> Vec<(&'static Command, String)> {
    let query = query.to_lowercase();
    let terms = query.split_whitespace().collect::<Vec<_>>();
    commands
        .iter()
        .filter_map(|&command| {
            let binding = ACTIONS
                .iter()
                .find(|spec| spec.command == Some(command.id))
                .map(|spec| {
                    keymap
                        .bindings_for(spec.id, spec.context)
                        .into_iter()
                        .map(|(stroke, _)| stroke)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|binding| !binding.is_empty());
            let label = binding
                .map(|binding| format!("{} · {binding}", command.label))
                .unwrap_or_else(|| command.label.into());
            let searchable = format!("{label} {}", command.key);
            matches_terms(&searchable, &terms).then_some((command, label))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::commands::{COMMANDS, CommandId};

    #[test]
    fn every_term_must_match_regardless_of_case_order_or_whitespace() {
        let commands = COMMANDS.iter().collect::<Vec<_>>();
        let keymap = KeymapFile::default();
        let ids = |query| {
            matching_commands(&commands, &keymap, query)
                .into_iter()
                .map(|(command, _)| command.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("  BY\tORDER  "), [CommandId::OrderBy]);
        assert_eq!(ids("reset reload"), [CommandId::ReloadReset]);
        assert!(ids("reload missing").is_empty());
        assert_eq!(ids(" \t\n ").len(), commands.len());
        assert_eq!(
            ids("reload reload"),
            [CommandId::Reload, CommandId::ReloadReset]
        );
    }

    #[test]
    fn terms_can_match_labels_shortcuts_and_command_names() {
        let commands = COMMANDS.iter().collect::<Vec<_>>();
        let rows = matching_commands(&commands, &KeymapFile::default(), "shift copy_as");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.id, CommandId::CopyAs);
        assert_eq!(rows[0].1, "Copy As · ctrl-shift-c");
    }
}
