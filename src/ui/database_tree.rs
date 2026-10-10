use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::application::state::ApplicationState;
use crate::domain::documents::{DatabaseView, DocumentDefinition, ViewKind};
use crate::infrastructure::catalog::scan_sources;
use crate::infrastructure::json_store::ViewRepository;
use crate::infrastructure::source_database::SourceDatabase;

/// What a leaf of the tree opens.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeTarget {
    View(Box<DocumentDefinition>),
    Source(PathBuf),
    Relation(DatabaseView),
}

pub enum DatabaseTreeEvent {
    Open {
        target: TreeTarget,
        new_window: bool,
        pivot_draft: bool,
    },
    FocusGrid,
    Refresh,
    Close,
}

/// Everything the tree lists, read on the background executor.
#[derive(Clone, Debug, Default)]
pub struct TreeData {
    pub views: Vec<DocumentDefinition>,
    pub relations: Vec<DatabaseView>,
    pub functions: Vec<DatabaseView>,
    /// Each watched folder with the source files found under it.
    pub folders: Vec<(PathBuf, Vec<PathBuf>)>,
    /// Recently opened sources outside every watched folder.
    pub recent: Vec<PathBuf>,
}

impl TreeData {
    pub fn load(repository: ViewRepository, database: &SourceDatabase) -> Result<Self, String> {
        let state = ApplicationState::load(repository).map_err(|error| error.to_string())?;
        let mut views = state.views;
        views.sort_by_key(|view| view.name.to_lowercase());
        let folders = state
            .settings
            .watched_folders
            .iter()
            .map(|folder| {
                let folder = PathBuf::from(folder);
                let root = folder.canonicalize().unwrap_or_else(|_| folder.clone());
                let files = scan_sources(std::slice::from_ref(&root))
                    .into_iter()
                    .filter_map(|file| file.strip_prefix(&root).ok().map(Path::to_path_buf))
                    .collect();
                (folder, files)
            })
            .collect::<Vec<_>>();
        let watched = state
            .settings
            .watched_folders
            .iter()
            .map(|folder| normalized(Path::new(folder)))
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let recent = state
            .history
            .recent_sources
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| {
                let key = normalized(path);
                !watched.iter().any(|folder| key.starts_with(folder)) && seen.insert(key)
            })
            .collect();
        Ok(Self {
            views,
            relations: database.relations()?,
            functions: database.metadata_functions()?,
            folders,
            recent,
        })
    }
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Top-level groups open on first show.
const OPEN_GROUPS: [&str; 3] = ["group:views", "group:databases", "group:sources"];

/// A folder under construction: subfolders by name, then leaves.
#[derive(Default)]
struct Folder {
    folders: BTreeMap<String, Folder>,
    leaves: Vec<(String, String, TreeTarget)>,
}

impl Folder {
    fn into_items(self, id: &str, builder: &mut Builder) -> Vec<TreeItem> {
        let mut items = Vec::new();
        for (name, folder) in self.folders {
            let id = format!("{id}/{name}");
            let children = folder.into_items(&id, builder);
            if let Some(item) = builder.folder(&id, &name, children) {
                items.push(item);
            }
        }
        for (leaf_id, label, target) in self.leaves {
            if let Some(item) = builder.leaf(leaf_id, label, target) {
                items.push(item);
            }
        }
        items
    }
}

/// Builds tree items, keeping only leaves matching the filter terms and
/// recording what each leaf opens.
struct Builder<'a> {
    terms: Vec<String>,
    expanded: &'a HashSet<String>,
    targets: HashMap<SharedString, TreeTarget>,
}

impl Builder<'_> {
    fn leaf(&mut self, id: String, label: String, target: TreeTarget) -> Option<TreeItem> {
        let searchable = match &target {
            TreeTarget::Source(path) => path.to_string_lossy().to_lowercase(),
            _ => label.to_lowercase(),
        };
        if !self.terms.iter().all(|term| searchable.contains(term)) {
            return None;
        }
        self.targets.insert(id.clone().into(), target);
        Some(TreeItem::new(id, label))
    }

    /// Empty folders are left out; while filtering every folder shows its matches.
    fn folder(&self, id: &str, label: &str, children: Vec<TreeItem>) -> Option<TreeItem> {
        (!children.is_empty()).then(|| {
            TreeItem::new(id.to_owned(), label.to_owned())
                .children(children)
                .expanded(!self.terms.is_empty() || self.expanded.contains(id))
        })
    }
}

/// The tree's items for `data`, plus the target of every leaf by id.
pub fn build_items(
    data: &TreeData,
    filter: &str,
    expanded: &HashSet<String>,
) -> (Vec<TreeItem>, HashMap<SharedString, TreeTarget>) {
    let mut builder = Builder {
        terms: filter
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        expanded,
        targets: HashMap::new(),
    };
    let mut groups = Vec::new();

    let mut views = BTreeMap::<u8, (&str, Vec<TreeItem>)>::new();
    for view in &data.views {
        let (order, kind) = match view.view {
            ViewKind::Table(_) => (0, "Table views"),
            ViewKind::Query(_) => (1, "Query views"),
            ViewKind::Sql(_) => (2, "SQL views"),
        };
        if let Some(item) = builder.leaf(
            format!("view:{}", view.id),
            view.name.clone(),
            TreeTarget::View(Box::new(view.clone())),
        ) {
            views
                .entry(order)
                .or_insert((kind, Vec::new()))
                .1
                .push(item);
        }
    }
    let views = views
        .into_values()
        .filter_map(|(kind, items)| builder.folder(&format!("views:{kind}"), kind, items))
        .collect();
    groups.extend(builder.folder("group:views", "Saved views", views));

    let mut databases = BTreeMap::<(bool, String), BTreeMap<String, Vec<TreeItem>>>::new();
    for relation in &data.relations {
        let catalog = relation.catalog.clone().unwrap_or_else(|| "profile".into());
        let id = format!("db:{}", relation.label());
        if let Some(item) = builder.leaf(
            id,
            relation.name.clone(),
            TreeTarget::Relation(relation.clone()),
        ) {
            databases
                .entry((relation.catalog.is_some(), catalog))
                .or_default()
                .entry(relation.schema.clone())
                .or_default()
                .push(item);
        }
    }
    let databases = databases
        .into_iter()
        .filter_map(|((_, catalog), schemas)| {
            let schemas = schemas
                .into_iter()
                .filter_map(|(schema, items)| {
                    builder.folder(&format!("db:{catalog}.{schema}"), &schema, items)
                })
                .collect();
            builder.folder(&format!("db:{catalog}"), &catalog, schemas)
        })
        .collect();
    groups.extend(builder.folder("group:databases", "Databases", databases));

    let mut sources = Vec::new();
    for (root, files) in &data.folders {
        let mut tree = Folder::default();
        for file in files {
            let mut folder = &mut tree;
            let parts = file
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let Some((name, parents)) = parts.split_last() else {
                continue;
            };
            for parent in parents {
                folder = folder.folders.entry(parent.clone()).or_default();
            }
            let path = root.join(file);
            folder.leaves.push((
                format!("src:{}", path.display()),
                name.clone(),
                TreeTarget::Source(path),
            ));
        }
        let id = format!("src:{}", root.display());
        let children = tree.into_items(&id, &mut builder);
        sources.extend(builder.folder(&id, &root.display().to_string(), children));
    }
    let recent = data
        .recent
        .iter()
        .filter_map(|path| {
            builder.leaf(
                format!("recent:{}", path.display()),
                file_name(path),
                TreeTarget::Source(path.clone()),
            )
        })
        .collect();
    sources.extend(builder.folder("src:recent", "Recent files", recent));
    groups.extend(builder.folder("group:sources", "Sources", sources));

    let functions = data
        .functions
        .iter()
        .filter_map(|function| {
            builder.leaf(
                format!("meta:{}", function.name),
                function.label(),
                TreeTarget::Relation(function.clone()),
            )
        })
        .collect();
    groups.extend(builder.folder("group:metadata", "DuckDB metadata", functions));

    (groups, builder.targets)
}

/// A left pane listing saved views, database relations, watched source
/// folders, and DuckDB's metadata functions.
pub struct DatabaseTree {
    focus_handle: FocusHandle,
    pub(crate) tree: Entity<TreeState>,
    pub(crate) filter: Entity<InputState>,
    data: TreeData,
    targets: HashMap<SharedString, TreeTarget>,
    expanded: HashSet<String>,
    loading: bool,
    error: Option<String>,
    revision: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DatabaseTreeEvent> for DatabaseTree {}

impl DatabaseTree {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let subscriptions = vec![
            cx.subscribe(&tree, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.to_string());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id.as_ref());
                }
            }),
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rebuild(cx);
                }
            }),
        ];
        Self {
            focus_handle: cx.focus_handle(),
            tree,
            filter,
            data: TreeData::default(),
            targets: HashMap::new(),
            expanded: OPEN_GROUPS.iter().map(|id| (*id).to_owned()).collect(),
            loading: false,
            error: None,
            revision: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Reads the tree's contents again, keeping folders open and the selection.
    pub fn refresh(
        &mut self,
        repository: ViewRepository,
        database: Arc<SourceDatabase>,
        cx: &mut Context<Self>,
    ) {
        self.revision += 1;
        let revision = self.revision;
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let data = cx
                .background_executor()
                .spawn(async move { TreeData::load(repository, &database) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.loading = false;
                match data {
                    Ok(data) => {
                        this.error = None;
                        this.data = data;
                        this.rebuild(cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(all(test, feature = "snapshots"))]
    pub(crate) fn relation_labels(&self) -> Vec<String> {
        let mut labels = self
            .targets
            .values()
            .filter_map(|target| match target {
                TreeTarget::Relation(relation) => Some(relation.label()),
                _ => None,
            })
            .collect::<Vec<_>>();
        labels.sort();
        labels
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value().to_string();
        let (items, targets) = build_items(&self.data, &filter, &self.expanded);
        self.targets = targets;
        // While filtering, a selected folder gives way to the first match so
        // Enter opens it.
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone())
            .filter(|id| filter.is_empty() || self.targets.contains_key(id));
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let index = selected
                .and_then(|id| tree.index_of(&id))
                .or_else(|| (!filter.is_empty()).then(|| first_leaf(tree)).flatten())
                .or_else(|| tree.entry(0).map(|_| 0));
            tree.set_selected_index(index, cx);
        });
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.tree.update(cx, |tree, cx| {
            if tree.selected_index().is_none() && tree.entry(0).is_some() {
                tree.set_selected_index(Some(0), cx);
            }
            tree.focus(window, cx);
        });
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    fn open_selected(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) -> bool {
        let Some(target) = self
            .tree
            .read(cx)
            .selected_item()
            .and_then(|item| self.targets.get(&item.id))
            .cloned()
        else {
            return false;
        };
        cx.emit(DatabaseTreeEvent::Open {
            target,
            new_window: modifiers.shift,
            pivot_draft: modifiers.control,
        });
        true
    }

    /// Enter opens a leaf or toggles a folder; Left on a closed entry moves to
    /// its parent, as in VS Code's explorer; typing starts a filter.
    fn tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let selected = self.tree.read(cx).selected_entry().cloned();
        match key {
            "enter" => {
                if !self.open_selected(modifiers, cx)
                    && let Some(entry) = selected.filter(|entry| entry.is_folder())
                {
                    self.toggle(entry.item().id.to_string(), !entry.is_expanded(), cx);
                }
                cx.stop_propagation();
            }
            "left" if !modifiers.modified() => {
                if let Some(entry) = selected.filter(|e| !(e.is_folder() && e.is_expanded())) {
                    let tree = self.tree.read(cx);
                    let index = tree.selected_index().unwrap_or(0);
                    let parent = (0..index)
                        .rev()
                        .find(|ix| tree.entry(*ix).is_some_and(|e| e.depth() < entry.depth()));
                    if let Some(parent) = parent {
                        self.tree.update(cx, |tree, cx| {
                            tree.set_selected_index(Some(parent), cx);
                            tree.scroll_to_item(parent, ScrollStrategy::Top);
                        });
                    }
                    cx.stop_propagation();
                }
            }
            "escape" => {
                cx.emit(DatabaseTreeEvent::FocusGrid);
                cx.stop_propagation();
            }
            _ => {
                let typed = event.keystroke.key_char.as_deref().filter(|text| {
                    !modifiers.control
                        && !modifiers.alt
                        && !modifiers.platform
                        && !text.trim().is_empty()
                });
                if let Some(text) = typed {
                    // Typing in the tree starts a new filter; keep typing in the box to refine it.
                    let text = text.to_owned();
                    self.filter.update(cx, |filter, cx| {
                        filter.set_value(text, window, cx);
                        filter.focus(window, cx);
                    });
                    self.rebuild(cx);
                    cx.stop_propagation();
                }
            }
        }
    }

    fn toggle(&mut self, id: String, expand: bool, cx: &mut Context<Self>) {
        if expand {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
        self.rebuild(cx);
    }

    fn filter_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.filter.read(cx).value().is_empty() {
                    self.focus(window, cx);
                } else {
                    self.filter
                        .update(cx, |filter, cx| filter.set_value("", window, cx));
                    self.rebuild(cx);
                }
                cx.stop_propagation();
            }
            "enter" => {
                self.open_selected(event.keystroke.modifiers, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

fn first_leaf(tree: &TreeState) -> Option<usize> {
    (0..)
        .map_while(|ix| tree.entry(ix).map(|entry| (ix, entry.is_folder())))
        .find(|(_, folder)| !folder)
        .map(|(ix, _)| ix)
}

impl Render for DatabaseTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let empty = self.targets.is_empty();
        let status = if self.loading && empty {
            Some("Loading…".to_owned())
        } else if let Some(error) = &self.error {
            Some(error.clone())
        } else if empty && !self.filter.read(cx).value().is_empty() {
            Some("No matches".to_owned())
        } else {
            None
        };
        div()
            .id("database-tree")
            .key_context("DatabaseTree")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .capture_key_down(cx.listener(Self::filter_key_down))
                            .on_action(cx.listener(|this, _: &MoveDown, window, cx| {
                                this.focus(window, cx);
                            }))
                            .child(Input::new(&self.filter).small().cleanable(true)),
                    )
                    .child(
                        Button::new("database-tree-refresh")
                            .ghost()
                            .small()
                            .label("⟳")
                            .tooltip("Refresh")
                            .loading(self.loading)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(DatabaseTreeEvent::Refresh);
                            })),
                    )
                    .child(
                        Button::new("database-tree-close")
                            .ghost()
                            .small()
                            .label("×")
                            .tooltip("Hide · Ctrl+B")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(DatabaseTreeEvent::Close);
                            })),
                    ),
            )
            .when_some(status, |pane, status| {
                pane.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(muted)
                        .child(status),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .capture_key_down(cx.listener(Self::tree_key_down))
                    .child(tree(&self.tree, {
                        let entity = cx.entity();
                        move |ix, entry, _, _, cx| {
                            let folder = entry.is_folder();
                            let chevron = match (folder, entry.is_expanded()) {
                                (true, true) => "▾",
                                (true, false) => "▸",
                                (false, _) => "",
                            };
                            let id = entry.item().id.clone();
                            let entity = entity.clone();
                            ListItem::new(ix)
                                .py(px(1.))
                                .pl(px(6. + entry.depth() as f32 * 12.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .min_w_0()
                                        .child(div().w(px(10.)).flex_shrink_0().child(chevron))
                                        .child(
                                            div()
                                                .truncate()
                                                .when(entry.depth() == 0, |label| {
                                                    label.font_weight(FontWeight::SEMIBOLD)
                                                })
                                                .child(entry.item().label.clone()),
                                        ),
                                )
                                .when(!folder, |item| {
                                    item.on_click(move |event, window, cx| {
                                        let modifiers = event.modifiers();
                                        entity.update(cx, |this, cx| {
                                            let index = this.tree.read(cx).index_of(&id);
                                            this.tree.update(cx, |tree, cx| {
                                                tree.set_selected_index(index, cx)
                                            });
                                            this.open_selected(modifiers, cx);
                                        });
                                        let _ = window;
                                    })
                                })
                                .when(folder, |item| item.text_color(cx.theme().foreground))
                        }
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DatabaseView, DocumentDefinition, HashMap, OPEN_GROUPS, PathBuf, SharedString, TreeData,
        TreeItem, TreeTarget, ViewKind, build_items,
    };
    use crate::domain::documents::{QueryConfig, RelationKind};

    fn data() -> TreeData {
        let mut sql = DocumentDefinition::draft_table("Monthly SQL".into(), String::new());
        sql.id = "sql-1".into();
        sql.view = ViewKind::Sql(Default::default());
        let mut table = DocumentDefinition::draft_table("Sales table".into(), "s.csv".into());
        table.id = "table-1".into();
        let mut query = table.clone();
        query.id = "query-1".into();
        query.name = "Sales by region".into();
        query.view = ViewKind::Query(QueryConfig::default());
        TreeData {
            views: vec![sql, table, query],
            relations: vec![
                DatabaseView::view("main", "notes"),
                DatabaseView {
                    catalog: Some("shop".into()),
                    kind: RelationKind::Table,
                    ..DatabaseView::view("main", "orders")
                },
            ],
            functions: vec![DatabaseView::function("duckdb_tables")],
            folders: vec![(
                PathBuf::from("C:/data"),
                vec![PathBuf::from("2024/q1.csv"), PathBuf::from("top.parquet")],
            )],
            recent: vec![PathBuf::from("D:/elsewhere/old.csv")],
        }
    }

    fn outline(items: &[TreeItem], depth: usize, lines: &mut Vec<String>) {
        for item in items {
            let marker = if item.is_folder() && !item.is_expanded() {
                "+"
            } else {
                ""
            };
            lines.push(format!("{}{marker}{}", "  ".repeat(depth), item.label));
            if item.is_expanded() {
                outline(&item.children, depth + 1, lines);
            }
        }
    }

    fn render(filter: &str, expanded: &[&str]) -> (Vec<String>, HashMap<SharedString, TreeTarget>) {
        let expanded = expanded.iter().map(|id| (*id).to_owned()).collect();
        let (items, targets) = build_items(&data(), filter, &expanded);
        let mut lines = Vec::new();
        outline(&items, 0, &mut lines);
        (lines, targets)
    }

    #[test]
    fn groups_nest_views_databases_folders_and_metadata() {
        let (lines, targets) = render("", &OPEN_GROUPS);
        assert_eq!(
            lines,
            [
                "Saved views",
                "  +Table views",
                "  +Query views",
                "  +SQL views",
                "Databases",
                "  +profile",
                "  +shop",
                "Sources",
                "  +C:/data",
                "  +Recent files",
                "+DuckDB metadata",
            ]
        );
        assert_eq!(targets.len(), 9);
        assert!(
            targets
                .values()
                .any(|t| *t == TreeTarget::Source(PathBuf::from("C:/data").join("2024/q1.csv")))
        );
        assert!(matches!(
            targets.get("meta:duckdb_tables"),
            Some(TreeTarget::Relation(view)) if view.kind == RelationKind::Function
        ));
    }

    #[test]
    fn filtering_keeps_matching_leaves_with_their_folders_open() {
        let (lines, _) = render("Q1", &[]);
        assert_eq!(lines, ["Sources", "  C:/data", "    2024", "      q1.csv"]);
        // Terms match anywhere in a source path, so a folder name finds its files.
        let (lines, _) = render("2024", &[]);
        assert!(lines.contains(&"      q1.csv".to_owned()));
        let (lines, _) = render("orders", &[]);
        assert_eq!(lines, ["Databases", "  shop", "    main", "      orders"]);
        let (lines, targets) = render("nothing matches", &[]);
        assert!(lines.is_empty() && targets.is_empty());
    }
}
