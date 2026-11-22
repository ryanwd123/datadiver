#![windows_subsystem = "windows"]
use std::path::PathBuf;

use datadiver::application::state::ApplicationState;
use datadiver::domain::documents::DocumentDefinition;
use datadiver::infrastructure::duckdb_store::DuckDbStore;
use datadiver::infrastructure::json_store::{StorePaths, ViewRepository};
use datadiver::ui::dataframe_view::ViewerOptions;
use datadiver::{DataFrameView, FrameSnapshot};

/// What a DataDiver answers to, so a dashboard opened while it runs with
/// the same profile opens in it: one process per profile database.
fn instance_name(paths: &StorePaths) -> String {
    format!("DataDiver profile {}", paths.config.to_string_lossy().to_lowercase())
}

/// The dashboard and profile of `datadiver [--home DIR] board.ddash`, which
/// a running DataDiver can open instead.
fn dashboard_to_hand_off(arguments: &[String]) -> Option<(PathBuf, StorePaths)> {
    let (path, home) = match arguments {
        [path] => (path, None),
        [flag, home, path] | [path, flag, home] if flag == "--home" => (path, Some(home)),
        _ => return None,
    };
    let path = PathBuf::from(path);
    if !datadiver::infrastructure::dashboard::is_dashboard(&path) {
        return None;
    }
    let paths = match home {
        Some(home) => StorePaths::for_home(std::path::Path::new(home)),
        None => StorePaths::operating_system(),
    };
    Some((path.canonicalize().unwrap_or(path), paths))
}

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) == Some("dashboard") {
        std::process::exit(datadiver::cli::run_dashboard(&arguments[1..]));
    }
    if let Some((path, paths)) = dashboard_to_hand_off(&arguments) {
        let message = serde_json::json!({ "open": path }).to_string();
        if datadiver::ui::platform::window::send_to_instance(&instance_name(&paths), &message) {
            return;
        }
    }
    let mut args = arguments.into_iter();
    let mut rows = None;
    let mut source_path = None;
    let mut view_path = None;
    let mut home_path = None;
    let mut ephemeral = false;
    let mut no_restore = false;
    let mut viewer_options = ViewerOptions::default();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--benchmark-ui" => {
                viewer_options.benchmark_ui = true;
                datadiver::infrastructure::performance::enable(PathBuf::from(
                    args.next().expect("--benchmark-ui requires a log path"),
                ));
            }
            "--rows" => {
                rows = Some(
                    args.next()
                        .expect("--rows requires a count")
                        .parse::<usize>()
                        .expect("invalid row count"),
                )
            }
            "--file" => {
                source_path = Some(PathBuf::from(args.next().expect("--file requires a path")))
            }
            "--view" => {
                view_path = Some(PathBuf::from(args.next().expect("--view requires a path")))
            }
            "--home" => {
                home_path = Some(PathBuf::from(
                    args.next().expect("--home requires a directory"),
                ))
            }
            "--window" => {
                viewer_options.restore_window = false;
                let dimensions = args.next().expect("--window requires WxH");
                let (width, height) = dimensions
                    .split_once(['x', 'X'])
                    .expect("--window requires WxH");
                viewer_options.width = width.parse().expect("invalid window width");
                viewer_options.height = height.parse().expect("invalid window height");
            }
            "--capture" => {
                viewer_options.capture = Some(PathBuf::from(
                    args.next().expect("--capture requires a path"),
                ));
                if viewer_options.width == 1400.0 && viewer_options.height == 900.0 {
                    viewer_options.width = 1920.0;
                    viewer_options.height = 1080.0;
                }
            }
            "--capture-delay" => {
                viewer_options.capture_delay = std::time::Duration::from_millis(
                    args.next()
                        .expect("--capture-delay requires milliseconds")
                        .parse()
                        .expect("invalid delay"),
                )
            }
            "--exit-after" => {
                viewer_options.exit_after = Some(std::time::Duration::from_millis(
                    args.next()
                        .expect("--exit-after requires milliseconds")
                        .parse()
                        .expect("invalid delay"),
                ))
            }
            "--search" => viewer_options.search_text = args.next().expect("--search requires text"),
            "--slicer" => viewer_options
                .slicers
                .push(args.next().expect("--slicer requires a column")),
            "--show" => {
                let targets = args.next().expect("--show requires a target");
                for target in targets.split(',') {
                    // help:<page> opens the help on that page.
                    if !target.starts_with("help:") && !matches!(
                        target,
                        "info"
                            | "about"
                            | "editor:sql"
                            | "editor:startup-sql"
                            | "editor:sql-results"
                            | "editor:sql-completion"
                            | "editor:sql-hints"
                            | "editor:sql-hover"
                            | "editor:sql-error"
                            | "editor:sql-helper"
                            | "editor:sql-ask"
                            | "editor:sql-ask-log"
                            | "editor:sql-ask-mention"
                            | "theme"
                            | "theme-editor"
                            | "help"
                            | "space"
                            | "command-menu"
                            | "cache-drop"
                            | "save"
                            | "palette:copy-as"
                            | "palette:commands"
                            | "palette:fonts"
                            | "palette:editor-fonts"
                            | "palette:editor-options"
                            | "palette:view-options"
                            | "palette:slicers"
                            | "palette:files"
                            | "palette:folders"
                            | "palette:recovered"
                            | "confirm:archive-sql"
                            | "palette:favorites"
                            | "palette:columns"
                            | "palette:column-chooser"
                            | "panel:group"
                            | "editor:group"
                            | "editor:group-error"
                            | "editor:pivot"
                            | "editor:pivot-error"
                            | "panel:aggregation"
                            | "panel:aggregation-functions"
                            | "panel:pivot"
                            | "panel:order-by"
                            | "editor:order-by"
                            | "editor:having"
                            | "aggregation-menu"
                            | "form:aggregation"
                            | "form:condition"
                            | "editor:aggregation"
                            | "editor:aggregation-error"
                            | "keymap"
                            | "actions"
                            | "formatting"
                            | "formatting:presets"
                            | "formatted"
                            | "formatted:tooltip"
                            | "formatted:selection"
                            | "ai-providers"
                            | "action-menu"
                            | "slicer-search"
                            | "slicer-values"
                            | "paste-filter"
                            | "search-columns"
                            | "editor:aggregation-completion"
                            | "editor:group-completion"
                            | "database-tree"
                            | "database-tree:sql"
                            | "cell-detail"
                    ) {
                        eprintln!("unsupported --show target: {target}");
                        std::process::exit(2);
                    }
                    viewer_options.show.push(target.into());
                }
            }
            "--ephemeral" => ephemeral = true,
            "--no-restore" => {
                no_restore = true;
                viewer_options.restore_window = false;
            }
            "--help" | "-h" => {
                println!(
                    "DataDiver\n  --rows N       open synthetic data\n  --file PATH    open a CSV (including .csv.gz), TSV, XLSX, or Parquet source\n  --view PATH    open a saved JSON view\n  --search TEXT  filter displayed rows\n  --slicer COLUMN  add a source-column slicer\n  --window WxH   set window size\n  --exit-after MS  close on a timer\n  --capture PATH  save a PNG of the window\n  PATH.ddash     open a dashboard\n  dashboard ...  dashboard tools; see `datadiver dashboard help`"
                );
                return;
            }
            _ if !argument.starts_with('-') && source_path.is_none() => {
                source_path = Some(PathBuf::from(argument))
            }
            _ => {
                eprintln!("unsupported argument: {argument}");
                std::process::exit(2);
            }
        }
    }
    if usize::from(rows.is_some())
        + usize::from(source_path.is_some())
        + usize::from(view_path.is_some())
        > 1
    {
        eprintln!("--rows, --file, and --view cannot be combined");
        std::process::exit(2);
    }
    if ephemeral && home_path.is_some() {
        eprintln!("--home and --ephemeral cannot be combined");
        std::process::exit(2);
    }
    if viewer_options.benchmark_ui && !ephemeral && home_path.is_none() {
        eprintln!("UI benchmarks require --ephemeral or a disposable --home profile");
        std::process::exit(2);
    }
    let ephemeral_home = if ephemeral {
        Some(tempfile::tempdir().expect("temporary home"))
    } else {
        None
    };
    let paths = if let Some(home) = home_path
        .as_deref()
        .or_else(|| ephemeral_home.as_ref().map(|home| home.path()))
    {
        StorePaths::for_home(home)
    } else {
        StorePaths::operating_system()
    };
    viewer_options.repository = ViewRepository::new(paths);
    if rows.is_none() && source_path.is_none() && view_path.is_none() && !no_restore {
        match ApplicationState::load(viewer_options.repository.clone()) {
            Ok(state) => {
                if let Some(view) = state
                    .history
                    .last_view_id
                    .as_ref()
                    .and_then(|id| state.views.iter().find(|view| &view.id == id))
                    .cloned()
                {
                    if view.database_view.is_none()
                        && !matches!(view.view, datadiver::domain::documents::ViewKind::Sql(_))
                    {
                        source_path = Some(PathBuf::from(&view.source_path));
                    }
                    viewer_options.load_view = Some(view);
                } else if let Some(path) = state.history.last_raw_source {
                    source_path = Some(PathBuf::from(path));
                } else if !state.history.about_shown {
                    // The first start, with nothing to reopen, shows the About page once.
                    viewer_options.about = true;
                    if let Err(error) = viewer_options
                        .repository
                        .update_history(|history| history.about_shown = true)
                    {
                        eprintln!("cannot save that the About page was shown: {error}");
                    }
                }
            }
            Err(error) => eprintln!("cannot restore previous session: {error}"),
        }
    }
    if let Some(path) = view_path {
        let definition = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|json| {
                serde_json::from_str::<DocumentDefinition>(&json).map_err(|error| error.to_string())
            })
            .and_then(|definition| {
                definition.validate()?;
                Ok(definition)
            });
        let mut definition = match definition {
            Ok(definition) => definition,
            Err(error) => {
                eprintln!("cannot open view {}: {error}", path.display());
                std::process::exit(2);
            }
        };
        if definition.database_view.is_none()
            && !matches!(
                definition.view,
                datadiver::domain::documents::ViewKind::Sql(_)
            )
        {
            let source = PathBuf::from(&definition.source_path);
            let source = if source.is_absolute() {
                source
            } else {
                path.parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(source)
            };
            let source = source.canonicalize().unwrap_or(source);
            definition.source_path = source.to_string_lossy().into_owned();
            source_path = Some(source);
        }
        viewer_options.load_view = Some(definition);
    }
    let store = match rows {
        Some(count) => DuckDbStore::synthetic(count),
        None => DuckDbStore::empty(),
    };
    let store = store.expect("initialize DuckDB data source");
    let source_path = source_path.map(|path| path.canonicalize().unwrap_or(path));
    viewer_options.load_path = source_path.clone();
    let snapshot = FrameSnapshot::new(store, 1);
    let source_path = source_path.map(|path| path.to_string_lossy().into_owned());
    let scripted = viewer_options.capture.is_some() || !viewer_options.show.is_empty() || viewer_options.benchmark_ui;
    if !ephemeral && !scripted {
        let repository = viewer_options.repository.clone();
        let name = instance_name(&repository.paths);
        datadiver::ui::platform::window::serve_instance::<DataFrameView>(&name, move |runtime, message| {
            let path = serde_json::from_str::<serde_json::Value>(&message)
                .ok()
                .and_then(|message| message.get("open")?.as_str().map(PathBuf::from));
            if let Some(path) = path {
                let mut options = ViewerOptions::default();
                options.repository = repository.clone();
                options.restore_window = false;
                options.load_path = Some(path);
                DataFrameView::open_another(runtime, options);
            }
        });
    }
    if !ephemeral && !scripted && home_path.is_none() {
        // A moved exe would leave double-clicked dashboards opening nothing.
        if let Ok(exe) = datadiver::infrastructure::file_association::current_exe()
            && let Err(error) = datadiver::infrastructure::file_association::follow_moved_exe(&exe)
        {
            eprintln!("cannot update the .ddash file association: {error}");
        }
    }
    datadiver::ui::platform::window::run::<DataFrameView>(move |runtime| {
        DataFrameView::open(runtime, snapshot, source_path, viewer_options);
    });
    datadiver::application::history::HistoryWriter::flush_profiles();
    datadiver::application::table_preferences::TablePreferencesWriter::flush_profiles();
    if let Err(error) =
        datadiver::infrastructure::source_database::SourceDatabase::checkpoint_profiles()
    {
        eprintln!("{error}");
    }
    datadiver::infrastructure::performance::flush();
    drop(ephemeral_home);
}
