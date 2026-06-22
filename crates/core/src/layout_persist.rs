//! Layout persistence: a serde view of the split tree plus per-leaf tab
//! metadata, with safe save / load and a never-crash fallback.
//!
//! The live [`Layout`](crate::layout::Layout) tracks structure and ids; it does
//! NOT own shells, working directories, or profiles (the engine is UI-free).
//! Persistence needs the *intent* of each cell — where to launch and which tab
//! was active — so a saved workspace can be reopened with **fresh** shells.
//! Live process state (scrollback, child PIDs, env) is deliberately NOT
//! persisted: reopening a workspace spawns new PTYs per leaf, matching the
//! "restore the shape, not the session" contract (tmux-server-grade live
//! restore is explicitly out of scope).
//!
//! # Format
//!
//! A [`LayoutSnapshot`] is JSON: a recursive [`NodeView`] tree (split axis +
//! flex-weighted children, or a leaf carrying [`LeafView`] tab metadata) plus
//! the focused-leaf ordinal. The format is structural only — there is no code,
//! no command string to execute, no path the loader will run. Loading reads
//! data, never instructions (STRIDE Tampering / Elevation review: the worst a
//! hostile file can do is request a `cwd` that does not exist, which the app
//! falls back from per [`LeafView::cwd`]).
//!
//! # Safety (pre-mortem #6)
//!
//! [`LayoutSnapshot::load`] never panics and never trusts the file blindly:
//! malformed JSON, a tree exceeding [`MAX_PANES`], an empty tree, a
//! non-finite/zero flex sum, or a structurally-broken split all resolve to a
//! single-pane fallback (logged at WARN), so a corrupt workspace file can
//! degrade the UI to one shell but can never crash it or wedge the grid.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::layout::{Axis, Child, Layout, LayoutNode, LeafId, TabGroup, MAX_PANES};

/// Per-leaf launch metadata captured for persistence. Mirrors one grid cell:
/// where its tabs should launch (`cwd`), which shell profile to use
/// (`profile`), and which tab was active.
///
/// `cwd` / `profile` are `Option`s: `None` means "use the app default" (the
/// configured shell, the process working directory). A persisted `cwd` that no
/// longer exists is the app's concern to fall back from — the format only
/// records the user's intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafView {
    /// Number of tabs this cell held. Always `>= 1` for a valid leaf; the
    /// loader clamps a zero to one so a restored cell always has a shell.
    pub tab_count: usize,
    /// Index (into the cell's tabs) of the tab that was visible. Clamped into
    /// range on load.
    pub active: usize,
    /// Working directory each fresh shell in this cell should launch in.
    /// `None` = the app default. Stored as a plain string (no path the loader
    /// executes — it is handed to the app, which may ignore a missing dir).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Named shell profile for this cell's tabs. `None` = the configured
    /// default shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Optional captured scrollback for this cell, as inert plain-text lines.
    ///
    /// **Default `None`** — scrollback is NOT persisted by the data model
    /// defaults. Capture is opt-in and wired by the app layer (security: a
    /// terminal's scrollback can contain echoed secrets, so persisting it is
    /// always an explicit user choice, not a default). When present, the lines
    /// are inert text the app replays above a fresh prompt; the loader never
    /// executes them. The app SHOULD cap captured lines to
    /// [`SCROLLBACK_MAX_LINES`] at capture time; the loader also clamps on
    /// load as defense-in-depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<Vec<String>>,
}

/// Default cap on persisted scrollback lines per leaf.
///
/// The app SHOULD enforce this at *capture* time (so a hostile/huge terminal
/// cannot bloat the state file); [`LeafView::normalize`] also truncates an
/// over-cap `scrollback` on *load* as a second line of defense, mirroring the
/// `MAX_PANES` cap discipline. Chosen to match the research dossier's suggested
/// `scrollback_max_lines = 2000`.
pub const SCROLLBACK_MAX_LINES: usize = 2000;

impl LeafView {
    /// A single-tab leaf with no cwd/profile override (the default cell).
    #[must_use]
    pub fn single() -> Self {
        LeafView {
            tab_count: 1,
            active: 0,
            cwd: None,
            profile: None,
            scrollback: None,
        }
    }

    /// A leaf carrying explicit launch intent (no scrollback).
    ///
    /// Scrollback defaults to `None`; use [`LeafView::with_scrollback`] to
    /// attach captured lines when the app's opt-in scrollback flag is set.
    #[must_use]
    pub fn new(
        tab_count: usize,
        active: usize,
        cwd: Option<String>,
        profile: Option<String>,
    ) -> Self {
        LeafView {
            tab_count,
            active,
            cwd,
            profile,
            scrollback: None,
        }
    }

    /// Attach (or clear) captured scrollback lines, returning the updated leaf.
    ///
    /// The app calls this only when its opt-in scrollback persistence is
    /// enabled. Lines are truncated to [`SCROLLBACK_MAX_LINES`] (keeping the
    /// most-recent tail) so a single over-large terminal cannot bloat the
    /// state file.
    #[must_use]
    pub fn with_scrollback(mut self, scrollback: Option<Vec<String>>) -> Self {
        self.scrollback = scrollback.map(cap_scrollback);
        self
    }
}

/// Truncate `lines` to the most-recent [`SCROLLBACK_MAX_LINES`] (the tail is
/// the useful end of scrollback). A no-op when already within the cap.
fn cap_scrollback(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() > SCROLLBACK_MAX_LINES {
        let drop = lines.len() - SCROLLBACK_MAX_LINES;
        lines.drain(0..drop);
    }
    lines
}

/// A node in the persisted tree: either a split (axis + weighted children) or a
/// leaf carrying its [`LeafView`] metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeView {
    /// An internal split.
    Split {
        /// Split orientation.
        axis: Axis,
        /// Weighted children (the loader requires `>= 2`).
        children: Vec<ChildView>,
    },
    /// A terminal cell with its launch metadata.
    Leaf {
        /// The cell's tab/cwd/profile intent.
        view: LeafView,
    },
}

/// A weighted child inside a [`NodeView::Split`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildView {
    /// Fractional share of the parent's extent along the split axis.
    pub flex: f32,
    /// The subtree under this child.
    pub node: NodeView,
}

/// A complete persisted layout: the structural tree plus the focused leaf's
/// ordinal (its 0-based position in left-to-right DFS leaf order).
///
/// Carries a `version` so a future format change can be detected and migrated
/// rather than mis-parsed. Unknown future versions are rejected on load (→
/// single-pane fallback), never silently mis-read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    /// Format version. Current is [`LayoutSnapshot::VERSION`].
    pub version: u32,
    /// Root of the persisted tree.
    pub root: NodeView,
    /// 0-based DFS ordinal of the focused leaf. Clamped on load.
    #[serde(default)]
    pub focused_ordinal: usize,
}

/// Why a saved layout could not be restored. Every variant degrades the UI to
/// a single pane; none ever panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The file was missing or unreadable.
    Io(String),
    /// The JSON did not parse.
    Parse(String),
    /// The file's format version is not understood.
    UnsupportedVersion(u32),
    /// The tree was empty, exceeded [`MAX_PANES`], or was otherwise invalid.
    Invalid(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "layout file io error: {e}"),
            LoadError::Parse(e) => write!(f, "layout file parse error: {e}"),
            LoadError::UnsupportedVersion(v) => write!(f, "unsupported layout version {v}"),
            LoadError::Invalid(m) => write!(f, "invalid layout: {m}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The result of loading a workspace: a reconstructed [`Layout`] (fresh ids,
/// no live sessions) plus the per-leaf [`LeafView`] metadata in DFS order, so
/// the app can spawn one fresh shell per leaf with the right cwd/profile.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredLayout {
    /// The structural layout, ready to drive the renderer.
    pub layout: Layout,
    /// Per-leaf launch metadata, parallel to `layout.leaves()` (DFS order).
    pub leaves: Vec<(LeafId, LeafView)>,
}

impl RestoredLayout {
    /// The trivial single-pane workspace (used as the safe fallback).
    #[must_use]
    pub fn single_pane() -> Self {
        let layout = Layout::new();
        let focused = layout.focused;
        RestoredLayout {
            layout,
            leaves: vec![(focused, LeafView::single())],
        }
    }
}

impl LayoutSnapshot {
    /// Current persistence format version.
    pub const VERSION: u32 = 1;

    /// Build a snapshot from a live [`Layout`] and a per-leaf metadata lookup.
    ///
    /// `meta` is called for each [`LeafId`] in the tree; return the cell's
    /// [`LeafView`] (tab count, active index, cwd, profile). The app supplies
    /// this from its session store. The focused leaf's DFS ordinal is recorded.
    pub fn capture(layout: &Layout, mut meta: impl FnMut(LeafId) -> LeafView) -> Self {
        let leaves = layout.leaves();
        let focused_ordinal = leaves
            .iter()
            .position(|&id| id == layout.focused)
            .unwrap_or(0);
        LayoutSnapshot {
            version: Self::VERSION,
            root: node_view(&layout.root, &mut meta),
            focused_ordinal,
        }
    }

    /// Serialize to pretty JSON. Deterministic for a fixed snapshot (no maps,
    /// no time, no RNG) so saved files are byte-stable.
    pub fn to_json(&self) -> Result<String, LoadError> {
        serde_json::to_string_pretty(self).map_err(|e| LoadError::Parse(e.to_string()))
    }

    /// Parse from JSON, validating structure and the [`MAX_PANES`] cap. Does
    /// NOT reconstruct a [`Layout`] — use [`LayoutSnapshot::restore`] for that.
    pub fn from_json(src: &str) -> Result<Self, LoadError> {
        let snap: LayoutSnapshot =
            serde_json::from_str(src).map_err(|e| LoadError::Parse(e.to_string()))?;
        snap.validate()?;
        Ok(snap)
    }

    /// Validate the snapshot: known version, non-empty, within [`MAX_PANES`],
    /// every split has `>= 2` children, every flex is finite and non-negative.
    pub fn validate(&self) -> Result<(), LoadError> {
        if self.version != Self::VERSION {
            return Err(LoadError::UnsupportedVersion(self.version));
        }
        let n = count_leaves(&self.root)?;
        if n == 0 {
            return Err(LoadError::Invalid("layout has no panes".into()));
        }
        if n > MAX_PANES {
            return Err(LoadError::Invalid(format!(
                "layout has {n} panes, exceeds MAX_PANES ({MAX_PANES})"
            )));
        }
        Ok(())
    }

    /// Reconstruct a live [`Layout`] (fresh, deterministic ids) plus the DFS
    /// per-leaf metadata. Fails (→ caller falls back) on an invalid snapshot.
    pub fn restore(&self) -> Result<RestoredLayout, LoadError> {
        self.validate()?;
        let mut next_id = 0u64;
        let mut leaves: Vec<(LeafId, LeafView)> = Vec::new();
        let root = build_node(&self.root, &mut next_id, &mut leaves);
        if leaves.is_empty() {
            return Err(LoadError::Invalid("layout produced no leaves".into()));
        }
        let focused = leaves
            .get(self.focused_ordinal.min(leaves.len() - 1))
            .map(|(id, _)| *id)
            .unwrap_or(leaves[0].0);
        let layout = Layout {
            root,
            focused,
            zoomed: None,
            next_id,
        };
        Ok(RestoredLayout { layout, leaves })
    }

    /// Write the snapshot to `path` as pretty JSON, creating parent dirs.
    ///
    /// Uses the crash-safe, owner-only [`crate::atomic_write::atomic_write_owner_only`]
    /// helper (temp + fsync + rename, 0600 / owner-ACL) — the same write the
    /// sibling [`WorkspaceSnapshot::save_atomic`] uses, because a snapshot records
    /// each pane's `cwd` (a path that can reveal usernames / project names), which
    /// must not be written world-readable or left half-written on a crash.
    pub fn save(&self, path: &Path) -> Result<(), LoadError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| LoadError::Io(e.to_string()))?;
            }
        }
        let json = self.to_json()?;
        crate::atomic_write::atomic_write_owner_only(path, json.as_bytes())
            .map_err(|e| LoadError::Io(e.to_string()))
    }
}

/// Save a live [`Layout`] to `path`, capturing per-leaf metadata via `meta`.
/// A convenience wrapper over [`LayoutSnapshot::capture`] + `save`.
pub fn save(
    layout: &Layout,
    path: &Path,
    meta: impl FnMut(LeafId) -> LeafView,
) -> Result<(), LoadError> {
    LayoutSnapshot::capture(layout, meta).save(path)
}

/// Load and reconstruct a layout from `path`. **Never panics**: a missing,
/// unreadable, malformed, over-cap, or otherwise-invalid file logs a WARN and
/// returns the single-pane fallback (pre-mortem #6). The `Result` form is
/// available via [`load_strict`] when the caller wants to distinguish a real
/// failure from an absent file.
#[must_use]
pub fn load(path: &Path) -> RestoredLayout {
    match load_strict(path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("layout restore failed ({e}); falling back to a single pane");
            RestoredLayout::single_pane()
        }
    }
}

/// Load and reconstruct a layout from `path`, returning the [`LoadError`] on
/// failure instead of the silent fallback. Used by tests and by callers that
/// want to surface a corrupt-file message.
pub fn load_strict(path: &Path) -> Result<RestoredLayout, LoadError> {
    let src = std::fs::read_to_string(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let snap = LayoutSnapshot::from_json(&src)?;
    snap.restore()
}

// --- multi-tab workspace ---------------------------------------------------

/// A versioned, multi-window-tab workspace: an ordered list of per-tab
/// [`LayoutSnapshot`]s plus the active tab index.
///
/// The original on-disk format is a single bare [`LayoutSnapshot`] (one window
/// tab). This wrapper adds the "1–6+ terminals across several window tabs"
/// case while staying **backward compatible**: a v1 single-`LayoutSnapshot`
/// file is migrated forward into a 1-tab `WorkspaceSnapshot` on load
/// ([`WorkspaceSnapshot::from_json`]). The wrapper carries its own
/// [`WorkspaceSnapshot::VERSION`] so a future format change is detected, never
/// mis-parsed.
///
/// Like [`LayoutSnapshot`], the format is structural data only — the loader
/// reads it, never executes it — and loading **never panics**: a malformed,
/// empty, or unknown-version file degrades to a single-default-tab workspace
/// ([`WorkspaceSnapshot::load`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Workspace format version. Current is [`WorkspaceSnapshot::VERSION`].
    pub version: u32,
    /// One [`LayoutSnapshot`] per window tab, in tab order. Always `>= 1` after
    /// a successful load (an empty list is coerced to a single default tab).
    pub tabs: Vec<LayoutSnapshot>,
    /// Index of the active window tab. Clamped into range on load.
    #[serde(default)]
    pub active: usize,
}

impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self::single_tab()
    }
}

impl WorkspaceSnapshot {
    /// Current workspace (multi-tab) format version. Distinct from
    /// [`LayoutSnapshot::VERSION`]: the wrapper and the per-tab payload version
    /// independently. v2 is the first version that carries the multi-tab
    /// wrapper; v1 is the implicit "bare single [`LayoutSnapshot`]" format that
    /// migrates forward.
    pub const VERSION: u32 = 2;

    /// A workspace holding a single trivial single-pane tab (the safe fallback,
    /// used when no file exists or a file is corrupt).
    #[must_use]
    pub fn single_tab() -> Self {
        let tab = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Leaf {
                view: LeafView::single(),
            },
            focused_ordinal: 0,
        };
        WorkspaceSnapshot {
            version: Self::VERSION,
            tabs: vec![tab],
            active: 0,
        }
    }

    /// Build a workspace from an ordered list of per-tab [`LayoutSnapshot`]s and
    /// the active tab index. An empty `tabs` list is coerced to a single
    /// default tab; `active` is clamped into range.
    #[must_use]
    pub fn from_tabs(tabs: Vec<LayoutSnapshot>, active: usize) -> Self {
        if tabs.is_empty() {
            return Self::single_tab();
        }
        let active = active.min(tabs.len() - 1);
        WorkspaceSnapshot {
            version: Self::VERSION,
            tabs,
            active,
        }
    }

    /// Serialize to pretty JSON. Deterministic / byte-stable for a fixed
    /// snapshot (no maps, no time, no RNG).
    pub fn to_json(&self) -> Result<String, LoadError> {
        serde_json::to_string_pretty(self).map_err(|e| LoadError::Parse(e.to_string()))
    }

    /// Parse a workspace from JSON, with a **v1 → v2 migration shim**:
    ///
    /// 1. Try to parse the multi-tab [`WorkspaceSnapshot`] wrapper. If it parses
    ///    AND its `version` is understood, validate every tab and return it.
    /// 2. Otherwise, fall back to parsing a bare [`LayoutSnapshot`] (the v1
    ///    single-tab format) and wrap it into a 1-tab workspace.
    ///
    /// A future unknown wrapper version (not v2, and not a valid bare
    /// `LayoutSnapshot`) is rejected with [`LoadError::UnsupportedVersion`] so
    /// the caller falls back rather than mis-reading.
    pub fn from_json(src: &str) -> Result<Self, LoadError> {
        // Path 1: the multi-tab wrapper.
        if let Ok(ws) = serde_json::from_str::<WorkspaceSnapshot>(src) {
            // Only treat it as a wrapper if it is actually our version; a bare
            // LayoutSnapshot can also deserialize into this struct's shape only
            // by coincidence, so the version guard plus the `tabs` presence is
            // what disambiguates.
            if ws.version == Self::VERSION {
                let mut ws = ws;
                ws.validate_and_normalize()?;
                return Ok(ws);
            }
            // A wrapper-shaped value with an unknown version is a real, future
            // workspace format we do not understand — reject (→ fallback).
            if !ws.tabs.is_empty() {
                return Err(LoadError::UnsupportedVersion(ws.version));
            }
        }

        // Path 2: v1 migration — a bare single LayoutSnapshot.
        let tab = LayoutSnapshot::from_json(src)?;
        Ok(WorkspaceSnapshot::from_tabs(vec![tab], 0))
    }

    /// Validate every tab (known version, within [`MAX_PANES`], well-formed
    /// splits) and clamp `active` into range. Coerces an empty `tabs` list to a
    /// single default tab so a restored workspace always has a visible tab.
    fn validate_and_normalize(&mut self) -> Result<(), LoadError> {
        if self.tabs.is_empty() {
            *self = Self::single_tab();
            return Ok(());
        }
        for tab in &self.tabs {
            tab.validate()?;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        Ok(())
    }

    /// Reconstruct one [`RestoredLayout`] per tab (fresh ids, no live sessions),
    /// in tab order, plus the clamped active tab index. The app spawns one fresh
    /// shell per leaf per restored tab.
    ///
    /// # Errors
    ///
    /// Returns the first tab's [`LoadError`] if any tab is structurally invalid.
    pub fn restore_all(&self) -> Result<RestoredWorkspace, LoadError> {
        if self.tabs.is_empty() {
            return Ok(RestoredWorkspace::single_tab());
        }
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            tabs.push(tab.restore()?);
        }
        let active = self.active.min(tabs.len() - 1);
        Ok(RestoredWorkspace { tabs, active })
    }

    /// Atomically write the workspace to `path` as pretty JSON via the
    /// crash-safe [`crate::atomic_write::atomic_write_owner_only`] helper
    /// (temp-file + rename), creating parent dirs. A crash mid-save leaves the
    /// previous file intact, never a torn one.
    ///
    /// The file is tightened to **owner-only** access because each leaf records
    /// its `cwd` (a real filesystem path that reveals usernames and project
    /// names); other local accounts must not be able to read it. This mirrors
    /// the config-file tightening (`restrict_to_owner`).
    ///
    /// # Errors
    ///
    /// Returns [`LoadError::Parse`] if serialization fails or [`LoadError::Io`]
    /// if the write/rename fails.
    pub fn save_atomic(&self, path: &Path) -> Result<(), LoadError> {
        let json = self.to_json()?;
        crate::atomic_write::atomic_write_owner_only(path, json.as_bytes())
            .map_err(|e| LoadError::Io(e.to_string()))
    }

    /// Load a workspace from `path`. **Never panics**: a missing, unreadable,
    /// malformed, over-cap, or unknown-version file logs a WARN and returns the
    /// single-default-tab fallback. A v1 single-`LayoutSnapshot` file is
    /// migrated forward (see [`WorkspaceSnapshot::from_json`]). Use
    /// [`WorkspaceSnapshot::load_strict`] to surface the error instead.
    #[must_use]
    pub fn load(path: &Path) -> WorkspaceSnapshot {
        match Self::load_strict(path) {
            Ok(ws) => ws,
            Err(e) => {
                tracing::warn!(
                    "workspace restore failed ({e}); falling back to a single default tab"
                );
                Self::single_tab()
            }
        }
    }

    /// Load a workspace from `path`, returning the [`LoadError`] on failure
    /// instead of the silent fallback. Used by tests and callers that want to
    /// surface a corrupt-file message.
    pub fn load_strict(path: &Path) -> Result<WorkspaceSnapshot, LoadError> {
        let src = std::fs::read_to_string(path).map_err(|e| LoadError::Io(e.to_string()))?;
        Self::from_json(&src)
    }

    /// Load a workspace from `path`, distinguishing the three load states the
    /// UI must treat differently:
    ///
    /// * [`RestoreOutcome::Absent`] — no file exists. This is the **normal**
    ///   first-launch / never-saved case; the app starts fresh **silently**.
    /// * [`RestoreOutcome::Corrupt`] — a file exists but could not be parsed or
    ///   failed semantic validation (truncated/malformed JSON, an unknown
    ///   format version, an out-of-range index, an over-cap or empty tree). The
    ///   app starts fresh AND must surface a "session restore failed — started
    ///   fresh" notice so the user knows their saved layout was dropped, not
    ///   lost to a bug. The `reason` is a human-readable [`LoadError`] message.
    /// * [`RestoreOutcome::Restored`] — a valid file fully reconstructed.
    ///
    /// Unlike [`WorkspaceSnapshot::load`] (which collapses absent and corrupt
    /// into the same silent single-tab fallback), this never panics and never
    /// loses the absent/corrupt distinction. The caller decides whether to
    /// surface a notice and whether to quarantine via [`quarantine_corrupt`].
    #[must_use]
    pub fn load_outcome(path: &Path) -> RestoreOutcome {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            // A missing file is the normal "nothing saved yet" case — Absent,
            // never Corrupt, so a fresh start is silent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RestoreOutcome::Absent,
            // Any other IO failure (permission, unreadable) is a real failure
            // the user should be told about — treat as Corrupt with the reason.
            Err(e) => {
                return RestoreOutcome::Corrupt {
                    reason: LoadError::Io(e.to_string()).to_string(),
                }
            }
        };
        match Self::from_json(&src).and_then(|ws| ws.restore_all()) {
            Ok(restored) => RestoreOutcome::Restored(restored),
            Err(e) => RestoreOutcome::Corrupt {
                reason: e.to_string(),
            },
        }
    }
}

/// The outcome of attempting to restore a persisted workspace, distinguishing
/// the three states the UI treats differently (see
/// [`WorkspaceSnapshot::load_outcome`]). This is the **total** restore surface:
/// every persisted file resolves to exactly one of these variants, and none of
/// them ever panics.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreOutcome {
    /// A valid file fully reconstructed into a live workspace.
    Restored(RestoredWorkspace),
    /// A file existed but was corrupt (unparseable, semantically invalid, or
    /// unreadable). The app should start fresh AND surface a "restore failed"
    /// notice. `reason` is a human-readable [`LoadError`] description.
    Corrupt {
        /// Human-readable description of why the restore failed.
        reason: String,
    },
    /// No file existed — the normal first-launch / never-saved case. The app
    /// starts fresh **silently** (no notice).
    Absent,
}

impl RestoreOutcome {
    /// `true` for the [`RestoreOutcome::Corrupt`] variant — the only state that
    /// warrants a user-facing "restore failed" notice.
    #[must_use]
    pub fn is_corrupt(&self) -> bool {
        matches!(self, RestoreOutcome::Corrupt { .. })
    }

    /// The corruption reason, or `None` for the non-corrupt variants.
    #[must_use]
    pub fn corrupt_reason(&self) -> Option<&str> {
        match self {
            RestoreOutcome::Corrupt { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// Quarantine a corrupt workspace file by renaming it aside to
/// `<file>.corrupt-<hash8>` so it is **not silently re-loaded** on the next
/// launch (which would re-surface the same notice forever) but is **preserved
/// for debugging** (never deleted — user data is never destroyed). The `<hash8>`
/// suffix is the first 8 hex chars of a stable FNV-1a hash of the file's bytes,
/// so re-running on an unchanged file targets the same quarantine name (no
/// proliferation) while two distinct corrupt files get distinct names.
///
/// Returns the quarantine path on success. A best-effort operation: an IO
/// failure (e.g. the file vanished, or the destination is unwritable) returns
/// the [`LoadError::Io`] so the caller may log it, but is never fatal — the app
/// has already fallen back to a fresh session by the time this is called.
pub fn quarantine_corrupt(path: &Path) -> Result<std::path::PathBuf, LoadError> {
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let hash = fnv1a_hash8(&bytes);
    // Append a `.corrupt-<hash8>` suffix to the existing file name, keeping the
    // original extension(s) intact in the stem so the provenance is obvious.
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".corrupt-{hash}"));
    let dest = path.with_file_name(name);
    std::fs::rename(path, &dest).map_err(|e| LoadError::Io(e.to_string()))?;
    Ok(dest)
}

/// First 8 hex chars of a 64-bit FNV-1a hash of `bytes`. Stable, dependency-free
/// (no RNG, no external crate), and good enough to disambiguate distinct corrupt
/// files in a quarantine name — not a security primitive.
fn fnv1a_hash8(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{:08x}", (h >> 32) as u32)
}

/// The result of restoring a [`WorkspaceSnapshot`]: one [`RestoredLayout`] per
/// window tab (in tab order) plus the active tab index.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredWorkspace {
    /// One reconstructed layout per window tab, in tab order.
    pub tabs: Vec<RestoredLayout>,
    /// Index of the active window tab (always in range).
    pub active: usize,
}

impl RestoredWorkspace {
    /// The trivial single-tab, single-pane workspace (the safe fallback).
    #[must_use]
    pub fn single_tab() -> Self {
        RestoredWorkspace {
            tabs: vec![RestoredLayout::single_pane()],
            active: 0,
        }
    }
}

// --- internal recursion helpers -------------------------------------------

/// Recursively build a [`NodeView`] from a live [`LayoutNode`], pulling each
/// leaf's metadata from `meta`.
fn node_view(node: &LayoutNode, meta: &mut impl FnMut(LeafId) -> LeafView) -> NodeView {
    match node {
        LayoutNode::Leaf(id) => NodeView::Leaf { view: meta(*id) },
        LayoutNode::Split { axis, children } => NodeView::Split {
            axis: *axis,
            children: children
                .iter()
                .map(|c| ChildView {
                    flex: c.flex,
                    node: node_view(&c.node, meta),
                })
                .collect(),
        },
    }
}

/// Count leaves in a [`NodeView`] while validating split arity (`>= 2`
/// children) and flex finiteness. Returns an error on a structurally-broken
/// tree.
fn count_leaves(node: &NodeView) -> Result<usize, LoadError> {
    match node {
        NodeView::Leaf { view } => {
            if !view.tab_count_is_sane() {
                return Err(LoadError::Invalid("leaf has zero tabs".into()));
            }
            Ok(1)
        }
        NodeView::Split { children, .. } => {
            if children.len() < 2 {
                return Err(LoadError::Invalid("split has fewer than 2 children".into()));
            }
            for c in children {
                if !c.flex.is_finite() || c.flex < 0.0 {
                    return Err(LoadError::Invalid("non-finite or negative flex".into()));
                }
            }
            let mut total = 0usize;
            for c in children {
                total += count_leaves(&c.node)?;
            }
            Ok(total)
        }
    }
}

/// Recursively build a live [`LayoutNode`] from a validated [`NodeView`],
/// allocating fresh deterministic ids and collecting per-leaf metadata in DFS
/// order. Flex weights are normalized per split so the cascade stays valid even
/// if the file's weights drifted.
fn build_node(
    node: &NodeView,
    next_id: &mut u64,
    leaves: &mut Vec<(LeafId, LeafView)>,
) -> LayoutNode {
    match node {
        NodeView::Leaf { view } => {
            let id = LeafId(*next_id);
            *next_id += 1;
            let mut v = view.clone();
            v.normalize();
            leaves.push((id, v));
            LayoutNode::Leaf(id)
        }
        NodeView::Split { axis, children } => {
            let mut kids: Vec<Child> = children
                .iter()
                .map(|c| {
                    let child_node = build_node(&c.node, next_id, leaves);
                    Child::new(child_node, c.flex.max(0.0))
                })
                .collect();
            let mut split = LayoutNode::Split {
                axis: *axis,
                children: std::mem::take(&mut kids),
            };
            split.renormalize_children();
            split
        }
    }
}

impl LeafView {
    /// `true` when the leaf claims at least one tab.
    fn tab_count_is_sane(&self) -> bool {
        self.tab_count >= 1
    }

    /// Clamp `tab_count`/`active` into a usable range (`tab_count >= 1`,
    /// `active < tab_count`) so a restored cell always has a visible shell, and
    /// truncate an over-cap `scrollback` (defense-in-depth: a hand-edited or
    /// hostile file cannot make restore replay an unbounded blob).
    fn normalize(&mut self) {
        if self.tab_count == 0 {
            self.tab_count = 1;
        }
        if self.active >= self.tab_count {
            self.active = self.tab_count - 1;
        }
        if let Some(sb) = self.scrollback.take() {
            self.scrollback = Some(cap_scrollback(sb));
        }
    }
}

/// Build the per-leaf [`LeafView`] for a [`TabGroup`] given an optional cwd and
/// profile. A small helper the app can use when capturing a snapshot.
#[must_use]
pub fn leaf_view_for(group: &TabGroup, cwd: Option<String>, profile: Option<String>) -> LeafView {
    LeafView {
        tab_count: group.len().max(1),
        active: group.active.min(group.len().saturating_sub(1)),
        cwd,
        profile,
        scrollback: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rect, SplitOutcome};

    /// Build a 2x2-ish layout via the guarded action layer.
    fn grid_of(n: usize) -> Layout {
        let mut l = Layout::new();
        let mut axis = Axis::Horizontal;
        while l.leaf_count() < n {
            let t = l.focused;
            if let SplitOutcome::Split(id) = l.try_split(t, axis) {
                l.focused = id;
            }
            axis = axis.opposite();
        }
        l
    }

    #[test]
    fn round_trip_is_structurally_equal_and_byte_stable() {
        let l = grid_of(4);
        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());

        let json = snap.to_json().expect("serialize");
        let back = LayoutSnapshot::from_json(&json).expect("deserialize");
        assert_eq!(snap, back, "snapshot round-trip must be structurally equal");

        // Byte-stable: re-serializing the decoded snapshot is identical.
        let json2 = back.to_json().expect("reserialize");
        assert_eq!(json, json2, "serde output must be byte-stable");
    }

    #[test]
    fn restore_rebuilds_an_equivalent_cascade() {
        let l = grid_of(4);
        let win = Rect::new(0, 0, 1200, 900);
        let before = l.cascade(win);

        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());
        let restored = snap.restore().expect("restore");
        let after = restored.layout.cascade(win);

        // Same number of cells and same rectangles (ids are freshly allocated
        // but the geometry is identical).
        assert_eq!(before.len(), after.len());
        let before_rects: Vec<_> = before.iter().map(|(_, r)| *r).collect();
        let after_rects: Vec<_> = after.iter().map(|(_, r)| *r).collect();
        assert_eq!(before_rects, after_rects);
        assert_eq!(restored.leaves.len(), 4);
    }

    #[test]
    fn restore_preserves_focused_ordinal() {
        let mut l = grid_of(3);
        // Focus the last leaf in DFS order.
        let last = *l.leaves().last().unwrap();
        l.focused = last;
        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());
        assert_eq!(snap.focused_ordinal, 2);

        let restored = snap.restore().expect("restore");
        let restored_last = *restored.layout.leaves().last().unwrap();
        assert_eq!(restored.layout.focused, restored_last);
    }

    #[test]
    fn restore_carries_cwd_and_profile_metadata() {
        let mut l = Layout::new();
        let _ = l.try_split(l.focused, Axis::Horizontal);
        let leaves = l.leaves();
        let snap = LayoutSnapshot::capture(&l, |id| {
            if id == leaves[0] {
                LeafView::new(2, 1, Some("/home/op".into()), Some("powershell".into()))
            } else {
                LeafView::single()
            }
        });
        let restored = snap.restore().expect("restore");
        let first = &restored.leaves[0].1;
        assert_eq!(first.tab_count, 2);
        assert_eq!(first.active, 1);
        assert_eq!(first.cwd.as_deref(), Some("/home/op"));
        assert_eq!(first.profile.as_deref(), Some("powershell"));
    }

    #[test]
    fn corrupt_json_strict_errors_and_load_falls_back() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-corrupt-{}.json", std::process::id()));
        std::fs::write(&tmp, "this is { not ] valid json").unwrap();

        // Strict surfaces the parse error.
        let err = load_strict(&tmp).unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)), "got {err:?}");

        // The safe loader degrades to a single pane, never panics.
        let restored = load(&tmp);
        assert_eq!(restored.layout.leaf_count(), 1);
        assert_eq!(restored.leaves.len(), 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn over_cap_tree_is_rejected() {
        // Hand-build a 7-leaf snapshot (over MAX_PANES) bypassing the action guard.
        let mut children = Vec::new();
        for _ in 0..7 {
            children.push(ChildView {
                flex: 1.0 / 7.0,
                node: NodeView::Leaf {
                    view: LeafView::single(),
                },
            });
        }
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children,
            },
            focused_ordinal: 0,
        };
        let err = snap.validate().unwrap_err();
        assert!(matches!(err, LoadError::Invalid(_)), "got {err:?}");
        // restore also refuses the over-cap tree.
        assert!(snap.restore().is_err());
    }

    #[test]
    fn invalid_flex_sum_is_rejected() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children: vec![
                    ChildView {
                        flex: f32::NAN,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                    ChildView {
                        flex: 0.5,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                ],
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn split_with_one_child_is_rejected() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Vertical,
                children: vec![ChildView {
                    flex: 1.0,
                    node: NodeView::Leaf {
                        view: LeafView::single(),
                    },
                }],
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let snap = LayoutSnapshot {
            version: 999,
            root: NodeView::Leaf {
                view: LeafView::single(),
            },
            focused_ordinal: 0,
        };
        assert!(matches!(
            snap.validate(),
            Err(LoadError::UnsupportedVersion(999))
        ));
    }

    #[test]
    fn valid_single_pane_restores() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Leaf {
                view: LeafView::single(),
            },
            focused_ordinal: 0,
        };
        let restored = snap.restore().expect("single pane restores");
        assert_eq!(restored.layout.leaf_count(), 1);
    }

    #[test]
    fn save_then_load_round_trips_through_disk() {
        let l = grid_of(3);
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-ws-{}.json", std::process::id()));
        save(&l, &tmp, |_| LeafView::single()).expect("save");

        let restored = load_strict(&tmp).expect("load");
        assert_eq!(restored.layout.leaf_count(), 3);
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(restored.layout.cascade(win).len(), 3);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn missing_file_falls_back_to_single_pane() {
        let missing = std::env::temp_dir().join("c0pl4nd-does-not-exist-xyzzy.json");
        let _ = std::fs::remove_file(&missing);
        let restored = load(&missing);
        assert_eq!(restored.layout.leaf_count(), 1);
    }

    #[test]
    fn leaf_view_for_clamps_active() {
        let mut g = TabGroup::new(LeafId(0), 0);
        g.add_tab(1);
        g.add_tab(2);
        g.active = 99; // out of range
        let v = leaf_view_for(&g, Some("/tmp".into()), None);
        assert_eq!(v.tab_count, 3);
        assert_eq!(v.active, 2);
        assert_eq!(v.cwd.as_deref(), Some("/tmp"));
        assert!(v.scrollback.is_none(), "scrollback defaults to None");
    }

    // --- WorkspaceSnapshot (multi-tab) -----------------------------------

    /// Build a single-tab snapshot from a live layout, with per-leaf metadata.
    fn snap_of(n: usize, meta: impl FnMut(LeafId) -> LeafView) -> LayoutSnapshot {
        LayoutSnapshot::capture(&grid_of(n), meta)
    }

    #[test]
    fn workspace_multi_tab_round_trip_is_structural_and_byte_stable() {
        // Three window tabs of differing shapes; per-leaf cwd on the middle tab.
        let tab0 = snap_of(2, |_| LeafView::single());
        let tab1 = snap_of(3, |id| {
            LeafView::new(1, 0, Some(format!("/d/{}", id.0)), Some("pwsh".into()))
        });
        let tab2 = snap_of(1, |_| LeafView::single());
        let ws = WorkspaceSnapshot::from_tabs(vec![tab0, tab1, tab2], 1);
        assert_eq!(ws.version, WorkspaceSnapshot::VERSION);
        assert_eq!(ws.tabs.len(), 3);
        assert_eq!(ws.active, 1);

        let json = ws.to_json().expect("serialize");
        let back = WorkspaceSnapshot::from_json(&json).expect("deserialize");
        assert_eq!(ws, back, "workspace round-trip must be structurally equal");

        // Byte-stable.
        let json2 = back.to_json().expect("reserialize");
        assert_eq!(json, json2, "workspace serde output must be byte-stable");

        // Active index + per-leaf cwd survive a full restore.
        let restored = back.restore_all().expect("restore");
        assert_eq!(restored.tabs.len(), 3);
        assert_eq!(restored.active, 1);
        assert_eq!(restored.tabs[0].layout.leaf_count(), 2);
        assert_eq!(restored.tabs[1].layout.leaf_count(), 3);
        assert_eq!(restored.tabs[2].layout.leaf_count(), 1);
        // The middle tab's first leaf carries its persisted cwd.
        let cwd = restored.tabs[1].leaves[0].1.cwd.as_deref();
        assert!(
            cwd.is_some() && cwd.unwrap().starts_with("/d/"),
            "got {cwd:?}"
        );
    }

    #[test]
    fn workspace_from_tabs_clamps_active_and_coerces_empty() {
        // Active out of range clamps to the last tab.
        let ws = WorkspaceSnapshot::from_tabs(vec![snap_of(1, |_| LeafView::single())], 99);
        assert_eq!(ws.active, 0);

        // Empty tab list coerces to a single default tab.
        let ws = WorkspaceSnapshot::from_tabs(vec![], 5);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.active, 0);
        assert_eq!(
            ws.tabs[0].root,
            NodeView::Leaf {
                view: LeafView::single()
            }
        );
    }

    #[test]
    fn v1_single_layout_file_migrates_to_one_tab_workspace() {
        // A v1 file is a BARE LayoutSnapshot (no `tabs`/wrapper).
        let v1 = snap_of(3, |_| LeafView::single());
        let v1_json = v1.to_json().expect("serialize v1");
        // Sanity: the v1 json has no "tabs" key — it is the bare layout format.
        assert!(
            !v1_json.contains("\"tabs\""),
            "v1 file must be the bare layout format"
        );

        let ws = WorkspaceSnapshot::from_json(&v1_json).expect("v1 migrates");
        assert_eq!(ws.version, WorkspaceSnapshot::VERSION);
        assert_eq!(ws.tabs.len(), 1, "v1 migrates to a single-tab workspace");
        assert_eq!(ws.active, 0);
        // The migrated tab is structurally the original v1 layout.
        assert_eq!(ws.tabs[0], v1);

        // And it fully restores.
        let restored = ws.restore_all().expect("restore migrated");
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].layout.leaf_count(), 3);
    }

    #[test]
    fn workspace_save_atomic_then_load_round_trips_through_disk() {
        let ws = WorkspaceSnapshot::from_tabs(
            vec![
                snap_of(2, |_| LeafView::single()),
                snap_of(4, |_| LeafView::single()),
            ],
            1,
        );
        let tmp =
            std::env::temp_dir().join(format!("c0pl4nd-ws-multi-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        ws.save_atomic(&tmp).expect("save_atomic");

        // No sibling .tmp left behind on success.
        let mut tmp_name = tmp.file_name().unwrap().to_os_string();
        tmp_name.push(".tmp");
        let sidecar = tmp.parent().unwrap().join(tmp_name);
        assert!(!sidecar.exists(), "atomic write must leave no .tmp behind");

        let back = WorkspaceSnapshot::load_strict(&tmp).expect("load_strict");
        assert_eq!(ws, back);
        let restored = WorkspaceSnapshot::load(&tmp)
            .restore_all()
            .expect("restore");
        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.active, 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn workspace_corrupt_file_falls_back_to_single_default_tab() {
        let tmp =
            std::env::temp_dir().join(format!("c0pl4nd-ws-corrupt-{}.json", std::process::id()));
        std::fs::write(&tmp, "this is { not ] valid json").unwrap();

        // Strict surfaces the parse error.
        assert!(WorkspaceSnapshot::load_strict(&tmp).is_err());

        // The safe loader degrades to a single default tab, never panics.
        let ws = WorkspaceSnapshot::load(&tmp);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.active, 0);
        let restored = ws.restore_all().expect("restore fallback");
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].layout.leaf_count(), 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn workspace_missing_file_falls_back_to_single_default_tab() {
        let missing = std::env::temp_dir().join("c0pl4nd-ws-absent-xyzzy.json");
        let _ = std::fs::remove_file(&missing);
        let ws = WorkspaceSnapshot::load(&missing);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.active, 0);
    }

    #[test]
    fn workspace_unknown_future_version_falls_back() {
        // A wrapper-shaped file with an unknown future version is rejected →
        // the safe loader degrades to a single default tab.
        let json = r#"{ "version": 999, "tabs": [ { "version": 1, "root": { "kind": "leaf", "view": { "tab_count": 1, "active": 0 } }, "focused_ordinal": 0 } ], "active": 0 }"#;
        assert!(matches!(
            WorkspaceSnapshot::from_json(json),
            Err(LoadError::UnsupportedVersion(999))
        ));
    }

    #[test]
    fn workspace_over_cap_tab_is_rejected() {
        // A tab with > MAX_PANES leaves makes the whole workspace invalid.
        let mut children = Vec::new();
        for _ in 0..(MAX_PANES + 1) {
            children.push(ChildView {
                flex: 1.0,
                node: NodeView::Leaf {
                    view: LeafView::single(),
                },
            });
        }
        let bad_tab = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children,
            },
            focused_ordinal: 0,
        };
        let mut ws = WorkspaceSnapshot {
            version: WorkspaceSnapshot::VERSION,
            tabs: vec![bad_tab],
            active: 0,
        };
        let json = serde_json::to_string(&ws).unwrap();
        assert!(WorkspaceSnapshot::from_json(&json).is_err());
        // restore_all also refuses.
        assert!(ws.restore_all().is_err());
        // validate_and_normalize surfaces it directly.
        assert!(ws.validate_and_normalize().is_err());
    }

    // --- scrollback slot -------------------------------------------------

    #[test]
    fn scrollback_defaults_none_and_round_trips_when_some() {
        // Default: None, and the field is omitted from JSON entirely.
        let v = LeafView::single();
        assert!(v.scrollback.is_none());
        let json = serde_json::to_string(&v).unwrap();
        assert!(
            !json.contains("scrollback"),
            "None scrollback is skipped: {json}"
        );

        // Some round-trips through serde.
        let lines = vec!["line 1".to_string(), "line 2".to_string()];
        let v = LeafView::single().with_scrollback(Some(lines.clone()));
        assert_eq!(v.scrollback.as_deref(), Some(lines.as_slice()));
        let json = serde_json::to_string(&v).unwrap();
        let back: LeafView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.scrollback, Some(lines));
    }

    #[test]
    fn scrollback_is_capped_at_capture_and_on_load() {
        // with_scrollback caps to SCROLLBACK_MAX_LINES (keeping the tail).
        let big: Vec<String> = (0..(SCROLLBACK_MAX_LINES + 50))
            .map(|i| i.to_string())
            .collect();
        let v = LeafView::single().with_scrollback(Some(big.clone()));
        let sb = v.scrollback.as_ref().unwrap();
        assert_eq!(sb.len(), SCROLLBACK_MAX_LINES, "capped at capture time");
        // The tail (most-recent lines) is kept.
        assert_eq!(sb.last().unwrap(), &(SCROLLBACK_MAX_LINES + 49).to_string());

        // A hand-built over-cap leaf is also truncated on load (via restore →
        // normalize), defending against a hostile/edited file.
        let over: Vec<String> = (0..(SCROLLBACK_MAX_LINES + 10))
            .map(|i| i.to_string())
            .collect();
        let leaf = LeafView {
            tab_count: 1,
            active: 0,
            cwd: None,
            profile: None,
            scrollback: Some(over),
        };
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Leaf { view: leaf },
            focused_ordinal: 0,
        };
        let restored = snap.restore().expect("restore");
        let loaded_sb = restored.leaves[0].1.scrollback.as_ref().unwrap();
        assert_eq!(loaded_sb.len(), SCROLLBACK_MAX_LINES, "capped on load");
    }

    #[test]
    fn scrollback_survives_full_workspace_round_trip() {
        let lines = vec!["$ echo hi".to_string(), "hi".to_string()];
        let tab = LayoutSnapshot::capture(&Layout::new(), |_| {
            LeafView::single().with_scrollback(Some(lines.clone()))
        });
        let ws = WorkspaceSnapshot::from_tabs(vec![tab], 0);
        let json = ws.to_json().unwrap();
        let back = WorkspaceSnapshot::from_json(&json).unwrap();
        let restored = back.restore_all().unwrap();
        assert_eq!(restored.tabs[0].leaves[0].1.scrollback, Some(lines));
    }

    // --- restore-outcome corruption guard (F4-2) -------------------------

    /// A unique temp path per test invocation so the suite is parallel-safe.
    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "c0pl4nd-outcome-{}-{}-{tag}.json",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn load_outcome_absent_file_is_absent_not_corrupt() {
        let missing = unique_tmp("absent");
        let _ = std::fs::remove_file(&missing);
        let outcome = WorkspaceSnapshot::load_outcome(&missing);
        assert_eq!(outcome, RestoreOutcome::Absent);
        assert!(!outcome.is_corrupt(), "an absent file is NOT corrupt");
        assert_eq!(outcome.corrupt_reason(), None);
    }

    #[test]
    fn load_outcome_malformed_json_is_corrupt_with_reason() {
        let tmp = unique_tmp("malformed");
        std::fs::write(&tmp, "this is { not ] valid json").unwrap();
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        assert!(
            outcome.is_corrupt(),
            "malformed JSON must be Corrupt: {outcome:?}"
        );
        let reason = outcome.corrupt_reason().expect("a corrupt reason");
        assert!(!reason.is_empty(), "the reason must be human-readable");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_outcome_truncated_json_is_corrupt() {
        // A valid workspace JSON cut off mid-structure (the classic
        // crash-during-save / partial-write corruption shape).
        let tmp = unique_tmp("truncated");
        let ws = WorkspaceSnapshot::from_tabs(vec![snap_of(2, |_| LeafView::single())], 0);
        let full = ws.to_json().unwrap();
        let truncated = &full[..full.len() / 2];
        std::fs::write(&tmp, truncated).unwrap();
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        assert!(
            outcome.is_corrupt(),
            "truncated JSON must be Corrupt: {outcome:?}"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_outcome_unknown_version_is_corrupt() {
        let tmp = unique_tmp("badversion");
        // A wrapper-shaped file with an unknown future version → Corrupt.
        let json = r#"{ "version": 999, "tabs": [ { "version": 1, "root": { "kind": "leaf", "view": { "tab_count": 1, "active": 0 } }, "focused_ordinal": 0 } ], "active": 0 }"#;
        std::fs::write(&tmp, json).unwrap();
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        assert!(
            outcome.is_corrupt(),
            "unknown version must be Corrupt: {outcome:?}"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_outcome_over_cap_tree_is_corrupt() {
        // A semantically-invalid tree (over MAX_PANES) that parses as JSON but
        // fails validation → Corrupt, not a silent drop.
        let tmp = unique_tmp("overcap");
        let mut children = Vec::new();
        for _ in 0..(MAX_PANES + 1) {
            children.push(ChildView {
                flex: 1.0,
                node: NodeView::Leaf {
                    view: LeafView::single(),
                },
            });
        }
        let bad_tab = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children,
            },
            focused_ordinal: 0,
        };
        let ws = WorkspaceSnapshot {
            version: WorkspaceSnapshot::VERSION,
            tabs: vec![bad_tab],
            active: 0,
        };
        std::fs::write(&tmp, ws.to_json().unwrap()).unwrap();
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        assert!(
            outcome.is_corrupt(),
            "over-cap tree must be Corrupt: {outcome:?}"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_outcome_out_of_range_active_is_coerced_and_restored() {
        // An out-of-range active index is COERCED (clamped) on load, not
        // rejected — a recoverable defect, so the outcome is Restored.
        let tmp = unique_tmp("oob-active");
        let ws = WorkspaceSnapshot {
            version: WorkspaceSnapshot::VERSION,
            tabs: vec![snap_of(2, |_| LeafView::single())],
            active: 99, // out of range
        };
        std::fs::write(&tmp, ws.to_json().unwrap()).unwrap();
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        match outcome {
            RestoreOutcome::Restored(r) => {
                assert_eq!(r.active, 0, "out-of-range active clamps to the last tab");
                assert_eq!(r.tabs.len(), 1);
            }
            other => panic!("expected coerced Restored, got {other:?}"),
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_outcome_valid_file_round_trips_to_restored() {
        let tmp = unique_tmp("valid");
        let ws = WorkspaceSnapshot::from_tabs(
            vec![
                snap_of(2, |_| LeafView::single()),
                snap_of(3, |_| LeafView::single()),
            ],
            1,
        );
        ws.save_atomic(&tmp).expect("save");
        let outcome = WorkspaceSnapshot::load_outcome(&tmp);
        match outcome {
            RestoreOutcome::Restored(r) => {
                assert_eq!(r.tabs.len(), 2);
                assert_eq!(r.active, 1);
                assert_eq!(r.tabs[0].layout.leaf_count(), 2);
                assert_eq!(r.tabs[1].layout.leaf_count(), 3);
            }
            other => panic!("expected Restored, got {other:?}"),
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn quarantine_renames_corrupt_file_aside_without_deleting_data() {
        let tmp = unique_tmp("quarantine");
        let body = "this is { not ] valid json";
        std::fs::write(&tmp, body).unwrap();

        let dest = quarantine_corrupt(&tmp).expect("quarantine");

        // The original path is gone (so it won't be re-loaded next launch)...
        assert!(
            !tmp.exists(),
            "the corrupt file is moved aside, not left in place"
        );
        // ...but the data is preserved at the quarantine path (never deleted).
        assert!(dest.exists(), "the quarantine file exists for debugging");
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            body,
            "data preserved verbatim"
        );
        // The quarantine name carries the `.corrupt-<hash8>` provenance suffix.
        let dest_name = dest.file_name().unwrap().to_string_lossy().into_owned();
        assert!(dest_name.contains(".corrupt-"), "got {dest_name}");

        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn quarantine_hash_is_stable_for_same_bytes() {
        let a = unique_tmp("stable-a");
        let b = unique_tmp("stable-b");
        let body = "identical corrupt bytes";
        std::fs::write(&a, body).unwrap();
        std::fs::write(&b, body).unwrap();

        let da = quarantine_corrupt(&a).expect("quarantine a");
        let db = quarantine_corrupt(&b).expect("quarantine b");

        // Same bytes → same `.corrupt-<hash8>` suffix (deterministic, no RNG).
        let sa = da.file_name().unwrap().to_string_lossy();
        let sb = db.file_name().unwrap().to_string_lossy();
        let suffix_a = sa.rsplit(".corrupt-").next().unwrap();
        let suffix_b = sb.rsplit(".corrupt-").next().unwrap();
        assert_eq!(suffix_a, suffix_b, "stable hash for identical bytes");

        let _ = std::fs::remove_file(&da);
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn quarantine_missing_file_errors_but_does_not_panic() {
        let missing = unique_tmp("quarantine-absent");
        let _ = std::fs::remove_file(&missing);
        // Best-effort: a missing file yields an Io error, never a panic.
        assert!(matches!(
            quarantine_corrupt(&missing),
            Err(LoadError::Io(_))
        ));
    }

    // --- additional edge-coverage -----------------------------------------

    #[test]
    fn load_error_display_messages() {
        // Each Display arm is distinct and embeds its payload.
        assert_eq!(
            LoadError::Io("boom".into()).to_string(),
            "layout file io error: boom"
        );
        assert_eq!(
            LoadError::Parse("nope".into()).to_string(),
            "layout file parse error: nope"
        );
        assert_eq!(
            LoadError::UnsupportedVersion(7).to_string(),
            "unsupported layout version 7"
        );
        assert_eq!(
            LoadError::Invalid("empty".into()).to_string(),
            "invalid layout: empty"
        );
    }

    #[test]
    fn restored_layout_single_pane_is_one_leaf() {
        let r = RestoredLayout::single_pane();
        assert_eq!(r.layout.leaf_count(), 1);
        assert_eq!(r.leaves.len(), 1);
        assert_eq!(r.leaves[0].1, LeafView::single());
    }

    #[test]
    fn restored_workspace_single_tab_fallback() {
        let w = RestoredWorkspace::single_tab();
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.active, 0);
        assert_eq!(w.tabs[0].layout.leaf_count(), 1);
    }

    #[test]
    fn workspace_default_is_single_tab() {
        let w = WorkspaceSnapshot::default();
        assert_eq!(w, WorkspaceSnapshot::single_tab());
    }

    #[test]
    fn restore_outcome_corrupt_reason_is_none_for_non_corrupt() {
        assert_eq!(RestoreOutcome::Absent.corrupt_reason(), None);
        assert!(!RestoreOutcome::Absent.is_corrupt());
        let restored = RestoreOutcome::Restored(RestoredWorkspace::single_tab());
        assert_eq!(restored.corrupt_reason(), None);
        assert!(!restored.is_corrupt());
    }

    #[test]
    fn leaf_view_for_empty_group_clamps_tab_count_to_one() {
        // A pathological group with zero tabs (only transiently true mid-collapse)
        // must clamp tab_count to >= 1 and active to 0.
        let mut g = TabGroup::new(LeafId(0), 0);
        let (_, empty) = g.close_active();
        assert!(empty && g.is_empty());
        let v = leaf_view_for(&g, None, Some("zsh".into()));
        assert_eq!(v.tab_count, 1, "empty group clamps to one tab");
        assert_eq!(v.active, 0);
        assert_eq!(v.profile.as_deref(), Some("zsh"));
        assert!(v.cwd.is_none());
    }

    #[test]
    fn count_leaves_rejects_zero_tab_leaf() {
        // A leaf claiming zero tabs is structurally invalid → Invalid error.
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Leaf {
                view: LeafView {
                    tab_count: 0,
                    active: 0,
                    cwd: None,
                    profile: None,
                    scrollback: None,
                },
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
        assert!(snap.restore().is_err());
    }

    #[test]
    fn count_leaves_rejects_negative_flex() {
        // A negative (finite) flex is rejected by count_leaves' flex check.
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children: vec![
                    ChildView {
                        flex: -0.5,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                    ChildView {
                        flex: 0.5,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                ],
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn layout_snapshot_save_creates_parent_and_round_trips() {
        // Exercise LayoutSnapshot::save directly (distinct from the free `save`),
        // including the parent-dir creation branch.
        let base = unique_tmp("ls-save");
        let dir = base.with_extension("d");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("layout.json");

        let snap = LayoutSnapshot::capture(&grid_of(2), |_| LeafView::single());
        snap.save(&path).expect("save creates parent dirs");
        assert!(path.exists());

        let back =
            LayoutSnapshot::from_json(&std::fs::read_to_string(&path).unwrap()).expect("reparse");
        assert_eq!(back, snap);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_json_path1_empty_tabs_wrapper_falls_through_to_v1_then_fails() {
        // A wrapper-shaped value with an UNKNOWN version AND empty tabs does NOT
        // hit the early UnsupportedVersion return (that needs non-empty tabs);
        // it falls through to the v1 bare-LayoutSnapshot parse, which fails here
        // because the shape is not a valid bare LayoutSnapshot → Parse error.
        let json = r#"{ "version": 999, "tabs": [], "active": 0 }"#;
        let err = WorkspaceSnapshot::from_json(json).unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn node_view_round_trips_a_nested_split_via_capture() {
        // node_view's Split arm recurses; capture a real nested tree and confirm
        // the persisted shape mirrors the live one (split axis + child count).
        let l = grid_of(4);
        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());
        // The captured root mirrors the live root's split-ness.
        match (&l.root, &snap.root) {
            (LayoutNode::Split { children: lc, .. }, NodeView::Split { children: sc, .. }) => {
                assert_eq!(
                    lc.len(),
                    sc.len(),
                    "child arity preserved through node_view"
                );
            }
            other => panic!("expected both splits, got {other:?}"),
        }
        // And it restores to the same leaf count.
        assert_eq!(snap.restore().unwrap().layout.leaf_count(), 4);
    }

    #[test]
    fn restore_clamps_out_of_range_focused_ordinal() {
        // focused_ordinal past the end clamps to the last leaf, not a panic.
        let mut snap = LayoutSnapshot::capture(&grid_of(3), |_| LeafView::single());
        snap.focused_ordinal = 999;
        let restored = snap.restore().expect("restore clamps the ordinal");
        let last = *restored.layout.leaves().last().unwrap();
        assert_eq!(restored.layout.focused, last, "clamps to the last leaf");
    }

    #[test]
    fn workspace_save_atomic_creates_then_load_outcome_quarantine_cycle() {
        // End-to-end: a corrupt file → load_outcome Corrupt → quarantine moves it
        // aside so the next load_outcome on the original path is Absent.
        let tmp = unique_tmp("cycle");
        std::fs::write(&tmp, "garbage { not json").unwrap();
        assert!(WorkspaceSnapshot::load_outcome(&tmp).is_corrupt());
        let dest = quarantine_corrupt(&tmp).expect("quarantine");
        assert_eq!(
            WorkspaceSnapshot::load_outcome(&tmp),
            RestoreOutcome::Absent,
            "after quarantine the original path is Absent"
        );
        let _ = std::fs::remove_file(&dest);
    }
}
