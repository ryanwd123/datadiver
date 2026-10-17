use super::test_support::{Viewer, opened_url};
use crate::application::commands::CommandId;
use crate::domain::documents::ViewAction;
use crate::ui::hit::{Button, Hit};

/// Selects the second row's second cell, then presses `keys`.
fn press_keys(viewer: &Viewer, keys: &str) {
    viewer.select(1, 1);
    viewer.press(keys);
    viewer.run_until_parked();
}

#[test]
fn space_x_runs_the_keyed_action_for_the_active_row() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("people.csv");
    std::fs::write(&path, "name,city\nAda,London\nO'Brien & Co,New York\n").unwrap();
    let viewer = Viewer::file(&path);

    // Without actions, the menu key opens the editor instead.
    press_keys(&viewer, "space x");
    assert!(viewer.has_dialog());
    assert!(!viewer.read(|view| view.action_menu_open));
    viewer.close_dialog();

    viewer.update(|view, _| {
        view.ensure_working_view();
        view.session
            .edit_definition(|definition| {
                definition.actions = vec![
                    ViewAction {
                        key: "M".into(),
                        description: "Map".into(),
                        command: "https://maps.example/?q={name} {city}".into(),
                    },
                    ViewAction {
                        key: "b".into(),
                        description: "Broken".into(),
                        command: "datadiver-missing-program {name}".into(),
                    },
                ]
            })
            .unwrap();
    });
    viewer.run_until_parked();

    press_keys(&viewer, "space x");
    assert!(viewer.read(|view| view.action_menu_open));
    // Lowercase m is not the action's key, so the menu stays open.
    press_keys(&viewer, "m");
    assert!(viewer.read(|view| view.action_menu_open));
    assert_eq!(opened_url(), None);
    press_keys(&viewer, "shift-m");
    assert_eq!(
        opened_url().as_deref(),
        Some("https://maps.example/?q=O%27Brien%20%26%20Co%20New%20York")
    );
    viewer.read(|view| {
        assert!(!view.action_menu_open);
        assert!(view.error.is_none(), "{:?}", view.error);
    });

    press_keys(&viewer, "space x b");
    let error = viewer.read(|view| view.error.clone()).unwrap();
    assert!(error.contains("Broken") && error.contains("datadiver-missing-program"));

    press_keys(&viewer, "space x escape");
    assert!(!viewer.read(|view| view.action_menu_open));

    // The editor's Run button uses the active row too, here the first one.
    viewer.select(0, 0);
    viewer.update(|view, cx| view.dispatch_command(CommandId::EditActions, cx));
    assert!(viewer.has_dialog());
    viewer.click(|hit| *hit == Hit::Button(Button::ActionRun(0)));
    viewer.run_until_parked();
    assert_eq!(opened_url().as_deref(), Some("https://maps.example/?q=Ada%20London"));
}
