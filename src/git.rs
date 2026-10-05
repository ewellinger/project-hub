use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{HubError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name, `None` when detached.
    pub branch: Option<String>,
}

/// Outcome of `worktree remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRemoval {
    Removed,
    /// Git unregistered the worktree but could not delete its directory;
    /// carries git's own explanation.
    ResidueLeft(String),
}

/// Runs `git` in one directory. Holds no business logic.
#[derive(Debug, Clone)]
pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn output(&self, args: &[&OsStr]) -> Result<Output> {
        Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .output()
            .map_err(|e| HubError::io(format!("running git in {}", self.dir.display()), e))
    }

    fn describe(&self, args: &[&OsStr]) -> String {
        let words: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        format!("git {} (in {})", words.join(" "), self.dir.display())
    }

    fn run_os(&self, args: &[&OsStr]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(HubError::Command {
                command: self.describe(args),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let os: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.run_os(&os)
    }

    fn succeeds(&self, args: &[&str]) -> Result<bool> {
        let os: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        Ok(self.output(&os)?.status.success())
    }

    pub fn init(dir: &Path, branch: &str) -> Result<()> {
        Git::new(dir).run(&["init", "-q", "-b", branch]).map(|_| ())
    }

    pub fn is_repo(&self) -> bool {
        self.dir.is_dir() && self.succeeds(&["rev-parse", "--git-dir"]).unwrap_or(false)
    }

    pub fn toplevel(&self) -> Result<PathBuf> {
        self.run(&["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    /// The repository's common directory (the main worktree's `.git`),
    /// absolute. Every linked worktree of a repo reports the same path.
    pub fn common_dir(&self) -> Result<PathBuf> {
        self.run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .map(PathBuf::from)
    }

    pub fn current_branch(&self) -> Result<Option<String>> {
        let out = self.output(&[
            OsStr::new("symbolic-ref"),
            OsStr::new("--short"),
            OsStr::new("-q"),
            OsStr::new("HEAD"),
        ])?;
        if out.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    /// The URL as configured (`remote.<name>.url`), never the
    /// `url.<base>.insteadOf` expansion git would connect through, so the
    /// value identifies the repository the same way its hosting provider does.
    ///
    /// With several `remote.<name>.url` values configured, `config --get`
    /// returns the last one, where `remote get-url` returned the first.
    pub fn remote_url(&self, remote: &str) -> Result<Option<String>> {
        let out = self.output(&[
            OsStr::new("config"),
            OsStr::new("--get"),
            OsStr::new(&format!("remote.{remote}.url")),
        ])?;
        if out.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    pub fn has_remotes(&self) -> Result<bool> {
        Ok(!self.run(&["remote"])?.is_empty())
    }

    /// Fetch with prune so a branch deleted on origin disappears locally.
    pub fn fetch(&self, remote: &str) -> Result<()> {
        self.run(&["fetch", "-q", "--prune", remote]).map(|_| ())
    }

    pub fn ref_exists(&self, refname: &str) -> Result<bool> {
        self.succeeds(&["show-ref", "--verify", "--quiet", refname])
    }

    pub fn local_branch_exists(&self, branch: &str) -> Result<bool> {
        self.ref_exists(&format!("refs/heads/{branch}"))
    }

    pub fn remote_branch_exists(&self, branch: &str) -> Result<bool> {
        self.ref_exists(&format!("refs/remotes/origin/{branch}"))
    }

    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        self.run(&["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")])
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.succeeds(&["merge-base", "--is-ancestor", ancestor, descendant])
    }

    /// New branch at `start` with no upstream, so a later push targets the branch's own name.
    pub fn create_branch(&self, name: &str, start: &str) -> Result<()> {
        self.run(&["branch", "--no-track", name, start]).map(|_| ())
    }

    pub fn create_tracking_branch(&self, name: &str) -> Result<()> {
        self.run(&["branch", "--track", name, &format!("origin/{name}")])
            .map(|_| ())
    }

    pub fn force_branch(&self, name: &str, target: &str) -> Result<()> {
        self.run(&["branch", "-f", name, target]).map(|_| ())
    }

    /// Only for rolling back a branch created by the same invocation.
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        self.run(&["branch", "-D", name]).map(|_| ())
    }

    /// Move `refname` to `new` only if it still points at `old`; git refuses
    /// otherwise, so a ref another process moved meanwhile is never clobbered.
    pub fn update_ref_if(&self, refname: &str, new: &str, old: &str) -> Result<()> {
        self.run(&["update-ref", refname, new, old]).map(|_| ())
    }

    /// Delete `refname` only if it still points at `old`.
    pub fn delete_ref_if(&self, refname: &str, old: &str) -> Result<()> {
        self.run(&["update-ref", "-d", refname, old]).map(|_| ())
    }

    pub fn worktrees(&self) -> Result<Vec<WorktreeEntry>> {
        let text = self.run(&["worktree", "list", "--porcelain"])?;
        let mut entries = Vec::new();
        for block in text.split("\n\n") {
            let mut path = None;
            let mut head = String::new();
            let mut branch = None;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    head = h.to_string();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
                }
            }
            if let Some(path) = path {
                entries.push(WorktreeEntry { path, head, branch });
            }
        }
        Ok(entries)
    }

    /// Full ref names under `patterns`, e.g. `refs/heads/x` and
    /// `refs/remotes/origin/x`, in git's own (sorted) order.
    pub fn refnames(&self, patterns: &[&str]) -> Result<Vec<String>> {
        let mut args = vec!["for-each-ref", "--format=%(refname)"];
        args.extend_from_slice(patterns);
        Ok(self
            .run(&args)?
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn worktree_add(&self, path: &Path, branch: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HubError::io(format!("creating {}", parent.display()), e))?;
        }
        self.run_os(&[
            OsStr::new("worktree"),
            OsStr::new("add"),
            path.as_os_str(),
            OsStr::new(branch),
        ])
        .map(|_| ())
    }

    /// Remove a worktree. Git unregisters it before deleting the directory
    /// and reports failure if any entry survives (a process recreating files,
    /// an unwritable subdirectory), so a failed command may still have
    /// unregistered the worktree: that is `ResidueLeft`, not an error. A
    /// failure that leaves the worktree registered is returned as-is.
    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<WorktreeRemoval> {
        let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
        if force {
            args.push(OsStr::new("--force"));
        }
        args.push(path.as_os_str());
        match self.run_os(&args) {
            Ok(_) => Ok(WorktreeRemoval::Removed),
            Err(err) => {
                if self.worktrees()?.iter().any(|w| same_path(&w.path, path)) {
                    return Err(err);
                }
                if !path.exists() {
                    return Ok(WorktreeRemoval::Removed);
                }
                let reason = match &err {
                    HubError::Command { stderr, .. } => stderr.clone(),
                    other => other.to_string(),
                };
                Ok(WorktreeRemoval::ResidueLeft(reason))
            }
        }
    }

    pub fn status_porcelain(&self) -> Result<String> {
        self.run(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])
    }

    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.status_porcelain()?.is_empty())
    }

    pub fn status_short(&self) -> Result<String> {
        self.run(&["status", "--short", "--untracked-files=all"])
    }

    /// `(ahead, behind)` of `branch` relative to `origin/<branch>`, or `None` without a remote branch.
    pub fn ahead_behind(&self, branch: &str) -> Result<Option<(u64, u64)>> {
        if !self.remote_branch_exists(branch)? {
            return Ok(None);
        }
        let text = self.run(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{branch}...origin/{branch}"),
        ])?;
        let mut parts = text.split_whitespace();
        let ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        Ok(Some((ahead, behind)))
    }

    /// `git merge --ff-only <target>` in this checkout: moves the checked-out
    /// branch and working tree to `target` when it is a descendant of HEAD,
    /// and fails without touching anything otherwise.
    pub fn merge_ff_only(&self, target: &str) -> Result<()> {
        self.run(&["merge", "-q", "--ff-only", target]).map(|_| ())
    }

    /// Merge base of two commits, or `None` when they share no history.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let args = [OsStr::new("merge-base"), OsStr::new(a), OsStr::new(b)];
        let out = self.output(&args)?;
        match out.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            )),
            Some(1) => Ok(None),
            _ => Err(HubError::Command {
                command: self.describe(&args),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            }),
        }
    }

    /// Commits reachable from `target` but not from `branch`:
    /// `git rev-list --count <branch>..<target>`.
    pub fn missing_commits(&self, branch: &str, target: &str) -> Result<u64> {
        let text = self.run(&["rev-list", "--count", &format!("{branch}..{target}")])?;
        text.trim().parse().map_err(|_| {
            HubError::Precondition(format!(
                "unexpected rev-list output '{text}' in {}",
                self.dir.display()
            ))
        })
    }

    pub fn check_ref_format(&self, branch: &str) -> Result<bool> {
        self.succeeds(&["check-ref-format", "--branch", branch])
    }

    /// Branch `origin/HEAD` points at, without the `origin/` prefix.
    pub fn default_remote_branch(&self) -> Result<Option<String>> {
        let out = self.output(&[
            OsStr::new("symbolic-ref"),
            OsStr::new("--short"),
            OsStr::new("-q"),
            OsStr::new("refs/remotes/origin/HEAD"),
        ])?;
        if !out.status.success() {
            return Ok(None);
        }
        let full = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Some(
            full.strip_prefix("origin/").unwrap_or(&full).to_string(),
        ))
    }

    pub fn add(&self, paths: &[&Path]) -> Result<()> {
        let mut args = vec![OsStr::new("add"), OsStr::new("--")];
        args.extend(paths.iter().map(|p| p.as_os_str()));
        self.run_os(&args).map(|_| ())
    }

    /// Tracked files that the ignore rule `pattern` would match.
    pub fn tracked_matching(&self, pattern: &str) -> Result<Vec<String>> {
        let out = self.run(&["ls-files", "-ci", &format!("--exclude={pattern}")])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// Remove `paths` from the index, leaving them in the working tree.
    pub fn rm_cached(&self, paths: &[&Path]) -> Result<()> {
        let mut args = vec![
            OsStr::new("rm"),
            OsStr::new("-q"),
            OsStr::new("--cached"),
            OsStr::new("--"),
        ];
        args.extend(paths.iter().map(|p| p.as_os_str()));
        self.run_os(&args).map(|_| ())
    }

    /// True when the index differs from HEAD under any of `paths`.
    pub fn has_staged_changes(&self, paths: &[&Path]) -> Result<bool> {
        let mut args = vec![
            OsStr::new("diff"),
            OsStr::new("--cached"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
        ];
        args.extend(paths.iter().map(|p| p.as_os_str()));
        let out = self.output(&args)?;
        match out.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(HubError::Command {
                command: self.describe(&args),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            }),
        }
    }

    /// Commit everything currently staged (test fixtures only; hub state uses `commit_paths`).
    pub fn commit(&self, message: &str) -> Result<()> {
        self.run(&["commit", "-q", "-m", message]).map(|_| ())
    }

    /// Commit only `paths` (`git commit --only`), leaving anything else staged as it was.
    pub fn commit_paths(&self, message: &str, paths: &[&Path]) -> Result<()> {
        let mut args = vec![
            OsStr::new("commit"),
            OsStr::new("-q"),
            OsStr::new("--only"),
            OsStr::new("-m"),
            OsStr::new(message),
            OsStr::new("--"),
        ];
        args.extend(paths.iter().map(|p| p.as_os_str()));
        self.run_os(&args).map(|_| ())
    }

    /// Put `path` back the way HEAD has it: unstage it, then restore it from
    /// HEAD, or delete it when HEAD does not have it.
    pub fn discard(&self, path: &Path) -> Result<()> {
        self.run_os(&[
            OsStr::new("reset"),
            OsStr::new("-q"),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.dir.join(path)
        };
        let abs = abs.canonicalize().unwrap_or(abs);
        let toplevel = self.toplevel()?;
        let rel = abs.strip_prefix(&toplevel).map_err(|_| {
            HubError::Precondition(format!(
                "{} is not under the repository toplevel {}",
                abs.display(),
                toplevel.display()
            ))
        })?;
        let spec = format!("HEAD:{}", rel.display());
        if self.succeeds(&["cat-file", "-e", &spec])? {
            self.run_os(&[
                OsStr::new("checkout"),
                OsStr::new("-q"),
                OsStr::new("HEAD"),
                OsStr::new("--"),
                path.as_os_str(),
            ])
            .map(|_| ())
        } else if abs.exists() {
            std::fs::remove_file(&abs)
                .map_err(|e| HubError::io(format!("removing {}", abs.display()), e))
        } else {
            Ok(())
        }
    }

    /// True when `rel` (relative to the repo) matches an ignore rule,
    /// including `.git/info/exclude`.
    pub fn is_ignored(&self, rel: &str) -> Result<bool> {
        let args = [
            OsStr::new("check-ignore"),
            OsStr::new("-q"),
            OsStr::new("--"),
            OsStr::new(rel),
        ];
        let out = self.output(&args)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(HubError::Command {
                command: self.describe(&args),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            }),
        }
    }

    /// Add `pattern` to this repository's local exclude file when missing.
    /// The exclude file is shared by linked worktrees and is never committed.
    pub fn exclude_locally(&self, pattern: &str) -> Result<()> {
        let exclude = PathBuf::from(self.run(&["rev-parse", "--git-path", "info/exclude"])?);
        let exclude = if exclude.is_absolute() {
            exclude
        } else {
            self.dir.join(exclude)
        };
        let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
        if text.lines().any(|line| line.trim() == pattern) {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(pattern);
        text.push('\n');
        if let Some(parent) = exclude.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HubError::io(format!("creating {}", parent.display()), e))?;
        }
        std::fs::write(&exclude, text)
            .map_err(|e| HubError::io(format!("writing {}", exclude.display()), e))
    }

    pub fn config(&self, key: &str, value: &str) -> Result<()> {
        self.run(&["config", key, value]).map(|_| ())
    }

    /// Run an arbitrary git command and return trimmed stdout. Prefer the typed methods.
    pub fn raw(&self, args: &[&str]) -> Result<String> {
        self.run(args)
    }
}

/// Compare two paths by their canonical form, falling back to the raw paths.
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn configure(git: &Git) {
        git.config("user.name", "Test").unwrap();
        git.config("user.email", "test@example.com").unwrap();
    }

    fn commit_file(git: &Git, name: &str, content: &str) -> String {
        std::fs::write(git.dir().join(name), content).unwrap();
        git.add(&[Path::new(name)]).unwrap();
        git.commit(&format!("add {name}")).unwrap();
        git.rev_parse("HEAD").unwrap()
    }

    fn repo() -> (TempDir, Git) {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        Git::init(&dir, "main").unwrap();
        let git = Git::new(&dir);
        configure(&git);
        commit_file(&git, "README.md", "hi\n");
        (tmp, git)
    }

    /// Bare remote with `main`, plus a clone of it under `root/clone`.
    fn repo_with_remote() -> (TempDir, Git, Git) {
        let (tmp, seed) = repo();
        let root = tmp.path().canonicalize().unwrap();
        let bare = root.join("remote.git");
        Git::new(&root)
            .run(&["init", "-q", "--bare", "-b", "main", "remote.git"])
            .unwrap();
        seed.run(&["remote", "add", "origin", bare.to_str().unwrap()])
            .unwrap();
        seed.run(&["push", "-q", "-u", "origin", "main"]).unwrap();
        Git::new(&root)
            .run(&["clone", "-q", "remote.git", "clone"])
            .unwrap();
        let clone = Git::new(root.join("clone"));
        configure(&clone);
        (tmp, seed, clone)
    }

    #[test]
    fn detects_repo_branch_and_toplevel() {
        let (_tmp, git) = repo();
        assert!(git.is_repo());
        assert!(!Git::new(git.dir().join("nope")).is_repo());
        assert_eq!(git.current_branch().unwrap().as_deref(), Some("main"));
        let sub = git.dir().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(Git::new(&sub).toplevel().unwrap(), git.dir());
        git.run(&["checkout", "-q", "--detach"]).unwrap();
        assert_eq!(git.current_branch().unwrap(), None);
    }

    #[test]
    fn branch_queries_and_mutations() {
        let (_tmp, git) = repo();
        let first = git.rev_parse("HEAD").unwrap();
        assert!(!git.local_branch_exists("feat").unwrap());
        git.create_branch("feat", "main").unwrap();
        assert!(git.local_branch_exists("feat").unwrap());
        let second = commit_file(&git, "b.txt", "b\n");
        assert!(git.is_ancestor("feat", "main").unwrap());
        assert!(!git.is_ancestor("main", "feat").unwrap());
        git.force_branch("feat", "main").unwrap();
        assert_eq!(git.rev_parse("feat").unwrap(), second);
        git.force_branch("feat", &first).unwrap();
        assert_eq!(git.rev_parse("feat").unwrap(), first);
        git.delete_branch("feat").unwrap();
        assert!(!git.local_branch_exists("feat").unwrap());
        assert!(git.check_ref_format("feature/x-1").unwrap());
        assert!(!git.check_ref_format("bad..name").unwrap());
        assert!(git.rev_parse("nope").is_err());
    }

    #[test]
    fn worktrees_are_parsed_including_detached() {
        let (_tmp, git) = repo();
        git.create_branch("feat", "main").unwrap();
        let wt = git.dir().parent().unwrap().join("worktrees").join("feat");
        git.worktree_add(&wt, "feat").unwrap();
        let detached = git.dir().parent().unwrap().join("worktrees").join("det");
        git.run_os(&[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            detached.as_os_str(),
        ])
        .unwrap();
        let list = git.worktrees().unwrap();
        assert_eq!(list.len(), 3);
        assert!(same_path(&list[0].path, git.dir()));
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        // `git worktree list --porcelain` orders linked worktrees alphabetically
        // by id (2.50), not by insertion order, so look each up by path.
        let feat_entry = list.iter().find(|e| same_path(&e.path, &wt)).unwrap();
        assert_eq!(feat_entry.branch.as_deref(), Some("feat"));
        let detached_entry = list.iter().find(|e| same_path(&e.path, &detached)).unwrap();
        assert_eq!(detached_entry.branch, None);
        assert!(!detached_entry.head.is_empty());
        std::fs::write(wt.join("junk"), "x").unwrap();
        assert!(git.worktree_remove(&wt, false).is_err());
        git.worktree_remove(&wt, true).unwrap();
        assert_eq!(git.worktrees().unwrap().len(), 2);
    }

    #[test]
    fn merge_base_and_missing_commits() {
        let (_tmp, git) = repo();
        let root = git.rev_parse("HEAD").unwrap();
        git.create_branch("feat", "main").unwrap();
        commit_file(&git, "m.txt", "m\n");
        assert_eq!(
            git.merge_base("feat", "main").unwrap().as_deref(),
            Some(root.as_str())
        );
        assert_eq!(git.missing_commits("feat", "main").unwrap(), 1);
        assert_eq!(git.missing_commits("main", "feat").unwrap(), 0);

        git.run(&["checkout", "-q", "feat"]).unwrap();
        commit_file(&git, "f.txt", "f\n");
        assert_eq!(
            git.merge_base("feat", "main").unwrap().as_deref(),
            Some(root.as_str()),
            "the fork point, not either tip"
        );
        assert_eq!(git.missing_commits("feat", "main").unwrap(), 1);

        let empty_tree = git
            .run(&["hash-object", "-t", "tree", "/dev/null"])
            .unwrap();
        let island = git
            .run(&["commit-tree", &empty_tree, "-m", "island"])
            .unwrap();
        git.create_branch("island", &island).unwrap();
        assert_eq!(git.merge_base("island", "main").unwrap(), None);
        assert!(git.merge_base("nope", "main").is_err());
        assert!(git.missing_commits("nope", "main").is_err());
    }

    #[test]
    fn dirty_detection_includes_untracked() {
        let (_tmp, git) = repo();
        assert!(!git.is_dirty().unwrap());
        std::fs::write(git.dir().join("new.txt"), "x").unwrap();
        assert!(git.is_dirty().unwrap());
        assert!(git.status_short().unwrap().contains("new.txt"));
        assert!(!git.has_staged_changes(&[Path::new("new.txt")]).unwrap());
        git.add(&[Path::new("new.txt")]).unwrap();
        assert!(git.has_staged_changes(&[Path::new("new.txt")]).unwrap());
        assert!(
            !git.has_staged_changes(&[Path::new("README.md")]).unwrap(),
            "scoped to the given paths"
        );
        std::fs::write(git.dir().join(".gitignore"), "secret.txt\n").unwrap();
        assert!(git.is_ignored("secret.txt").unwrap());
        assert!(!git.is_ignored("new.txt").unwrap());
    }

    #[test]
    fn commit_paths_leaves_other_staged_files_alone_and_discard_restores_head() {
        let (_tmp, git) = repo();
        std::fs::write(git.dir().join("other.txt"), "o").unwrap();
        git.add(&[Path::new("other.txt")]).unwrap();
        std::fs::write(git.dir().join("state.json"), "1").unwrap();
        git.add(&[Path::new("state.json")]).unwrap();
        git.commit_paths("state only", &[Path::new("state.json")])
            .unwrap();
        assert_eq!(
            git.run(&["show", "--name-only", "--format=", "HEAD"])
                .unwrap(),
            "state.json"
        );
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "other.txt",
            "still staged"
        );

        std::fs::write(git.dir().join("state.json"), "2").unwrap();
        git.add(&[Path::new("state.json")]).unwrap();
        git.discard(Path::new("state.json")).unwrap();
        assert_eq!(
            std::fs::read_to_string(git.dir().join("state.json")).unwrap(),
            "1"
        );
        git.discard(Path::new("other.txt")).unwrap();
        assert!(
            !git.dir().join("other.txt").exists(),
            "not in HEAD, so deleted"
        );
        assert!(!git.is_dirty().unwrap());
    }

    #[test]
    fn remote_queries() {
        let (_tmp, seed, clone) = repo_with_remote();
        assert!(clone.has_remotes().unwrap());
        assert!(seed.has_remotes().unwrap());
        assert!(
            clone
                .remote_url("origin")
                .unwrap()
                .unwrap()
                .ends_with("remote.git")
        );
        assert_eq!(clone.remote_url("nope").unwrap(), None);
        clone
            .run(&[
                "config",
                "url.https://rewritten.example/x.git.insteadOf",
                "https://origin.example/x.git",
            ])
            .unwrap();
        clone
            .run(&["remote", "add", "provider", "https://origin.example/x.git"])
            .unwrap();
        assert_eq!(
            clone.remote_url("provider").unwrap().as_deref(),
            Some("https://origin.example/x.git"),
            "the configured URL, not the insteadOf expansion"
        );
        assert_eq!(
            clone.default_remote_branch().unwrap().as_deref(),
            Some("main")
        );

        seed.create_branch("shared", "main").unwrap();
        seed.run(&["push", "-q", "origin", "shared"]).unwrap();
        assert!(!clone.remote_branch_exists("shared").unwrap());
        clone.fetch("origin").unwrap();
        assert!(clone.remote_branch_exists("shared").unwrap());
        clone.create_tracking_branch("shared").unwrap();
        assert_eq!(clone.ahead_behind("shared").unwrap(), Some((0, 0)));

        seed.run(&["checkout", "-q", "shared"]).unwrap();
        commit_file(&seed, "c.txt", "c\n");
        seed.run(&["push", "-q", "origin", "shared"]).unwrap();
        clone.fetch("origin").unwrap();
        assert_eq!(clone.ahead_behind("shared").unwrap(), Some((0, 1)));
        assert_eq!(clone.ahead_behind("main").unwrap(), Some((0, 0)));
        assert_eq!(clone.ahead_behind("local-only").unwrap(), None);

        seed.run(&["push", "-q", "origin", "--delete", "shared"])
            .unwrap();
        clone.fetch("origin").unwrap();
        assert!(
            !clone.remote_branch_exists("shared").unwrap(),
            "prune removes the stale ref"
        );
    }

    #[test]
    fn merge_ff_only_advances_a_descendant_and_refuses_a_diverged_branch() {
        let (_tmp, seed, clone) = repo_with_remote();
        let pushed = commit_file(&seed, "a.txt", "a\n");
        seed.run(&["push", "-q", "origin", "main"]).unwrap();
        clone.fetch("origin").unwrap();
        clone.merge_ff_only("origin/main").unwrap();
        assert_eq!(clone.rev_parse("HEAD").unwrap(), pushed);
        assert!(
            clone.dir().join("a.txt").is_file(),
            "the working tree moved too"
        );
        clone.merge_ff_only("origin/main").unwrap();
        assert_eq!(
            clone.rev_parse("HEAD").unwrap(),
            pushed,
            "already there is fine"
        );

        let local = commit_file(&clone, "local.txt", "l\n");
        commit_file(&seed, "b.txt", "b\n");
        seed.run(&["push", "-q", "origin", "main"]).unwrap();
        clone.fetch("origin").unwrap();
        assert!(clone.merge_ff_only("origin/main").is_err());
        assert_eq!(clone.rev_parse("HEAD").unwrap(), local, "nothing moved");
        assert!(!clone.is_dirty().unwrap());
    }

    #[test]
    fn conditional_ref_updates_refuse_a_moved_ref() {
        let (_tmp, git) = repo();
        let first = git.rev_parse("HEAD").unwrap();
        let second = commit_file(&git, "b.txt", "b\n");
        git.create_branch("feat", &first).unwrap();
        git.update_ref_if("refs/heads/feat", &second, &first)
            .unwrap();
        assert_eq!(git.rev_parse("feat").unwrap(), second);
        assert!(
            git.update_ref_if("refs/heads/feat", &first, &first)
                .is_err(),
            "old value no longer matches"
        );
        assert_eq!(git.rev_parse("feat").unwrap(), second, "untouched");
        assert!(git.delete_ref_if("refs/heads/feat", &first).is_err());
        assert!(git.local_branch_exists("feat").unwrap());
        git.delete_ref_if("refs/heads/feat", &second).unwrap();
        assert!(!git.local_branch_exists("feat").unwrap());
    }

    #[test]
    fn discard_restores_a_committed_file_through_a_symlinked_dir() {
        let (_tmp, git) = repo();
        let real_dir = git.dir().to_path_buf();
        let link = real_dir.parent().unwrap().join("linked-repo");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let linked = Git::new(&link);
        commit_file(&linked, "tracked.txt", "committed\n");
        std::fs::write(link.join("tracked.txt"), "modified\n").unwrap();
        linked.add(&[Path::new("tracked.txt")]).unwrap();
        linked.discard(Path::new("tracked.txt")).unwrap();
        assert_eq!(
            std::fs::read_to_string(real_dir.join("tracked.txt")).unwrap(),
            "committed\n",
            "restored, not deleted"
        );
        assert!(real_dir.join("tracked.txt").exists());
    }

    #[test]
    fn same_path_sees_through_symlinks() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_path(&real, &link));
        assert!(!same_path(&real, tmp.path()));
    }
}
