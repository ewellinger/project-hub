//! The agent skill bundled in the binary and written into every hub
//! worktree as a generated, locally excluded file.

use std::path::Path;

use crate::error::{HubError, Result};
use crate::git::Git;
use crate::hub::Hub;

/// `skills/hub/SKILL.md` at build time; the single source of truth.
pub const SKILL: &str = include_str!("../skills/hub/SKILL.md");

/// Where the skill lives in a worktree, for Claude Code and Codex.
pub const SKILL_DIRS: [&str; 2] = [".claude/skills/hub", ".agents/skills/hub"];

const SKILL_FILE: &str = "SKILL.md";

/// Write the skill into `worktree` wherever it is missing or differs.
/// An up-to-date file is not touched.
pub fn install(worktree: &Path) -> Result<()> {
    for dir in SKILL_DIRS {
        let dir = worktree.join(dir);
        let path = dir.join(SKILL_FILE);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(SKILL) {
            continue;
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| HubError::io(format!("creating {}", dir.display()), e))?;
        std::fs::write(&path, SKILL)
            .map_err(|e| HubError::io(format!("writing {}", path.display()), e))?;
    }
    Ok(())
}

/// Add both skill directories to the repository's local exclude file, which
/// every linked worktree shares.
pub fn ensure_ignored(git: &Git) -> Result<()> {
    for dir in SKILL_DIRS {
        git.exclude_locally(&format!("{dir}/"))?;
    }
    Ok(())
}

/// Bring every existing worktree of `hub` up to the embedded skill and make
/// sure the files stay untracked. Returns one message per failure; the
/// caller decides how to show them.
pub fn refresh(hub: &Hub) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Err(err) = ensure_ignored(&hub.git()) {
        warnings.push(format!("could not exclude the hub skill from git: {err}"));
    }
    for worktree in hub.worktrees.iter().filter(|p| p.is_dir()) {
        if let Err(err) = install(worktree) {
            warnings.push(format!(
                "could not refresh the hub skill in {}: {err}",
                worktree.display()
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn skill_paths(dir: &Path) -> Vec<std::path::PathBuf> {
        SKILL_DIRS
            .iter()
            .map(|d| dir.join(d).join(SKILL_FILE))
            .collect()
    }

    #[test]
    fn install_writes_both_copies() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        for path in skill_paths(tmp.path()) {
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                SKILL,
                "{}",
                path.display()
            );
        }
        assert!(SKILL.starts_with("---\nname: hub\n"));
    }

    #[test]
    fn install_leaves_an_up_to_date_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        for path in skill_paths(tmp.path()) {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
        // A rewrite of a read-only file would fail; an untouched one succeeds.
        install(tmp.path()).unwrap();
    }

    #[test]
    fn install_replaces_a_stale_copy() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        let [first, second]: [std::path::PathBuf; 2] = skill_paths(tmp.path()).try_into().unwrap();
        std::fs::write(&first, "stale\n").unwrap();
        std::fs::remove_file(&second).unwrap();
        install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), SKILL);
        assert_eq!(std::fs::read_to_string(&second).unwrap(), SKILL);
    }

    #[test]
    fn install_reports_an_unwritable_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join(".claude");
        std::fs::write(&blocker, "not a directory").unwrap();
        let err = install(tmp.path()).unwrap_err();
        assert!(matches!(err, HubError::Io { .. }), "{err:?}");
    }

    #[test]
    fn ensure_ignored_adds_both_patterns_once() {
        let tmp = tempfile::tempdir().unwrap();
        Git::init(tmp.path(), "main").unwrap();
        let git = Git::new(tmp.path());
        ensure_ignored(&git).unwrap();
        ensure_ignored(&git).unwrap();
        let exclude = std::fs::read_to_string(tmp.path().join(".git/info/exclude")).unwrap();
        for dir in SKILL_DIRS {
            let pattern = format!("{dir}/");
            assert_eq!(
                exclude.lines().filter(|l| *l == pattern).count(),
                1,
                "{pattern} in:\n{exclude}"
            );
        }
        install(tmp.path()).unwrap();
        assert!(git.is_ignored(".claude/skills/hub/SKILL.md").unwrap());
        assert!(git.is_ignored(".agents/skills/hub/SKILL.md").unwrap());
    }
}
