use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{HubError, Result};

/// A process and its working directory, as `lsof -d cwd` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub pid: String,
    pub command: String,
    pub cwd: PathBuf,
}

impl Holder {
    pub fn describe(&self) -> String {
        format!("{} {} (cwd {})", self.pid, self.command, self.cwd.display())
    }
}

/// Every process's working directory, or `None` when `lsof` cannot run at
/// all. This process, the shell that ran it and the `lsof` child itself are
/// left out: they inherit the caller's directory, which is often the very
/// worktree being checked, and none of them recreates files in it.
pub fn cwd_holders() -> Result<Option<Vec<Holder>>> {
    cwd_holders_with("lsof")
}

fn cwd_holders_with(program: impl Into<OsString>) -> Result<Option<Vec<Holder>>> {
    let program = program.into();
    let running = |e| HubError::io(format!("running {}", program.to_string_lossy()), e);
    let child = match Command::new(&program)
        .args(["-w", "-d", "cwd", "-F", "pcn"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(running(e)),
    };
    let own = [
        std::process::id(),
        std::os::unix::process::parent_id(),
        child.id(),
    ]
    .map(|pid| pid.to_string());
    let out = child.wait_with_output().map_err(running)?;
    let mut holders = parse(&String::from_utf8_lossy(&out.stdout));
    holders.retain(|h| !own.contains(&h.pid));
    Ok(Some(holders))
}

/// Holders whose cwd is `root` or below it.
pub fn holders_in<'a>(holders: &'a [Holder], root: &Path) -> Vec<&'a Holder> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    holders
        .iter()
        .filter(|h| h.cwd.starts_with(&root))
        .collect()
}

/// `-F pcn` output: `p<pid>` opens a process, `c<command>` names it, and each
/// `n<path>` is one of its files (here, only the cwd).
fn parse(text: &str) -> Vec<Holder> {
    let mut holders = Vec::new();
    let mut pid = String::new();
    let mut command = String::new();
    for line in text.lines() {
        let (tag, value) = match line.chars().next() {
            Some(c) => (c, &line[c.len_utf8()..]),
            None => continue,
        };
        match tag {
            'p' => {
                pid = value.to_string();
                command.clear();
            }
            'c' => command = value.to_string(),
            'n' => holders.push(Holder {
                pid: pid.clone(),
                command: command.clone(),
                cwd: PathBuf::from(value),
            }),
            _ => {}
        }
    }
    holders
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_field_output() {
        let text = "p230\ncvite\nfcwd\nn/work/app\np231\ncnode\nfcwd\nn/elsewhere\n";
        assert_eq!(
            parse(text),
            vec![
                Holder {
                    pid: "230".into(),
                    command: "vite".into(),
                    cwd: "/work/app".into()
                },
                Holder {
                    pid: "231".into(),
                    command: "node".into(),
                    cwd: "/elsewhere".into()
                },
            ]
        );
    }

    #[test]
    fn holders_in_matches_the_root_and_its_descendants_only() {
        let holders =
            parse("p1\nca\nfcwd\nn/w/feat\np2\ncb\nfcwd\nn/w/feat/app\np3\ncc\nfcwd\nn/w/feat-2\n");
        let inside: Vec<&str> = holders_in(&holders, Path::new("/w/feat"))
            .iter()
            .map(|h| h.pid.as_str())
            .collect();
        assert_eq!(inside, ["1", "2"]);
    }

    #[test]
    fn a_missing_lsof_is_reported_as_unavailable_not_an_error() {
        assert_eq!(cwd_holders_with("hub-test-no-such-lsof").unwrap(), None);
    }
}
