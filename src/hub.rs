use std::fs::File;
use std::path::{Path, PathBuf};

use fd_lock::RwLock;

use crate::env::Env;
use crate::error::{HubError, Result};
use crate::feature::{FEATURES_DIR, Feature};
use crate::git::Git;
use crate::manifest::{MANIFEST_FILE, Manifest};
use crate::tmux::{self, Tmux, TmuxOff};

const LOCK_FILE: &str = "hub.lock";
/// Directory under the git common dir that holds this machine's feature
/// records and the lock. Never committed, shared by every hub worktree.
const STATE_DIR: &str = "hub";

/// A located hub: its main clone, environment, manifest, and the feature the
/// current directory belongs to, if any.
#[derive(Debug, Clone)]
pub struct Hub {
    pub root: PathBuf,
    /// The repo's git common dir (`<root>/.git`); feature records live in
    /// `<git_dir>/hub/features/`.
    pub git_dir: PathBuf,
    pub env: Env,
    pub manifest: Manifest,
    /// `None` when sessions are off; `tmux_off` says why.
    pub tmux: Option<Tmux>,
    pub tmux_off: Option<TmuxOff>,
    /// Feature inferred from the hub worktree `cwd` is inside.
    pub cwd_feature: Option<String>,
    /// Every worktree of the hub repo as `git worktree list` reported it,
    /// main clone first; a path may no longer exist on disk.
    pub worktrees: Vec<PathBuf>,
}

/// True when `dir` holds at least one `*.json`: feature records a hub older
/// than 0.13.0 committed on `main`. A `.gitkeep` alone does not count.
fn has_feature_records(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        })
        .unwrap_or(false)
}

impl Hub {
    /// Walk up from `cwd` to its git repo, then to that repo's main worktree,
    /// which must hold `hub.json` and be on `main`.
    pub fn locate(cwd: &Path, env: Env) -> Result<Hub> {
        let toplevel = Git::new(cwd).toplevel().map_err(|_| {
            HubError::Usage(format!("{} is not inside a hub repository", cwd.display()))
        })?;
        let worktrees = Git::new(&toplevel).worktrees()?;
        let root = worktrees
            .first()
            .map(|w| w.path.clone())
            .ok_or_else(|| HubError::Precondition("git worktree list returned nothing".into()))?;
        let worktree_paths: Vec<PathBuf> = worktrees.iter().map(|w| w.path.clone()).collect();
        let manifest_path = root.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            return Err(HubError::Usage(format!(
                "no {MANIFEST_FILE} in {}; not a hub",
                root.display()
            )));
        }
        let root_branch = Git::new(&root).current_branch()?;
        if root_branch.as_deref() != Some("main") {
            return Err(HubError::Precondition(format!(
                "hub main clone {} must be on 'main', found {}",
                root.display(),
                root_branch.unwrap_or_else(|| "detached HEAD".into())
            )));
        }
        let legacy = root.join(FEATURES_DIR);
        if has_feature_records(&legacy) {
            return Err(HubError::Precondition(format!(
                "{} still has feature records in features/ on main; hub 0.13.0 keeps them under .git/hub/.\n\
                 Move them by hand:\n  \
                 cd {}\n  \
                 mkdir -p .git/hub/features && mv features/*.json .git/hub/features/\n  \
                 git rm -r -q features && git commit -q -m 'hub: move feature state out of git' -- features",
                root.display(),
                root.display()
            )));
        }
        let manifest = Manifest::load(&manifest_path)?;
        let git_dir = Git::new(&root).common_dir()?;
        let features_dir = git_dir.join(STATE_DIR).join(FEATURES_DIR);
        let cwd_feature = match Git::new(&toplevel).current_branch()? {
            Some(branch)
                if branch != "main" && features_dir.join(format!("{branch}.json")).is_file() =>
            {
                Some(branch)
            }
            _ => None,
        };
        let tmux_off = tmux::decide(
            env.tmux,
            tmux::on_path(std::env::var_os("PATH").as_deref()),
            &env.config_label,
        );
        let tmux = tmux_off.is_none().then(Tmux::default);
        Ok(Hub {
            root,
            git_dir,
            env,
            manifest,
            tmux,
            tmux_off,
            cwd_feature,
            worktrees: worktree_paths,
        })
    }

    /// The `tmux = true`-but-missing warning as zero or one output line,
    /// for commands that would have touched a session.
    pub fn tmux_warning(&self) -> Vec<String> {
        self.tmux_off
            .as_ref()
            .and_then(|off| off.warning.clone())
            .into_iter()
            .collect()
    }

    pub fn git(&self) -> Git {
        Git::new(&self.root)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    /// `<git_dir>/hub`: this machine's feature records and the lock.
    pub fn state_dir(&self) -> PathBuf {
        self.git_dir.join(STATE_DIR)
    }

    pub fn features_dir(&self) -> PathBuf {
        self.state_dir().join(FEATURES_DIR)
    }

    pub fn feature_path(&self, name: &str) -> PathBuf {
        self.features_dir().join(format!("{name}.json"))
    }

    /// Basename of the main clone; the hub's own worktrees live under `$GIT_WORKTREE_DIR/<this>/`.
    pub fn hub_dir_name(&self) -> String {
        self.root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn hub_worktree_path(&self, checkout: &str) -> PathBuf {
        self.env
            .worktree_dir
            .join(self.hub_dir_name())
            .join(checkout)
    }

    pub fn workspace_path(&self, feature: &Feature) -> PathBuf {
        self.hub_worktree_path(&feature.checkout)
            .join(format!("{}.code-workspace", self.manifest.name))
    }

    pub fn base_workspace_path(&self) -> PathBuf {
        self.root
            .join(format!("{}.code-workspace", self.manifest.name))
    }

    pub fn load_feature(&self, name: &str) -> Result<Feature> {
        let path = self.feature_path(name);
        if !path.is_file() {
            return Err(HubError::Usage(format!(
                "no feature named '{name}' in {}",
                self.root.display()
            )));
        }
        Feature::load(&path)
    }

    /// Every feature, sorted by name.
    pub fn list_features(&self) -> Result<Vec<Feature>> {
        let dir = self.features_dir();
        let mut features = Vec::new();
        if dir.is_dir() {
            let entries = std::fs::read_dir(&dir)
                .map_err(|e| HubError::io(format!("reading {}", dir.display()), e))?;
            for entry in entries {
                let path = entry
                    .map_err(|e| HubError::io(format!("reading {}", dir.display()), e))?
                    .path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    features.push(Feature::load(&path)?);
                }
            }
        }
        features.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(features)
    }

    /// `--feature` flag, else the feature of the current hub worktree.
    pub fn resolve_feature(&self, flag: Option<&str>) -> Result<Feature> {
        match flag.or(self.cwd_feature.as_deref()) {
            Some(name) => self.load_feature(name),
            None => Err(HubError::Usage(
                "not inside a feature worktree; pass --feature <name>".into(),
            )),
        }
    }

    /// Stage exactly `paths` and commit them alone (`git commit --only`) on
    /// main as `hub: <message>`; anything else staged is left staged. No-op
    /// when the paths are unchanged. A failed commit puts the paths back the
    /// way HEAD has them so no half-written state survives.
    /// Only `hub.json` goes through here; feature records are never committed.
    pub fn commit(&self, paths: &[&Path], message: &str) -> Result<()> {
        let git = self.git();
        git.add(paths)?;
        if !git.has_staged_changes(paths)? {
            return Ok(());
        }
        if let Err(err) = git.commit_paths(&format!("hub: {message}"), paths) {
            for path in paths {
                if let Err(undo) = git.discard(path) {
                    return Err(HubError::Precondition(format!(
                        "{err}\nand restoring {} failed: {undo}; run git status in {}",
                        path.display(),
                        self.root.display()
                    )));
                }
            }
            return Err(err);
        }
        Ok(())
    }

    /// Write the feature's record to the store. Never runs git: records are
    /// local to this machine. The write is atomic (temp file and rename), so
    /// a failure leaves the previous record, if any, exactly as it was.
    pub fn save_feature(&self, feature: &Feature) -> Result<()> {
        let dir = self.features_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| HubError::io(format!("creating {}", dir.display()), e))?;
        feature.save(&self.feature_path(&feature.name))
    }

    /// Write a replacement manifest and commit it.
    pub fn save_manifest(&self, manifest: &Manifest, message: &str) -> Result<()> {
        let path = self.manifest_path();
        manifest.save(&path)?;
        self.commit(&[&path], message)
    }

    /// Take the advisory lock in the hub's `.git`, reload `hub.json` under
    /// it, and run `f` with the fresh hub. Callers read feature files inside
    /// `f`, so a mutating command never acts on state another command changed
    /// while this one was starting. A second command fails immediately.
    pub fn with_lock<T>(mut self, f: impl FnOnce(&Hub) -> Result<T>) -> Result<T> {
        let state = self.state_dir();
        std::fs::create_dir_all(&state)
            .map_err(|e| HubError::io(format!("creating {}", state.display()), e))?;
        let lock_path = state.join(LOCK_FILE);
        let file = File::create(&lock_path)
            .map_err(|e| HubError::io(format!("creating {}", lock_path.display()), e))?;
        let mut lock = RwLock::new(file);
        let guard = lock.try_write().map_err(|_| {
            HubError::Precondition(format!(
                "another hub command is running (lock {})",
                lock_path.display()
            ))
        })?;
        self.manifest = Manifest::load(&self.manifest_path())?;
        let result = f(&self);
        drop(guard);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::feature::FeatureStatus;
    use crate::manifest::RepoSpec;
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        env: Env,
        hub_dir: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let projects = root.join("projects");
        let worktrees = root.join("worktrees");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&worktrees).unwrap();
        let env = Env::from_vars(Some(projects.clone().into()), Some(worktrees.into())).unwrap();
        let hub_dir = projects.join("acme");
        std::fs::create_dir_all(&hub_dir).unwrap();
        Git::init(&hub_dir, "main").unwrap();
        let git = Git::new(&hub_dir);
        git.config("user.name", "Test").unwrap();
        git.config("user.email", "test@example.com").unwrap();
        // A global core.hooksPath would bypass the failing hook some tests install.
        git.config("core.hooksPath", ".git/hooks").unwrap();
        Manifest::new("acme", None)
            .save(&hub_dir.join(MANIFEST_FILE))
            .unwrap();
        git.add(&[Path::new(".")]).unwrap();
        git.commit("init").unwrap();
        Fixture {
            _tmp: tmp,
            env,
            hub_dir,
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn locates_from_a_subdirectory() {
        let fx = fixture();
        let sub = fx.hub_dir.join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        let hub = Hub::locate(&sub, fx.env.clone()).unwrap();
        assert_eq!(hub.root, fx.hub_dir);
        assert_eq!(hub.git_dir, fx.hub_dir.join(".git"));
        assert_eq!(hub.features_dir(), fx.hub_dir.join(".git/hub/features"));
        assert_eq!(hub.manifest.name, "acme");
        assert_eq!(hub.cwd_feature, None);
        assert_eq!(hub.hub_dir_name(), "acme");
        assert_eq!(
            hub.hub_worktree_path("acme-x"),
            fx.env.worktree_dir.join("acme").join("acme-x")
        );
    }

    #[test]
    fn infers_feature_from_a_hub_worktree_through_the_common_dir() {
        let fx = fixture();
        let hub = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let feature = Feature::new("feat-1", "acme-feat_1");
        hub.save_feature(&feature).unwrap();
        let git = hub.git();
        git.create_branch("feat-1", "main").unwrap();
        let wt = hub.hub_worktree_path("acme-feat_1");
        git.worktree_add(&wt, "feat-1").unwrap();
        let from_wt = Hub::locate(&wt, fx.env.clone()).unwrap();
        assert_eq!(from_wt.root, fx.hub_dir);
        assert_eq!(
            from_wt.features_dir(),
            hub.features_dir(),
            "a worktree reaches the main clone's store"
        );
        assert_eq!(from_wt.cwd_feature.as_deref(), Some("feat-1"));
        assert_eq!(from_wt.resolve_feature(None).unwrap().name, "feat-1");
        assert_eq!(
            hub.resolve_feature(Some("feat-1")).unwrap().checkout,
            "acme-feat_1"
        );
        assert!(!wt.join("features").exists(), "nothing on the branch");
        let err = hub.resolve_feature(None).unwrap_err();
        assert!(matches!(err, HubError::Usage(m) if m.contains("--feature")));
        assert!(matches!(
            hub.resolve_feature(Some("nope")).unwrap_err(),
            HubError::Usage(_)
        ));
    }

    #[test]
    fn refuses_non_hubs_and_wrong_branch() {
        let fx = fixture();
        let plain = fx.env.project_home.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        Git::init(&plain, "main").unwrap();
        assert!(matches!(
            Hub::locate(&plain, fx.env.clone()).unwrap_err(),
            HubError::Usage(_)
        ));
        let nowhere = fx.env.project_home.clone();
        assert!(matches!(
            Hub::locate(&nowhere, fx.env.clone()).unwrap_err(),
            HubError::Usage(_)
        ));
        Git::new(&fx.hub_dir)
            .raw(&["checkout", "-q", "-b", "other"])
            .unwrap();
        let err = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap_err();
        assert!(
            matches!(err, HubError::Precondition(m) if m.contains("'main'") && m.contains("other"))
        );
    }

    #[test]
    fn refuses_a_hub_with_records_on_main_and_names_the_move() {
        let fx = fixture();
        let legacy = fx.hub_dir.join("features");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(".gitkeep"), "").unwrap();
        Hub::locate(&fx.hub_dir, fx.env.clone())
            .expect("an empty features/ from an old init is harmless");
        Feature::new("old", "old")
            .save(&legacy.join("old.json"))
            .unwrap();
        let git = Git::new(&fx.hub_dir);
        git.add(&[Path::new("features")]).unwrap();
        git.commit("legacy record").unwrap();
        let err = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap_err();
        match err {
            HubError::Precondition(m) => {
                assert!(m.contains("features/ on main"), "{m}");
                assert!(m.contains("mv features/*.json .git/hub/features/"), "{m}");
                assert!(m.contains("git rm -r -q features"), "{m}");
                assert!(m.contains(&format!("cd {}", fx.hub_dir.display())), "{m}");
                assert!(m.contains("-- features"), "{m}");
            }
            other => panic!("expected Precondition, got {other:?}"),
        }
    }

    #[test]
    fn save_feature_writes_under_git_hub_and_commits_nothing() {
        let fx = fixture();
        let hub = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let git = hub.git();
        hub.save_feature(&Feature::new("feat-1", "feat-1")).unwrap();
        assert!(fx.hub_dir.join(".git/hub/features/feat-1.json").is_file());
        assert_eq!(git.raw(&["log", "-1", "--format=%s"]).unwrap(), "init");
        assert_eq!(git.status_short().unwrap(), "", "main is untouched");
        assert_eq!(hub.list_features().unwrap().len(), 1);
    }

    #[test]
    fn save_manifest_commits_only_the_manifest() {
        let fx = fixture();
        let hub = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let git = hub.git();
        std::fs::write(fx.hub_dir.join("stray.txt"), "x").unwrap();
        std::fs::write(fx.hub_dir.join("staged.txt"), "y").unwrap();
        git.add(&[Path::new("staged.txt")]).unwrap();
        let mut manifest = hub.manifest.clone();
        manifest.repos.push(RepoSpec {
            role: "api".into(),
            clone: "api-clone".into(),
            remote: "git@example.com:api.git".into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        });
        hub.save_manifest(&manifest, "repo add api").unwrap();
        assert_eq!(
            git.raw(&["log", "-1", "--format=%s"]).unwrap(),
            "hub: repo add api"
        );
        assert_eq!(
            git.raw(&["show", "--name-only", "--format=", "HEAD"])
                .unwrap(),
            "hub.json"
        );
        assert_eq!(
            git.raw(&["diff", "--cached", "--name-only"]).unwrap(),
            "staged.txt",
            "unrelated staged work is left staged"
        );
        assert!(git.status_short().unwrap().contains("stray.txt"));
        hub.save_manifest(&manifest, "noop").unwrap();
        assert_eq!(
            git.raw(&["log", "-1", "--format=%s"]).unwrap(),
            "hub: repo add api",
            "a no-op save commits nothing, even with other work staged"
        );
    }

    #[test]
    fn a_failed_write_leaves_the_previous_record() {
        let fx = fixture();
        let hub = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let dir = hub.features_dir();
        std::fs::create_dir_all(&dir).unwrap();
        set_mode(&dir, 0o555);
        assert!(hub.save_feature(&Feature::new("feat-1", "feat-1")).is_err());
        assert!(!hub.feature_path("feat-1").exists(), "no new file");

        set_mode(&dir, 0o755);
        let mut feature = Feature::new("feat-1", "feat-1");
        hub.save_feature(&feature).unwrap();
        let written = std::fs::read_to_string(hub.feature_path("feat-1")).unwrap();
        set_mode(&dir, 0o555);
        feature.status = FeatureStatus::Finished;
        assert!(hub.save_feature(&feature).is_err());
        assert_eq!(
            std::fs::read_to_string(hub.feature_path("feat-1")).unwrap(),
            written,
            "the previous record is untouched"
        );
        assert!(!dir.join("feat-1.json.tmp").exists(), "no temp file left");
        set_mode(&dir, 0o755);
        assert_eq!(hub.git().status_short().unwrap(), "");
    }

    #[test]
    fn list_features_is_sorted() {
        let fx = fixture();
        let hub = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        hub.save_feature(&Feature::new("zeta", "zeta")).unwrap();
        hub.save_feature(&Feature::new("alpha", "alpha")).unwrap();
        let names: Vec<String> = hub
            .list_features()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn lock_is_exclusive_and_lives_under_git_hub() {
        let fx = fixture();
        let outer = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let inner = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let err = outer
            .with_lock(|_| inner.with_lock(|_| Ok(())))
            .unwrap_err();
        assert!(matches!(err, HubError::Precondition(m) if m.contains("another hub command")));
        assert!(fx.hub_dir.join(".git/hub/hub.lock").is_file());
        assert!(
            !fx.hub_dir.join(".git/hub.lock").exists(),
            "old location unused"
        );
        Hub::locate(&fx.hub_dir, fx.env.clone())
            .unwrap()
            .with_lock(|_| Ok(()))
            .unwrap();
    }

    #[test]
    fn with_lock_reloads_the_manifest() {
        let fx = fixture();
        let stale = Hub::locate(&fx.hub_dir, fx.env.clone()).unwrap();
        let mut manifest = stale.manifest.clone();
        manifest.repos.push(RepoSpec {
            role: "api".into(),
            clone: "api-clone".into(),
            remote: "git@example.com:api.git".into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        });
        Hub::locate(&fx.hub_dir, fx.env.clone())
            .unwrap()
            .save_manifest(&manifest, "repo add api")
            .unwrap();
        assert!(
            stale.manifest.repos.is_empty(),
            "read before the other command committed"
        );
        stale
            .with_lock(|fresh| {
                assert_eq!(fresh.manifest.repos.len(), 1);
                Ok(())
            })
            .unwrap();
    }
}
