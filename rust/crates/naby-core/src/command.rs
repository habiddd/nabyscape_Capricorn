//! Commands emitted by views and executed by the platform host.

/// A user intent that needs the platform (media keys, audio endpoints, shell).
///
/// Views never call Win32 directly; they push `Command`s and the host executes them.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    OpenSettings,
    Quit,
}
