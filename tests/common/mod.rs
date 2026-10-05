#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use hub::env::Env;
use tempfile::TempDir;

/// Temp world: `projects/` (PROJECT_HOME), `worktrees/` (GIT_WORKTREE_DIR),
/// `remotes/` (bare repos), fake tmux state and logs, an isolated git config,
/// and an empty `xdg/` as XDG_CONFIG_HOME so the developer's own
/// `hub/config.toml` never reaches the binary under test.
pub struct Fixture {
    _tmp: TempDir,
    pub root: PathBuf,
    pub project_home: PathBuf,
    pub worktree_dir: PathBuf,
    pub remotes: PathBuf,
    pub tmux_state: PathBuf,
    pub tmux_log: PathBuf,
    pub code_log: PathBuf,
    pub fake_bin: PathBuf,
    pub git_config: PathBuf,
    pub provider_dir: PathBuf,
    pub provider_log: PathBuf,
    pub xdg_config_home: PathBuf,
}

impl Fixture {
    pub fn new() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mk = |name: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        };
        let git_config = root.join("gitconfig");
        std::fs::write(
            &git_config,
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        Fixture {
            project_home: mk("projects"),
            worktree_dir: mk("worktrees"),
            remotes: mk("remotes"),
            tmux_state: mk("tmux"),
            tmux_log: root.join("tmux.log"),
            code_log: root.join("code.log"),
            fake_bin: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-bin"),
            git_config,
            provider_dir: mk("provider"),
            provider_log: root.join("provider.log"),
            xdg_config_home: mk("xdg"),
            root,
            _tmp: tmp,
        }
    }

    pub fn envs(&self) -> Vec<(&'static str, String)> {
        let path = format!(
            "{}:{}",
            self.fake_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        vec![
            ("PROJECT_HOME", self.project_home.display().to_string()),
            ("GIT_WORKTREE_DIR", self.worktree_dir.display().to_string()),
            ("FAKE_TMUX_STATE", self.tmux_state.display().to_string()),
            ("FAKE_TMUX_LOG", self.tmux_log.display().to_string()),
            ("FAKE_CODE_LOG", self.code_log.display().to_string()),
            ("FAKE_PROVIDER_DIR", self.provider_dir.display().to_string()),
            ("FAKE_PROVIDER_LOG", self.provider_log.display().to_string()),
            ("GIT_CONFIG_GLOBAL", self.git_config.display().to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
            (
                "XDG_CONFIG_HOME",
                self.xdg_config_home.display().to_string(),
            ),
            ("PATH", path),
        ]
    }

    /// Write `$XDG_CONFIG_HOME/hub/config.toml`.
    pub fn write_config(&self, text: &str) {
        let dir = self.xdg_config_home.join("hub");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// The real `hub` binary, run in `cwd` with the fixture environment.
    pub fn hub(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("hub").unwrap();
        cmd.current_dir(cwd).args(args);
        // The developer may run the suite inside tmux; the hub under test must not see that.
        cmd.env_remove("TMUX");
        for (key, value) in self.envs() {
            cmd.env(key, value);
        }
        cmd
    }

    /// Run git in `dir` with the fixture environment; panics on failure.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut cmd = StdCommand::new("git");
        cmd.current_dir(dir).args(args);
        for (key, value) in self.envs() {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "git {:?} in {} failed: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Bare remote `remotes/<clone>.git` with one commit on `base`, cloned to `projects/<clone>`.
    pub fn make_repo(&self, clone: &str, base: &str) -> PathBuf {
        let bare = self.remotes.join(format!("{clone}.git"));
        self.git(
            &self.remotes,
            &["init", "-q", "--bare", "-b", base, bare.to_str().unwrap()],
        );
        let seed = self.root.join(format!("seed-{clone}"));
        std::fs::create_dir_all(&seed).unwrap();
        self.git(&seed, &["init", "-q", "-b", base]);
        std::fs::write(seed.join("README.md"), format!("# {clone}\n")).unwrap();
        self.git(&seed, &["add", "."]);
        self.git(&seed, &["commit", "-q", "-m", "initial"]);
        self.git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
        self.git(&seed, &["push", "-q", "-u", "origin", base]);
        std::fs::remove_dir_all(&seed).unwrap();
        self.git(
            &self.project_home,
            &["clone", "-q", bare.to_str().unwrap(), clone],
        );
        self.project_home.join(clone)
    }

    /// Create `branch` on the remote (one commit past base) without leaving a local branch.
    pub fn remote_branch(&self, clone: &str, branch: &str) {
        let dir = self.project_home.join(clone);
        let base = self.git(&dir, &["symbolic-ref", "--short", "HEAD"]);
        self.git(&dir, &["checkout", "-q", "-b", branch]);
        self.commit_file(
            &dir,
            &format!("{}.txt", branch.replace('/', "-")),
            "remote work",
        );
        self.git(&dir, &["push", "-q", "origin", branch]);
        self.git(&dir, &["checkout", "-q", &base]);
        self.git(&dir, &["branch", "-D", branch]);
        self.git(&dir, &["fetch", "-q", "--prune", "origin"]);
    }

    /// Like `make_repo`, but the clone's origin is `remote` (a provider URL
    /// such as `https://gitlab.example.com/g/proj.git`) and the fixture's
    /// gitconfig rewrites that URL to the bare repo, so fetches work offline
    /// while `hub.json` records the provider identity.
    pub fn make_provider_repo(&self, clone: &str, base: &str, remote: &str) -> PathBuf {
        let bare = self.remotes.join(format!("{clone}.git"));
        let mut config = std::fs::read_to_string(&self.git_config).unwrap();
        config.push_str(&format!(
            "[url \"{}\"]\n\tinsteadOf = {remote}\n",
            bare.display()
        ));
        std::fs::write(&self.git_config, config).unwrap();
        let dir = self.make_repo(clone, base);
        self.git(&dir, &["remote", "set-url", "origin", remote]);
        dir
    }

    /// Store the fake provider's answer for `endpoint`
    /// (e.g. `projects/g%2Fproj/merge_requests/3`).
    pub fn provider_response(&self, endpoint: &str, body: &serde_json::Value) {
        let key = endpoint.replace('/', "__").replace('%', "_");
        std::fs::write(
            self.provider_dir.join(format!("{key}.json")),
            serde_json::to_string(body).unwrap(),
        )
        .unwrap();
    }

    /// Answer for `endpoint` from the second call on; the first call still
    /// gets what `provider_response` stored.
    pub fn provider_response_next(&self, endpoint: &str, body: &serde_json::Value) {
        let key = endpoint.replace('/', "__").replace('%', "_");
        std::fs::write(
            self.provider_dir.join(format!("{key}.next.json")),
            serde_json::to_string(body).unwrap(),
        )
        .unwrap();
    }

    pub fn provider_log(&self) -> String {
        std::fs::read_to_string(&self.provider_log).unwrap_or_default()
    }

    /// Commit id of `origin/<branch>` as the clone last fetched it.
    pub fn remote_sha(&self, clone: &str, branch: &str) -> String {
        let dir = self.project_home.join(clone);
        self.git(&dir, &["fetch", "-q", "--prune", "origin"]);
        self.git(&dir, &["rev-parse", &format!("origin/{branch}")])
    }

    pub fn commit_file(&self, dir: &Path, name: &str, message: &str) -> String {
        std::fs::write(dir.join(name), format!("{message}\n")).unwrap();
        self.git(dir, &["add", name]);
        self.git(dir, &["commit", "-q", "-m", message]);
        self.git(dir, &["rev-parse", "HEAD"])
    }

    pub fn init_hub(&self, name: &str) -> PathBuf {
        let dir = self.project_home.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        self.hub(&dir, &["init"]).assert().success();
        dir
    }

    pub fn add_repo(&self, hub_dir: &Path, role: &str, clone: &str) {
        self.hub(hub_dir, &["repo", "add", role, clone])
            .assert()
            .success();
    }

    pub fn env(&self) -> Env {
        Env::from_vars(
            Some(self.project_home.clone().into()),
            Some(self.worktree_dir.clone().into()),
        )
        .unwrap()
    }

    pub fn tmux_log(&self) -> String {
        std::fs::read_to_string(&self.tmux_log).unwrap_or_default()
    }

    pub fn has_session(&self, name: &str) -> bool {
        self.tmux_state.join(name).is_dir()
    }

    /// `(window name, directory)` pairs of a fake session, in order.
    pub fn windows(&self, session: &str) -> Vec<(String, String)> {
        let text = std::fs::read_to_string(self.tmux_state.join(session).join("windows"))
            .unwrap_or_default();
        text.lines()
            .map(|line| {
                let (name, dir) = line.split_once('\t').unwrap();
                (name.to_string(), dir.to_string())
            })
            .collect()
    }

    pub fn window_names(&self, session: &str) -> Vec<String> {
        self.windows(session).into_iter().map(|(n, _)| n).collect()
    }

    /// The feature record from this machine's store under the hub's `.git/hub/`.
    pub fn feature_json(&self, hub_dir: &Path, name: &str) -> serde_json::Value {
        let path = hub_dir
            .join(".git/hub/features")
            .join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// Make every commit in `dir` fail (a pre-commit hook) until `allow_commits`.
    pub fn block_commits(&self, dir: &Path) {
        let hook = dir.join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(
            &hook,
            "#!/bin/sh\necho 'test: commits blocked' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub fn allow_commits(&self, dir: &Path) {
        let _ = std::fs::remove_file(dir.join(".git/hooks/pre-commit"));
    }

    /// Make every feature-record write in `hub_dir` fail until
    /// `allow_feature_writes`: the store directory is read-only, so the
    /// temp file the atomic write starts with cannot be created.
    pub fn block_feature_writes(&self, hub_dir: &Path) {
        let dir = hub_dir.join(".git/hub/features");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    pub fn allow_feature_writes(&self, hub_dir: &Path) {
        let _ = std::fs::set_permissions(
            hub_dir.join(".git/hub/features"),
            std::fs::Permissions::from_mode(0o755),
        );
    }

    /// Append patterns to `dir`'s `.git/info/exclude` (shared by its worktrees).
    pub fn exclude_locally(&self, dir: &Path, patterns: &[&str]) {
        let exclude = dir.join(".git/info/exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
        for p in patterns {
            text.push_str(p);
            text.push('\n');
        }
        std::fs::write(&exclude, text).unwrap();
    }

    /// Leave `wt` clean but undeletable: an excluded, read-only `keep/`
    /// directory with a file in it. `git worktree remove` unregisters the
    /// worktree and then fails to delete the directory.
    pub fn block_deletion(&self, clone: &Path, wt: &Path) {
        self.exclude_locally(clone, &["keep/"]);
        let keep = wt.join("keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("blocker"), "x").unwrap();
        std::fs::set_permissions(&keep, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    pub fn unblock_deletion(&self, wt: &Path) {
        let _ = std::fs::set_permissions(wt.join("keep"), std::fs::Permissions::from_mode(0o755));
    }

    /// Simulate a pre-existing fake tmux session with the given windows.
    pub fn seed_session(&self, name: &str, windows: &[(&str, &str)]) {
        let dir = self.tmux_state.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let text: String = windows.iter().map(|(n, d)| format!("{n}\t{d}\n")).collect();
        std::fs::write(dir.join("windows"), text).unwrap();
    }

    /// A `PATH` with `git`, `sh`, and the fake `code`, but no `tmux`.
    pub fn path_without_tmux(&self) -> String {
        let bin = self.root.join("bin-no-tmux");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["git", "sh"] {
            let out = StdCommand::new("sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            let real = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let link = bin.join(tool);
            if !link.exists() {
                std::os::unix::fs::symlink(&real, &link).unwrap();
            }
        }
        let code = bin.join("code");
        if !code.exists() {
            std::os::unix::fs::symlink(self.fake_bin.join("code"), &code).unwrap();
        }
        bin.display().to_string()
    }
}
