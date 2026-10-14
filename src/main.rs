#![windows_subsystem = "windows"]
use std::path::PathBuf;

use datadiver::application::state::ApplicationState;
use datadiver::domain::documents::DocumentDefinition;
use datadiver::infrastructure::duckdb_store::DuckDbStore;
use datadiver::infrastructure::json_store::{StorePaths, ViewRepository};
use datadiver::ui::dataframe_view::ViewerOptions;
use datadiver::{DataFrameView, FrameSnapshot};

fn main() {
    let mut args = std::env::args().skip(1);
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
                    if !matches!(
                        target,
                        "info"
                            | "editor:sql"
                            | "editor:startup-sql"
                            | "editor:sql-results"
                            | "editor:sql-completion"
                            | "editor:sql-error"
                            | "editor:sql-ask"
                            | "editor:sql-ask-log"
                            | "editor:sql-ask-mention"
                            | "theme"
                            | "theme-editor"
                            | "help"
                            | "space"
                            | "cache-drop"
                            | "save"
                            | "palette:copy-as"
                            | "palette:commands"
                            | "palette:fonts"
                            | "palette:editor-fonts"
                            | "palette:slicers"
                            | "palette:files"
                            | "palette:folders"
                            | "palette:recovered"
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
                            | "aggregation-menu"
                            | "form:aggregation"
                            | "form:condition"
                            | "editor:aggregation"
                            | "editor:aggregation-error"
                            | "keymap"
                            | "actions"
                            | "ai-providers"
                            | "action-menu"
                            | "slicer-search"
                            | "search-columns"
                            | "editor:aggregation-completion"
                            | "editor:group-completion"
                            | "database-tree"
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
                    "DataDiver\n  --rows N       open synthetic data\n  --file PATH    open a CSV (including .csv.gz), TSV, XLSX, or Parquet source\n  --view PATH    open a saved JSON view\n  --search TEXT  filter displayed rows\n  --slicer COLUMN  add a source-column slicer\n  --window WxH   set window size\n  --exit-after MS  close on a timer\n  --capture PATH  save a PNG (snapshots feature)"
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
    if viewer_options.capture.is_some() && !cfg!(feature = "snapshots") {
        eprintln!("--capture requires --features snapshots");
        std::process::exit(2);
    }
    gpui_kit::application()
        .with_assets(datadiver::ui::AppIcons)
        .run(move |cx| {
            cx.set_app_identity("DataDiver", "DataDiver");
            gpui_kit::init(cx);
            // Always show scrollbars, as the grid does, so long lists and
            // tables show there is more to scroll to.
            gpui_kit::component::Theme::set_scrollbar_mode(
                gpui_kit::component::scroll::ScrollbarMode::Always,
                cx,
            );
            cx.bind_keys([
                gpui_kit::KeyBinding::new(
                    "ctrl-enter",
                    gpui_kit::base::actions::Confirm { secondary: false },
                    Some("Command"),
                ),
                gpui_kit::KeyBinding::new(
                    "shift-enter",
                    gpui_kit::base::actions::Confirm { secondary: false },
                    Some("Command"),
                ),
                gpui_kit::KeyBinding::new(
                    "ctrl-shift-enter",
                    gpui_kit::base::actions::Confirm { secondary: false },
                    Some("Command"),
                ),
                gpui_kit::KeyBinding::new(
                    "ctrl-delete",
                    datadiver::ui::dataframe_view::ArchiveSelectedView,
                    Some("Command"),
                ),
            ]);
            DataFrameView::open(snapshot, source_path, viewer_options, cx);
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
