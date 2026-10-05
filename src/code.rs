use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{HubError, Result};

/// Open a workspace file with `editor` (the command and its arguments,
/// `code` by default) found on PATH.
pub fn open_workspace(editor: &[String], path: &Path) -> Result<()> {
    let status = command(editor, path)
        .status()
        .map_err(|e| HubError::io(format!("running {}", editor[0]), e))?;
    check(editor, path, status)
}

/// Same as `open_workspace`, but with stdin, stdout, and stderr silenced:
/// for callers running with the terminal in raw mode, where the child's
/// inherited stdio would corrupt the screen.
pub fn open_workspace_quiet(editor: &[String], path: &Path) -> Result<()> {
    let status = command(editor, path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| HubError::io(format!("running {}", editor[0]), e))?;
    check(editor, path, status)
}

fn command(editor: &[String], path: &Path) -> Command {
    let mut cmd = Command::new(&editor[0]);
    cmd.args(&editor[1..]).arg(path);
    cmd
}

fn check(editor: &[String], path: &Path, status: std::process::ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(HubError::Command {
            command: format!("{} {}", editor.join(" "), path.display()),
            stderr: format!("exit status {status}"),
        })
    }
}
