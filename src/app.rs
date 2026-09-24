//! What is on screen and how keys move it. Git work happens in `worker`; this only arranges it.

use std::path::PathBuf;

use unicode_width::UnicodeWidthChar;

use crate::diff::{self, Kind, Row};
use crate::docs::Doc;
use crate::git::{self, FileChange, Tree};
use crate::pr::PrStatus;

#[derive(Clone)]
pub struct Group {
    pub root: PathBuf,
    pub tree: Result<Tree, String>,
    /// `None` until the first lookup answers.
    pub pr: Option<PrStatus>,
}

pub struct Snapshot {
    pub label: Option<String>,
    /// The worktree this tab's chat is in.
    pub current: Option<PathBuf>,
    pub groups: Vec<Group>,
    /// The workspace's design documents; `None` when vaglio runs outside herdr.
    pub docs: Option<Vec<Doc>>,
}

/// What the cursor is on: a file, or the design documents row pinned under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    /// Kept as root + path so a refresh that reorders the list does not move it.
    File(PathBuf, String),
    Docs,
}

/// One line of the file list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Header(usize),
    /// The pull request line under a header.
    Pr(usize),
    File(usize, usize),
    /// A worktree with nothing changed, or one git could not read.
    Empty(usize),
}

pub struct App {
    pub label: Option<String>,
    pub current: Option<PathBuf>,
    pub groups: Vec<Group>,
    pub items: Vec<Item>,
    pub selected: Option<Sel>,
    pub list_scroll: usize,
    pub view: Option<DiffView>,
    pub docs: Option<Vec<Doc>>,
    /// The design documents list, open, with the index of the selected one.
    pub docs_view: Option<usize>,
    pub loaded: bool,
    /// A short message in the footer, such as what `p` did.
    pub flash: Option<(String, std::time::Instant)>,
}

impl App {
    pub fn new() -> App {
        App {
            label: None,
            current: None,
            groups: Vec::new(),
            items: Vec::new(),
            selected: None,
            list_scroll: 0,
            view: None,
            docs: None,
            docs_view: None,
            loaded: false,
            flash: None,
        }
    }

    pub fn tree(&self, g: usize) -> Option<&Tree> {
        self.groups.get(g).and_then(|g| g.tree.as_ref().ok())
    }

    pub fn file(&self, item: Item) -> Option<(&Tree, &FileChange)> {
        let Item::File(g, f) = item else { return None };
        let tree = self.tree(g)?;
        Some((tree, tree.files.get(f)?))
    }

    pub fn apply(&mut self, snap: Snapshot) {
        self.loaded = true;
        self.label = snap.label;
        // The chat moved to another worktree: follow it there, as yazi does.
        let moved = snap.current.is_some() && snap.current != self.current;
        self.current = snap.current;
        self.groups = snap.groups;
        self.docs = snap.docs;
        match (&self.docs, self.docs_view) {
            (None, _) => self.docs_view = None,
            (Some(docs), Some(i)) => self.docs_view = Some(i.min(docs.len().saturating_sub(1))),
            _ => {}
        }
        self.items.clear();
        for (g, group) in self.groups.iter().enumerate() {
            self.items.push(Item::Header(g));
            if group.tree.is_ok() {
                self.items.push(Item::Pr(g));
            }
            match &group.tree {
                Ok(t) if !t.files.is_empty() => self.items.extend((0..t.files.len()).map(|f| Item::File(g, f))),
                _ => self.items.push(Item::Empty(g)),
            }
        }
        if moved {
            self.view = None;
            let root = self.current.clone();
            let first = self.files().find(|&i| self.file(i).is_some_and(|(t, _)| Some(&t.root) == root.as_ref()));
            self.selected = first.and_then(|i| self.key(i));
            self.list_scroll = 0;
        }
        if !self.selection_valid() {
            let first = self.files().next();
            self.selected = first.and_then(|i| self.key(i));
        }
        // The open file may have changed under Claude's hands: redraw it where it stands.
        if let Some(view) = &self.view {
            let current = self
                .groups
                .iter()
                .filter_map(|g| g.tree.as_ref().ok())
                .find(|t| t.root == view.root)
                .and_then(|t| t.files.iter().find(|f| f.path == view.file.path).map(|f| (t.clone(), f.clone())));
            if let Some((tree, file)) = current {
                if file != view.file || tree.merge_base != view.merge_base {
                    let (scroll, full, wrap) = (view.scroll, view.full, view.wrap);
                    let mut fresh = DiffView::open(&tree, &file, full, wrap);
                    fresh.scroll = scroll;
                    self.view = Some(fresh);
                }
            }
        }
    }

    fn key(&self, item: Item) -> Option<Sel> {
        self.file(item).map(|(t, f)| Sel::File(t.root.clone(), f.path.clone()))
    }

    fn files(&self) -> impl Iterator<Item = Item> + '_ {
        self.items.iter().copied().filter(|i| matches!(i, Item::File(..)))
    }

    /// Everything the cursor can stop on, top to bottom: the files, then the documents row.
    fn stops(&self) -> Vec<Sel> {
        let mut stops: Vec<Sel> = self.files().filter_map(|i| self.key(i)).collect();
        if self.docs.is_some() {
            stops.push(Sel::Docs);
        }
        stops
    }

    fn selection_valid(&self) -> bool {
        match &self.selected {
            Some(Sel::Docs) => self.docs.is_some(),
            Some(Sel::File(..)) => self.selected_index().is_some(),
            None => false,
        }
    }

    pub fn docs_selected(&self) -> bool {
        self.selected == Some(Sel::Docs)
    }

    /// The selected file's line in the list.
    pub fn selected_index(&self) -> Option<usize> {
        let Some(Sel::File(root, path)) = self.selected.as_ref() else { return None };
        self.items.iter().position(|&i| self.file(i).is_some_and(|(t, f)| &t.root == root && &f.path == path))
    }

    pub fn file_count(&self) -> usize {
        self.files().count()
    }

    /// Moves the selection by `delta` files, skipping headers; past the last file is the
    /// documents row.
    pub fn move_by(&mut self, delta: isize) {
        let stops = self.stops();
        if stops.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| stops.iter().position(|x| x == s)).unwrap_or(0);
        let next = (at as isize + delta).clamp(0, stops.len() as isize - 1) as usize;
        self.selected = Some(stops[next].clone());
    }

    pub fn select_edge(&mut self, last: bool) {
        let files: Vec<Item> = self.files().collect();
        let pick = if last { files.last() } else { files.first() };
        self.selected = pick.and_then(|&i| self.key(i));
    }

    pub fn open(&mut self) {
        if self.docs_selected() {
            self.docs_view = Some(0);
            return;
        }
        let Some(i) = self.selected_index() else { return };
        let Some((tree, file)) = self.file(self.items[i]) else { return };
        let (full, wrap) = self.view.as_ref().map_or((true, true), |v| (v.full, v.wrap));
        let mut view = DiffView::open(tree, file, full, wrap);
        view.jump_to_first_change = true;
        self.view = Some(view);
    }

    /// The group whose pull request `p` opens: the open diff's, or the selected file's.
    fn current_group(&self) -> Option<&Group> {
        let root = match (&self.view, &self.selected) {
            (Some(view), _) => &view.root,
            (None, Some(Sel::File(root, _))) => root,
            (None, _) => return self.groups.first(),
        };
        self.groups.iter().find(|g| &g.root == root)
    }

    pub fn open_pr(&mut self) {
        let msg = match self.current_group().map(|g| (g, g.pr.as_ref())) {
            None => "nessun worktree".to_string(),
            Some((_, None)) => "PR: sto ancora chiedendo".to_string(),
            Some((g, Some(status))) => match (status.web_url(), status) {
                (Some(url), PrStatus::Found(pr)) => {
                    if crate::pr::open(url) { format!("aperta #{}", pr.id) } else { format!("non riesco ad aprire {url}") }
                }
                (Some(url), _) => {
                    let branch = g.tree.as_ref().map(|t| t.branch.clone()).unwrap_or_default();
                    if crate::pr::open(url) { format!("nessuna PR per {branch}: aperto il form per crearla") } else { format!("non riesco ad aprire {url}") }
                }
                (None, PrStatus::NoCredentials) => "PR: manca il token Bitbucket (vedi README)".to_string(),
                (None, PrStatus::OnBase) => "sei sul branch di integrazione: niente PR da aprire".to_string(),
                (None, PrStatus::NoHost) => "PR: origin non è né Bitbucket né GitHub".to_string(),
                (None, PrStatus::Error(e)) => e.clone(),
                (None, _) => String::new(),
            },
        };
        self.flash = Some((msg, std::time::Instant::now()));
    }

    /// Copies the path of the selected (or open) file, relative to its worktree; in the
    /// documents list, the selected document's link or path.
    pub fn copy_path(&mut self) {
        let doc = self.docs_view.and_then(|i| self.docs.as_ref()?.get(i)).map(|d| d.target.clone());
        let path = match (&self.view, &self.selected) {
            _ if doc.is_some() => doc,
            (Some(view), _) => Some(view.file.path.clone()),
            (None, Some(Sel::File(_, path))) => Some(path.clone()),
            (None, _) => None,
        };
        let msg = match path {
            None => "nessun file selezionato".to_string(),
            Some(path) => match crate::clipboard::copy(&path) {
                true => format!("copiato {path}"),
                false => "non riesco a copiare negli appunti".to_string(),
            },
        };
        self.flash = Some((msg, std::time::Instant::now()));
    }

    /// Switches the open diff between the whole file and the hunks alone.
    pub fn toggle_full(&mut self) {
        let Some(view) = self.view.take() else { return };
        let tree = self.groups.iter().filter_map(|g| g.tree.as_ref().ok()).find(|t| t.root == view.root).cloned();
        let Some(tree) = tree else {
            self.view = Some(view);
            return;
        };
        let mut fresh = DiffView::open(&tree, &view.file, !view.full, view.wrap);
        fresh.jump_to_first_change = true;
        self.view = Some(fresh);
    }

    /// Opens the next or previous file without going back to the list.
    pub fn step_file(&mut self, delta: isize) {
        self.move_by(delta);
        if self.docs_selected() {
            self.move_by(-1);
        }
        self.open();
    }

    pub fn move_doc(&mut self, delta: isize) {
        let (Some(i), Some(docs)) = (self.docs_view, &self.docs) else { return };
        let last = docs.len().saturating_sub(1) as isize;
        self.docs_view = Some((i as isize + delta).clamp(0, last) as usize);
    }

    /// Opens the selected document in its own app.
    pub fn open_doc(&mut self) {
        let (Some(i), Some(docs)) = (self.docs_view, &self.docs) else { return };
        let Some(doc) = docs.get(i) else { return };
        let msg = match crate::docs::open(doc) {
            Ok(()) => format!("aperto {}", doc.title),
            Err(e) => e,
        };
        self.flash = Some((msg, std::time::Instant::now()));
    }
}

/// One wrapped screen line of a diff.
pub struct VisualRow {
    pub row: usize,
    pub first: bool,
    pub segs: Vec<diff::Seg>,
}

pub struct DiffView {
    pub root: PathBuf,
    pub base: String,
    pub merge_base: String,
    pub file: FileChange,
    pub rows: Vec<Row>,
    pub error: Option<String>,
    /// The whole file with its changes, or only the hunks.
    pub full: bool,
    pub wrap: bool,
    pub scroll: usize,
    pub jump_to_first_change: bool,
    pending_top_row: Option<usize>,
    pub number_width: usize,
    layout_key: Option<(u16, bool)>,
    pub visual: Vec<VisualRow>,
    /// First visual row of every block of added or deleted lines.
    pub changes: Vec<usize>,
}

impl DiffView {
    pub fn open(tree: &Tree, file: &FileChange, full: bool, wrap: bool) -> DiffView {
        let context = if full { None } else { Some(3) };
        let (rows, error) = match git::diff(tree, file, context) {
            Ok(raw) => {
                let mut rows = diff::build(&raw, &tree.root, &file.path);
                if full {
                    rows.retain(|r| r.kind != Kind::Hunk);
                }
                (rows, None)
            }
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let widest = rows.iter().filter_map(|r| r.old.max(r.new)).max().unwrap_or(0);
        DiffView {
            root: tree.root.clone(),
            base: tree.base.clone(),
            merge_base: tree.merge_base.clone(),
            file: file.clone(),
            rows,
            error,
            full,
            wrap,
            scroll: 0,
            jump_to_first_change: false,
            pending_top_row: None,
            number_width: widest.to_string().len().max(3),
            layout_key: None,
            visual: Vec::new(),
            changes: Vec::new(),
        }
    }

    pub fn gutter_width(&self) -> usize {
        self.number_width * 2 + 4
    }

    /// Wraps the rows to the pane width; cheap to call on every frame.
    pub fn layout(&mut self, width: u16, height: u16) {
        if self.layout_key != Some((width, self.wrap)) {
            self.layout_key = Some((width, self.wrap));
            let text_width = (width as usize).saturating_sub(self.gutter_width()).max(10);
            self.visual.clear();
            self.changes.clear();
            let mut prev_changed = false;
            for (i, row) in self.rows.iter().enumerate() {
                let changed = matches!(row.kind, Kind::Added | Kind::Deleted);
                if changed && !prev_changed {
                    self.changes.push(self.visual.len());
                }
                prev_changed = changed;
                let chunks = if self.wrap { wrap(&row.segs, text_width) } else { vec![row.segs.clone()] };
                for (k, segs) in chunks.into_iter().enumerate() {
                    self.visual.push(VisualRow { row: i, first: k == 0, segs });
                }
            }
        }
        if let Some(row) = self.pending_top_row.take() {
            self.scroll = self.visual.iter().position(|v| v.row == row).unwrap_or(0);
        }
        if self.jump_to_first_change {
            self.jump_to_first_change = false;
            if let Some(&first) = self.changes.first() {
                self.scroll = first.saturating_sub(3);
            }
        }
        self.clamp(height);
    }

    fn clamp(&mut self, height: u16) {
        let max = self.visual.len().saturating_sub(height as usize);
        self.scroll = self.scroll.min(max);
    }

    pub fn toggle_wrap(&mut self) {
        // Keep the same source row at the top across the re-wrap.
        let top = self.visual.get(self.scroll).map(|v| v.row);
        self.wrap = !self.wrap;
        self.layout_key = None;
        self.pending_top_row = top;
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).max(0) as usize;
    }

    /// Scrolls to the next (or previous) block of changes, a few lines of context above it.
    pub fn jump_change(&mut self, forward: bool) {
        let anchor = self.scroll + 3;
        let target = if forward {
            self.changes.iter().copied().find(|&c| c > anchor)
        } else {
            self.changes.iter().rev().copied().find(|&c| c < anchor)
        };
        if let Some(c) = target {
            self.scroll = c.saturating_sub(3);
        }
    }

    /// Which change block the top of the screen is in, for the title (1-based).
    pub fn change_position(&self) -> usize {
        self.changes.iter().filter(|&&c| c <= self.scroll + 3).count()
    }
}

fn wrap(segs: &[diff::Seg], width: usize) -> Vec<Vec<diff::Seg>> {
    let mut out = vec![Vec::new()];
    let mut used = 0;
    for seg in segs {
        let mut piece = String::new();
        for ch in seg.text.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width && used > 0 {
                if !piece.is_empty() {
                    out.last_mut().unwrap().push(diff::Seg { text: std::mem::take(&mut piece), ..seg.clone() });
                }
                out.push(Vec::new());
                used = 0;
            }
            piece.push(ch);
            used += w;
        }
        if !piece.is_empty() {
            out.last_mut().unwrap().push(diff::Seg { text: piece, ..seg.clone() });
        }
    }
    out
}
