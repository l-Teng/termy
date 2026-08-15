use super::{
    NativePaneLayoutTree, NativePaneZoomSnapshot, TabId, TerminalTab, workspaces::WorkspaceEntry,
};
use std::collections::HashMap;

/// Mutable terminal-session state that must stay coherent across tab,
/// workspace, and native-pane operations.
pub(super) struct SessionState {
    pub(super) tabs: Vec<TerminalTab>,
    /// All workspaces in sidebar order. The entry at `active_workspace` always
    /// has an empty `tabs` vec: the active workspace's tabs live in `tabs` so
    /// the existing tab machinery only ever sees the visible strip.
    pub(super) workspaces: Vec<WorkspaceEntry>,
    pub(super) active_workspace: usize,
    pub(super) next_workspace_id: u64,
    pub(super) native_pane_zoom_snapshots: HashMap<TabId, NativePaneZoomSnapshot>,
    pub(super) native_pane_layout_trees: HashMap<TabId, NativePaneLayoutTree>,
    pub(super) next_tab_id: TabId,
    pub(super) active_tab: usize,
}

impl SessionState {
    pub(super) fn new() -> Self {
        Self {
            tabs: Vec::new(),
            workspaces: vec![WorkspaceEntry::new(1)],
            active_workspace: 0,
            next_workspace_id: 2,
            native_pane_zoom_snapshots: HashMap::new(),
            native_pane_layout_trees: HashMap::new(),
            next_tab_id: 1,
            active_tab: 0,
        }
    }
}
