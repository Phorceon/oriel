//! Messages for the worker thread.

use crate::ax::Ax;
use oriel_core::Command;

/// A note from the OS workspace (main thread).
#[derive(Debug)]
pub enum WsNote {
    /// An app launched or quit: compare the watched apps with the running ones.
    Reconcile,
    /// This pid became the frontmost app.
    Activated(i32),
}

/// An AX notification, copied out of the observer callback.
pub struct AxNote {
    pub pid: i32,
    pub element: Ax,
    pub name: String,
}

pub enum Msg {
    Cmd(Command),
    Workspace(WsNote),
    Ax(AxNote),
    /// Answer on this channel when every message before this one has run.
    Flush(std::sync::mpsc::Sender<()>),
}
