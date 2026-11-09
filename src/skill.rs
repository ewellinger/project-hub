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

/// Every file of the skill as (name, content); `SKILL.md` links the rest.
pub const SKILL_FILES: [(&str, &str); 2] = [
    ("SKILL.md", SKILL),
    (
        "setup-scripts.md",
        include_str!("../skills/hub/setup-scripts.md"),
    ),
];

/// Write the skill into `worktree` wherever a file is missing or differs.
/// An up-to-date file is not touched.
pub fn install(worktree: &Path) -> Result<()> {
    for dir in SKILL_DIRS {
        let dir = worktree.join(dir);
        for (name, content) in SKILL_FILES {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() == Some(content) {
                continue;
            }
            std::fs::create_dir_all(&dir)
                .map_err(|e| HubError::io(format!("creating {}", dir.display()), e))?;
            std::fs::write(&path, content)
                .map_err(|e| HubError::io(format!("writing {}", path.display()), e))?;
        }
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

    /// Every installed file with the content it must hold.
    fn skill_paths(dir: &Path) -> Vec<(std::path::PathBuf, &'static str)> {
        SKILL_DIRS
            .iter()
            .flat_map(|d| SKILL_FILES.map(|(name, content)| (dir.join(d).join(name), content)))
            .collect()
    }

    #[test]
    fn install_writes_both_copies() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        for (path, content) in skill_paths(tmp.path()) {
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                content,
                "{}",
                path.display()
            );
        }
        assert!(SKILL.starts_with("---\nname: hub\n"));
        for (name, _) in &SKILL_FILES[1..] {
            assert!(
                SKILL.contains(&format!("]({name})")),
                "SKILL.md links {name}"
            );
        }
    }

    #[test]
    fn install_leaves_an_up_to_date_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        for (path, _) in skill_paths(tmp.path()) {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
        // A rewrite of a read-only file would fail; an untouched one succeeds.
        install(tmp.path()).unwrap();
    }

    #[test]
    fn install_replaces_a_stale_copy() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        let files = skill_paths(tmp.path());
        for (i, (path, _)) in files.iter().enumerate() {
            if i % 2 == 0 {
                std::fs::write(path, "stale\n").unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
        install(tmp.path()).unwrap();
        for (path, content) in &files {
            assert_eq!(std::fs::read_to_string(path).unwrap(), *content);
        }
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
        assert!(
            git.is_ignored(".claude/skills/hub/setup-scripts.md")
                .unwrap()
        );
    }
}
