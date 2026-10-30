use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::application::state::ApplicationState;
use crate::domain::documents::{DatabaseView, DocumentDefinition, RelationKind, ViewKind};
use crate::infrastructure::catalog::scan_sources;
use crate::infrastructure::json_store::ViewRepository;
use crate::infrastructure::source_database::SourceDatabase;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Modifiers, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::tree::{Tree, TreeItem};
use crate::ui::widgets::{ButtonStyle, Fonts, Row, WidgetId, button, colors};


/// What a leaf of the tree opens.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeTarget {
    View(Box<DocumentDefinition>),
    /// A SQL source, opened as the table of rows it returns.
    SqlSource(Box<DocumentDefinition>),
    Source(PathBuf),
    Relation(DatabaseView),
}

/// What pressing Delete on a tree item removes.
#[derive(Clone, Debug, PartialEq)]
pub enum DeleteTarget {
    View(Box<DocumentDefinition>),
    Relation(DatabaseView),
    /// A database folder; `None` is the profile database.
    Database(Option<String>),
    RecentFile(PathBuf),
    WatchedFile(PathBuf),
    WatchedFolder(PathBuf),
    /// A file or subfolder of the watched folder `root`.
    InWatchedFolder { root: PathBuf, path: PathBuf },
}

pub enum DatabaseTreeEvent {
    Open {
        target: TreeTarget,
        new_window: bool,
        pivot_draft: bool,
    },
    Delete(DeleteTarget),
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
    /// Single files watched outside every watched folder.
    pub watched_files: Vec<PathBuf>,
    /// Recently opened sources that are not watched.
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
        let watched_files = state
            .settings
            .watched_files
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let mut seen = watched_files
            .iter()
            .map(|path| normalized(path))
            .collect::<HashSet<_>>();
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
            watched_files,
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
const OPEN_GROUPS: [&str; 4] = ["group:views", "group:scripts", "group:databases", "group:sources"];

/// A folder under construction: subfolders by name, then leaves.
#[derive(Default)]
struct Folder {
    folders: BTreeMap<String, Folder>,
    leaves: Vec<(String, String, TreeTarget)>,
}

impl Folder {
    /// `path` is this folder on disk, under the watched folder `root`.
    fn into_items(
        self,
        id: &str,
        root: &Path,
        path: &Path,
        builder: &mut Builder,
    ) -> Vec<TreeItem> {
        let mut items = Vec::new();
        for (name, folder) in self.folders {
            let id = format!("{id}/{name}");
            let path = path.join(&name);
            let children = folder.into_items(&id, root, &path, builder);
            if let Some(item) = builder.folder(&id, &name, children) {
                builder.deletable.insert(
                    id.into(),
                    DeleteTarget::InWatchedFolder {
                        root: root.to_path_buf(),
                        path,
                    },
                );
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
    targets: HashMap<String, TreeTarget>,
    deletable: HashMap<String, DeleteTarget>,
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
        let deletable = match (&target, id.split_once(':').map(|(kind, _)| kind)) {
            (TreeTarget::View(view) | TreeTarget::SqlSource(view), _) => Some(DeleteTarget::View(view.clone())),
            (TreeTarget::Relation(relation), _) if relation.kind != RelationKind::Function => {
                Some(DeleteTarget::Relation(relation.clone()))
            }
            (TreeTarget::Source(path), Some("recent")) => {
                Some(DeleteTarget::RecentFile(path.clone()))
            }
            (TreeTarget::Source(path), Some("watched")) => {
                Some(DeleteTarget::WatchedFile(path.clone()))
            }
            _ => None,
        };
        if let Some(deletable) = deletable {
            self.deletable.insert(id.clone().into(), deletable);
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
) -> (Vec<TreeItem>, HashMap<String, TreeTarget>) {
    let (items, targets, _) = build_tree(data, filter, expanded);
    (items, targets)
}

/// [`build_items`], plus what Delete removes for each item that has one.
#[allow(clippy::type_complexity)]
fn build_tree(
    data: &TreeData,
    filter: &str,
    expanded: &HashSet<String>,
) -> (
    Vec<TreeItem>,
    HashMap<String, TreeTarget>,
    HashMap<String, DeleteTarget>,
) {
    let mut builder = Builder {
        terms: filter
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        expanded,
        targets: HashMap::new(),
        deletable: HashMap::new(),
    };
    let mut groups = Vec::new();

    let mut views = BTreeMap::<u8, (&str, Vec<TreeItem>)>::new();
    for view in &data.views {
        let (order, kind) = match view.view {
            ViewKind::Table(_) => (0, "Table views"),
            ViewKind::Query(_) => (1, "Query views"),
            // SQL sources are listed with the other sources.
            ViewKind::Sql(_) => continue,
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

    let scripts = data
        .views
        .iter()
        .filter(|view| view.is_sql_script())
        .filter_map(|view| builder.leaf(format!("script:{}", view.id), view.name.clone(), TreeTarget::View(Box::new(view.clone()))))
        .collect();
    groups.extend(builder.folder("group:scripts", "SQL scripts", scripts));

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
        .filter_map(|((attached, catalog), schemas)| {
            let schemas = schemas
                .into_iter()
                .filter_map(|(schema, items)| {
                    let id = format!("db:{catalog}.{schema}");
                    builder.folder(&id, &schema, items)
                })
                .collect();
            let id = format!("db:{catalog}");
            let item = builder.folder(&id, &catalog, schemas)?;
            builder
                .deletable
                .insert(id.into(), DeleteTarget::Database(attached.then_some(catalog)));
            Some(item)
        })
        .collect();
    groups.extend(builder.folder("group:databases", "Databases", databases));

    let mut sources = Vec::new();
    let sql_sources = data
        .views
        .iter()
        .filter(|view| matches!(view.view, ViewKind::Sql(_)) && !view.is_sql_script())
        .filter_map(|view| {
            builder.leaf(
                format!("sql:{}", view.id),
                view.name.clone(),
                TreeTarget::SqlSource(Box::new(view.clone())),
            )
        })
        .collect();
    sources.extend(builder.folder("src:sql", "SQL sources", sql_sources));
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
            let id = format!("src:{}", path.display());
            builder.deletable.insert(
                id.clone().into(),
                DeleteTarget::InWatchedFolder {
                    root: root.clone(),
                    path: path.clone(),
                },
            );
            folder.leaves.push((id, name.clone(), TreeTarget::Source(path)));
        }
        let id = format!("src:{}", root.display());
        let children = tree.into_items(&id, root, root, &mut builder);
        if let Some(item) = builder.folder(&id, &root.display().to_string(), children) {
            builder
                .deletable
                .insert(id.into(), DeleteTarget::WatchedFolder(root.clone()));
            sources.push(item);
        }
    }
    let watched_files = data
        .watched_files
        .iter()
        .filter_map(|path| {
            builder.leaf(
                format!("watched:{}", path.display()),
                file_name(path),
                TreeTarget::Source(path.clone()),
            )
        })
        .collect();
    sources.extend(builder.folder("src:watched", "Watched files", watched_files));
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

    (groups, builder.targets, builder.deletable)
}

/// A left pane listing saved views, SQL scripts, database relations, SQL
/// sources and watched source folders, and DuckDB's metadata functions.
pub struct DatabaseTree {
    pub(crate) tree: Tree,
    pub(crate) filter: Editor,
    data: TreeData,
    targets: HashMap<String, TreeTarget>,
    deletable: HashMap<String, DeleteTarget>,
    expanded: HashSet<String>,
    loading: bool,
    error: Option<String>,
    revision: u64,
    /// A database to select once the next refresh lands.
    reveal: Option<String>,
    events: Vec<DatabaseTreeEvent>,
}

impl Default for DatabaseTree {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseTree {
    pub fn new() -> Self {
        let mut filter = Editor::single_line().with_placeholder("Filter");
        filter.framed = true;
        Self {
            tree: Tree::new(),
            filter,
            data: TreeData::default(),
            targets: HashMap::new(),
            deletable: HashMap::new(),
            expanded: OPEN_GROUPS.iter().map(|id| (*id).to_owned()).collect(),
            loading: false,
            error: None,
            revision: 0,
            reveal: None,
            events: Vec::new(),
        }
    }

    pub fn take_events(&mut self) -> Vec<DatabaseTreeEvent> {
        std::mem::take(&mut self.events)
    }

    /// Starts reading the tree's contents again; the host runs the read and
    /// hands the result to [`DatabaseTree::finish_refresh`].
    pub fn begin_refresh(&mut self) -> u64 {
        self.revision += 1;
        self.loading = true;
        self.revision
    }

    /// Shows a read, keeping folders open and the selection.
    pub fn finish_refresh(&mut self, revision: u64, data: Result<TreeData, String>) {
        if self.revision != revision {
            return;
        }
        self.loading = false;
        match data {
            Ok(data) => {
                self.error = None;
                self.data = data;
                self.rebuild();
                if let Some(catalog) = self.reveal.take() {
                    let id = format!("db:{catalog}");
                    if let Some(index) = self.tree.index_of(&id) {
                        self.tree.set_selected_index(Some(index));
                    }
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Opens the folder of an attached database and selects it after the
    /// next refresh, which must follow.
    pub fn reveal(&mut self, catalog: &str) {
        self.expanded.insert("group:databases".into());
        self.expanded.insert(format!("db:{catalog}"));
        self.reveal = Some(catalog.to_owned());
    }

    #[cfg(test)]
    pub(crate) fn selected_id(&self) -> Option<String> {
        self.tree.selected_entry().map(|entry| entry.id.clone())
    }

    /// Opens every folder and selects the item `id`.
    #[cfg(test)]
    pub(crate) fn select(&mut self, id: &str) {
        fn folders(items: &[TreeItem], ids: &mut HashSet<String>) {
            for item in items.iter().filter(|item| item.is_folder()) {
                ids.insert(item.id.clone());
                folders(&item.children, ids);
            }
        }
        let (items, _, _) = build_tree(&self.data, "", &self.expanded);
        folders(&items, &mut self.expanded);
        self.rebuild();
        let index = self.tree.index_of(id);
        assert!(index.is_some(), "no tree item {id}");
        self.tree.set_selected_index(index);
    }

    #[cfg(test)]
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

    fn rebuild(&mut self) {
        let filter = self.filter.text().to_owned();
        let (items, targets, deletable) = build_tree(&self.data, &filter, &self.expanded);
        self.targets = targets;
        self.deletable = deletable;
        // While filtering, a selected folder gives way to the first match so
        // Enter opens it.
        let selected = self
            .tree
            .selected_entry()
            .map(|entry| entry.id.clone())
            .filter(|id| filter.is_empty() || self.targets.contains_key(id));
        self.tree.set_items(items);
        let index = selected
            .and_then(|id| self.tree.index_of(&id))
            .or_else(|| (!filter.is_empty()).then(|| first_leaf(&self.tree)).flatten())
            .or_else(|| self.tree.entry(0).map(|_| 0));
        self.tree.set_selected_index(index);
    }

    pub fn focus(&mut self, focus: &mut Option<WidgetId>) {
        if self.tree.selected_index().is_none() && self.tree.entry(0).is_some() {
            self.tree.set_selected_index(Some(0));
        }
        *focus = Some(self.tree.id);
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        id == self.tree.id || id == self.filter.id
    }

    fn open_selected(&mut self, modifiers: Modifiers) -> bool {
        let Some(target) = self
            .tree
            .selected_entry()
            .and_then(|entry| self.targets.get(&entry.id))
            .cloned()
        else {
            return false;
        };
        self.events.push(DatabaseTreeEvent::Open {
            target,
            new_window: modifiers.shift,
            pivot_draft: modifiers.control,
        });
        true
    }

    fn toggle(&mut self, id: String, expand: bool) {
        if expand {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
        self.rebuild();
    }

    /// Enter opens a leaf or toggles a folder; Left on a closed entry moves to
    /// its parent, as in VS Code's explorer; typing starts a filter.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        if *focus == Some(self.filter.id) {
            return self.filter_key(event, focus);
        }
        if *focus != Some(self.tree.id) {
            return false;
        }
        let modifiers = event.modifiers;
        let selected = self.tree.selected_entry().cloned();
        match event.key.as_str() {
            "enter" => {
                if !self.open_selected(modifiers)
                    && let Some(entry) = selected.filter(|entry| entry.folder)
                {
                    self.toggle(entry.id, !entry.expanded);
                }
                true
            }
            "left" if !modifiers.modified() => {
                if let Some(entry) = selected.filter(|entry| !(entry.folder && entry.expanded)) {
                    let index = self.tree.selected_index().unwrap_or(0);
                    let parent = (0..index)
                        .rev()
                        .find(|ix| self.tree.entry(*ix).is_some_and(|e| e.depth < entry.depth));
                    if let Some(parent) = parent {
                        self.tree.set_selected_index(Some(parent));
                    }
                    return true;
                }
                let outcome = self.tree.key(event);
                if let Some((id, expand)) = outcome.toggle {
                    self.toggle(id, expand);
                }
                outcome.handled
            }
            "escape" => {
                self.events.push(DatabaseTreeEvent::FocusGrid);
                true
            }
            "delete" if !modifiers.modified() => {
                if let Some(target) = selected.and_then(|entry| self.deletable.get(&entry.id)).cloned() {
                    self.events.push(DatabaseTreeEvent::Delete(target));
                }
                true
            }
            _ => {
                let outcome = self.tree.key(event);
                if let Some((id, expand)) = outcome.toggle {
                    self.toggle(id, expand);
                }
                outcome.handled
            }
        }
    }

    /// Typing in the tree starts a new filter; typing in the box refines it.
    pub fn type_text(&mut self, typed: &str, focus: &mut Option<WidgetId>) -> bool {
        if *focus == Some(self.tree.id) {
            if typed.trim().is_empty() {
                return false;
            }
            self.filter.set_text(typed);
            *focus = Some(self.filter.id);
            self.rebuild();
            return true;
        }
        if *focus == Some(self.filter.id) {
            self.filter.type_text(typed);
            self.rebuild();
            return true;
        }
        false
    }

    fn filter_key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        match event.key.as_str() {
            "escape" => {
                if self.filter.text().is_empty() {
                    self.focus(focus);
                } else {
                    self.filter.set_text("");
                    self.rebuild();
                }
                true
            }
            "enter" => {
                self.open_selected(event.modifiers);
                true
            }
            "down" if !event.modifiers.modified() => {
                self.focus(focus);
                true
            }
            _ => {
                let outcome = self.filter.key(event);
                if outcome.changed {
                    self.rebuild();
                }
                outcome.handled
            }
        }
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: Modifiers, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Button(Button::TreeRefresh) => self.events.push(DatabaseTreeEvent::Refresh),
            Hit::Button(Button::TreeClose) => self.events.push(DatabaseTreeEvent::Close),
            Hit::Editor(id, part) if *id == self.filter.id => {
                self.filter.mouse_down(*part, pos, clicks, modifiers.shift);
                *focus = Some(self.filter.id);
            }
            Hit::Tree(id, part) if *id == self.tree.id => {
                let outcome = self.tree.mouse_down(*part, pos);
                *focus = Some(self.tree.id);
                if let Some((id, expand)) = outcome.toggle {
                    self.toggle(id, expand);
                }
                if outcome.clicked {
                    self.open_selected(modifiers);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        if self.filter.is_dragging() {
            self.filter.mouse_drag(pos);
        }
        self.tree.mouse_drag(pos);
    }

    pub fn mouse_up(&mut self) {
        self.filter.mouse_up();
        self.tree.mouse_up();
    }

    pub fn wheel(&mut self, delta: Point) -> bool {
        self.tree.wheel(delta)
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        canvas.fill(r, colors::SIDEBAR);
        canvas.fill(rect(r.right() - 1.0, r.y, 1.0, r.h), colors::BORDER);
        let small = &fonts.small;
        let header_height = Editor::line_box_height(small);
        let header = rect(r.x + 4.0, r.y + 4.0, r.w - 9.0, header_height);
        let mut items = Row::new(header, 4.0);
        let close = items.right(header_height);
        button(canvas, close, "×", small, ButtonStyle::Ghost, true, Hit::Button(Button::TreeClose));
        canvas.hit_with(close, Hit::Button(Button::TreeClose), crate::ui::platform::Cursor::Arrow, Some("Hide · Ctrl+B"));
        let refresh = items.right(header_height);
        button(
            canvas,
            refresh,
            if self.loading { "…" } else { "⟳" },
            small,
            ButtonStyle::Ghost,
            true,
            Hit::Button(Button::TreeRefresh),
        );
        canvas.hit_with(refresh, Hit::Button(Button::TreeRefresh), crate::ui::platform::Cursor::Arrow, Some("Refresh"));
        let filter = items.rest();
        let filter_id = self.filter.id;
        self.filter
            .paint(canvas, filter, small, focus == Some(filter_id), |part| Hit::Editor(filter_id, part));
        let mut top = header.bottom() + 4.0;
        let empty = self.targets.is_empty();
        let status = if self.loading && empty {
            Some("Loading…".to_owned())
        } else if let Some(error) = &self.error {
            Some(error.clone())
        } else if empty && !self.filter.text().is_empty() {
            Some("No matches".to_owned())
        } else {
            None
        };
        if let Some(status) = status {
            let line = text::line_height(small);
            canvas.text(rect(r.x + 8.0, top, r.w - 16.0, line + 4.0), &status, small, colors::MUTED, Align::Left);
            top += line + 8.0;
        }
        let body = rect(r.x, top, r.w - 1.0, (r.bottom() - top).max(0.0));
        let tree_id = self.tree.id;
        self.tree
            .paint(canvas, body, &fonts.ui, focus == Some(tree_id), |part| Hit::Tree(tree_id, part));
    }
}

fn first_leaf(tree: &Tree) -> Option<usize> {
    (0..)
        .map_while(|ix| tree.entry(ix).map(|entry| (ix, entry.folder)))
        .find(|(_, folder)| !folder)
        .map(|(ix, _)| ix)
}

#[cfg(test)]
mod tests {
    use super::{
        DatabaseView, DocumentDefinition, HashMap, OPEN_GROUPS, PathBuf, TreeData,
        TreeItem, TreeTarget, ViewKind, build_items,
    };
    use crate::domain::documents::{QueryConfig, RelationKind};

    fn data() -> TreeData {
        let mut sql = DocumentDefinition::draft_table("Monthly SQL".into(), String::new());
        sql.id = "sql-1".into();
        sql.view = ViewKind::Sql(Default::default());
        let mut script = DocumentDefinition::draft_table("Cleanup".into(), String::new());
        script.id = "script-1".into();
        script.view = ViewKind::Sql(crate::domain::documents::SqlConfig {
            script: true,
            ..Default::default()
        });
        let mut table = DocumentDefinition::draft_table("Sales table".into(), "s.csv".into());
        table.id = "table-1".into();
        let mut query = table.clone();
        query.id = "query-1".into();
        query.name = "Sales by region".into();
        query.view = ViewKind::Query(QueryConfig::default());
        TreeData {
            views: vec![sql, script, table, query],
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
            watched_files: vec![PathBuf::from("E:/kept/single.csv")],
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

    fn render(filter: &str, expanded: &[&str]) -> (Vec<String>, HashMap<String, TreeTarget>) {
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
                "SQL scripts",
                "  Cleanup",
                "Databases",
                "  +profile",
                "  +shop",
                "Sources",
                "  +SQL sources",
                "  +C:/data",
                "  +Watched files",
                "  +Recent files",
                "+DuckDB metadata",
            ]
        );
        assert_eq!(targets.len(), 11);
        assert!(
            targets
                .values()
                .any(|t| *t == TreeTarget::Source(PathBuf::from("C:/data").join("2024/q1.csv")))
        );
        assert!(matches!(
            targets.get("meta:duckdb_tables"),
            Some(TreeTarget::Relation(view)) if view.kind == RelationKind::Function
        ));
        assert!(matches!(targets.get("sql:sql-1"), Some(TreeTarget::SqlSource(view)) if view.name == "Monthly SQL"));
        assert!(matches!(targets.get("script:script-1"), Some(TreeTarget::View(view)) if view.is_sql_script()));
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
