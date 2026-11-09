//! Per-role setup scripts: which worktrees get one, running it, and its log.
//!
//! A role's `setup` in `hub.json` names a script, relative to the hub's main
//! clone, that runs inside each worktree hub creates for that role. The ops
//! that create worktrees know nothing about it: the CLI and the dashboard
//! build the jobs from the saved feature record after the hub lock is
//! released, so a slow `npm ci` never blocks another hub command.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::env::shorten_home;
use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner};
use crate::hub::Hub;
use crate::manifest::Manifest;
use crate::ops::session::change_path;

/// One script run: where, with what, and where its output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub role: String,
    /// Absolute path of the script.
    pub script: PathBuf,
    /// The path as `hub.json` spells it, for messages.
    pub script_label: String,
    /// The worktree; also the script's working directory.
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub log: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    Failed(i32),
    Signaled,
    CouldNotStart(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub role: String,
    pub status: Status,
    pub elapsed: Duration,
    pub log: PathBuf,
}

/// The open changes of `roles` that get a setup run, with the role's script
/// path, in manifest order. A role needs a `setup`, an open change, and a
/// worktree hub created (or adopted, with `include_adopted`); a change
/// living in the main clone never qualifies. Roles not in the manifest or
/// not in `roles` are skipped.
pub fn select_changes<'a>(
    manifest: &Manifest,
    feature: &'a Feature,
    roles: &[String],
    include_adopted: bool,
) -> Vec<(&'a Change, String)> {
    manifest
        .repos
        .iter()
        .filter(|repo| roles.contains(&repo.role))
        .filter_map(|repo| {
            let setup = repo.setup.clone()?;
            let change = feature.open_change(&repo.role)?;
            let owner = &change.worktree.as_ref()?.owner;
            (*owner == Owner::Hub || include_adopted).then_some((change, setup))
        })
        .collect()
}

/// Jobs for `hub feature setup`: one named role, which may be an adopted
/// worktree, or every worktree hub created. A change living in the main
/// clone is refused rather than run, since that is the user's own checkout.
pub fn rerun_jobs(hub: &Hub, feature: &Feature, role: Option<&str>) -> Result<Vec<Job>> {
    feature.require_open()?;
    let Some(role) = role else {
        let all: Vec<String> = feature
            .open_changes()
            .iter()
            .map(|c| c.role.clone())
            .collect();
        return Ok(jobs(hub, feature, &all, false));
    };
    let repo = hub.manifest.require_repo(role)?;
    let Some(setup) = &repo.setup else {
        return Err(HubError::Usage(format!(
            "role '{role}' has no setup script in hub.json; set one with `hub repo set {role} --setup PATH`"
        )));
    };
    let Some(change) = feature.open_change(role) else {
        return Err(HubError::Precondition(format!(
            "role '{role}' has no open change in feature '{}'",
            feature.name
        )));
    };
    if change.worktree.is_none() {
        return Err(HubError::Precondition(format!(
            "{role} is checked out in the main clone at {}; run {setup} there yourself",
            change_path(hub, change).display()
        )));
    }
    Ok(jobs(hub, feature, &[role.to_string()], true))
}

pub fn log_path(hub: &Hub, feature: &str, role: &str) -> PathBuf {
    hub.state_dir()
        .join("setup")
        .join(feature)
        .join(format!("{role}.log"))
}

pub fn jobs(hub: &Hub, feature: &Feature, roles: &[String], include_adopted: bool) -> Vec<Job> {
    select_changes(&hub.manifest, feature, roles, include_adopted)
        .into_iter()
        .map(|(change, setup)| {
            let cwd = change_path(hub, change);
            let s = |p: &Path| p.display().to_string();
            Job {
                role: change.role.clone(),
                script: hub.root.join(&setup),
                script_label: setup,
                env: vec![
                    ("HUB_ROLE".into(), change.role.clone()),
                    ("HUB_FEATURE".into(), feature.name.clone()),
                    ("HUB_BRANCH".into(), change.branch.clone()),
                    ("HUB_WORKTREE".into(), s(&cwd)),
                    ("HUB_BASE".into(), s(&hub.env.clone_path(&change.clone))),
                    ("HUB_ROOT".into(), s(&hub.root)),
                ],
                log: log_path(hub, &feature.name, &change.role),
                cwd,
            }
        })
        .collect()
}

/// Run the script with stdout and stderr captured to the (truncated) log.
/// Every failure to get the script going is `CouldNotStart`, never a panic.
pub fn run(job: &Job) -> Outcome {
    let started = Instant::now();
    let status = match spawn_and_wait(job) {
        Ok(status) => status,
        Err(reason) => Status::CouldNotStart(reason),
    };
    Outcome {
        role: job.role.clone(),
        status,
        elapsed: started.elapsed(),
        log: job.log.clone(),
    }
}

fn spawn_and_wait(job: &Job) -> std::result::Result<Status, String> {
    let meta = std::fs::metadata(&job.script)
        .map_err(|e| format!("{} is not a file ({e})", job.script.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", job.script.display()));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(format!("{} is not executable", job.script.display()));
    }
    if let Some(parent) = job.log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let opening = |e: std::io::Error| format!("opening {}: {e}", job.log.display());
    // Not truncated until locked: a second run of the same role (dashboard and
    // CLI at once) must neither wipe the first one's log nor share its worktree.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&job.log)
        .map_err(opening)?;
    let mut lock = fd_lock::RwLock::new(file);
    let log = lock
        .try_write()
        .map_err(|_| format!("setup already running for {}", job.role))?;
    log.set_len(0).map_err(opening)?;
    let out = log.try_clone().map_err(opening)?;
    let err = log.try_clone().map_err(opening)?;
    let status = Command::new(&job.script)
        .current_dir(&job.cwd)
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .status()
        .map_err(|e| format!("running {}: {e}", job.script.display()))?;
    Ok(match status.code() {
        Some(0) => Status::Ok,
        Some(code) => Status::Failed(code),
        None => Status::Signaled,
    })
}

/// The last `n` lines of the log, lossily decoded; empty when unreadable.
pub fn tail(log: &Path, n: usize) -> Vec<String> {
    let bytes = std::fs::read(log).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

pub fn running_line(job: &Job) -> String {
    format!("Running setup for {}: {}", job.role, job.script_label)
}

/// The result as the CLI prints it: one line, plus the indented log tail
/// after a failure.
pub fn result_lines(outcome: &Outcome, tail_lines: usize, home: Option<&Path>) -> Vec<String> {
    let secs = outcome.elapsed.as_secs_f64();
    let log = shorten_home(&outcome.log, home);
    let role = &outcome.role;
    let (head, with_tail) = match &outcome.status {
        Status::Ok => (format!("  {role}: setup ok ({secs:.1}s)"), false),
        Status::Failed(code) => (
            format!("  {role}: setup failed (exit {code}) after {secs:.1}s; log {log}"),
            true,
        ),
        Status::Signaled => (
            format!("  {role}: setup killed by a signal after {secs:.1}s; log {log}"),
            true,
        ),
        Status::CouldNotStart(reason) => {
            (format!("  {role}: setup could not start: {reason}"), false)
        }
    };
    let mut lines = vec![head];
    if with_tail {
        lines.extend(
            tail(&outcome.log, tail_lines)
                .into_iter()
                .map(|l| format!("    {l}")),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::feature::{Change, Feature, Owner, Stage, Worktree};
    use crate::manifest::{Manifest, RepoSpec};

    fn spec(role: &str, setup: Option<&str>) -> RepoSpec {
        RepoSpec {
            role: role.into(),
            clone: format!("{role}-clone"),
            remote: "git@example.com:x.git".into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
            setup: setup.map(str::to_string),
        }
    }

    fn change(role: &str, worktree: Option<Owner>) -> Change {
        Change {
            role: role.into(),
            clone: format!("{role}-clone"),
            branch: "feat-1".into(),
            worktree: worktree.map(|owner| Worktree {
                name: "feat-1".into(),
                owner,
            }),
            stage: Stage::Working,
            review_url: None,
            merged_at: None,
            dropped_at: None,
            origin_seen: false,
            base: None,
            base_sha: None,
        }
    }

    fn world() -> (Manifest, Feature) {
        let mut m = Manifest::new("acme", None);
        m.repos.push(spec("api", Some("scripts/api.sh")));
        m.repos.push(spec("ui", Some("scripts/ui.sh")));
        m.repos.push(spec("bff", None));
        m.repos.push(spec("db", Some("scripts/db.sh")));
        let mut f = Feature::new("feat-1", "feat-1");
        f.push_change(change("ui", Some(Owner::Hub))).unwrap();
        f.push_change(change("api", Some(Owner::Hub))).unwrap();
        f.push_change(change("bff", Some(Owner::Hub))).unwrap();
        f.push_change(change("db", None)).unwrap();
        (m, f)
    }

    fn roles(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn selects_hub_owned_changes_with_a_setup_in_manifest_order() {
        let (m, f) = world();
        let picked = select_changes(&m, &f, &roles(&["db", "bff", "ui", "api"]), false);
        let got: Vec<(&str, &str)> = picked
            .iter()
            .map(|(c, s)| (c.role.as_str(), s.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![("api", "scripts/api.sh"), ("ui", "scripts/ui.sh")],
            "bff has no setup, db is in the main clone"
        );
        assert_eq!(select_changes(&m, &f, &roles(&["ui"]), false).len(), 1);
    }

    #[test]
    fn adopted_worktrees_only_when_asked() {
        let (m, mut f) = world();
        f.changes[0].worktree.as_mut().unwrap().owner = Owner::Adopted;
        assert!(select_changes(&m, &f, &roles(&["ui"]), false).is_empty());
        assert_eq!(select_changes(&m, &f, &roles(&["ui"]), true).len(), 1);
    }

    #[test]
    fn a_role_missing_from_the_manifest_is_skipped() {
        let (m, mut f) = world();
        f.push_change(change("gone", Some(Owner::Hub))).unwrap();
        assert!(select_changes(&m, &f, &roles(&["gone"]), true).is_empty());
    }

    fn script(dir: &std::path::Path, body: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.join("setup.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn job(dir: &std::path::Path, script: std::path::PathBuf) -> Job {
        let cwd = dir.join("wt");
        std::fs::create_dir_all(&cwd).unwrap();
        Job {
            role: "api".into(),
            script,
            script_label: "scripts/api.sh".into(),
            cwd,
            env: vec![
                ("HUB_ROLE".into(), "api".into()),
                ("HUB_FEATURE".into(), "feat-1".into()),
            ],
            log: dir.join("logs").join("feat-1").join("api.log"),
        }
    }

    #[test]
    fn run_passes_the_environment_and_captures_output() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(
            dir.path(),
            "echo hi\necho \"$HUB_ROLE $HUB_FEATURE\" > seen.txt\npwd >> seen.txt\n",
            0o755,
        );
        let j = job(dir.path(), s);
        let out = run(&j);
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.role, "api");
        assert_eq!(out.log, j.log);
        assert_eq!(std::fs::read_to_string(&j.log).unwrap(), "hi\n");
        let seen = std::fs::read_to_string(j.cwd.join("seen.txt")).unwrap();
        let cwd = j.cwd.canonicalize().unwrap();
        assert_eq!(seen, format!("api feat-1\n{}\n", cwd.display()));
    }

    #[test]
    fn a_second_run_of_the_same_log_could_not_start_and_keeps_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "echo new\n", 0o755);
        let j = job(dir.path(), s);
        std::fs::create_dir_all(j.log.parent().unwrap()).unwrap();
        std::fs::write(&j.log, "first run\n").unwrap();
        let mut held = fd_lock::RwLock::new(std::fs::File::open(&j.log).unwrap());
        let guard = held.write().unwrap();
        let out = run(&j);
        assert!(
            matches!(out.status, Status::CouldNotStart(ref r) if r == "setup already running for api"),
            "{out:?}"
        );
        assert_eq!(std::fs::read_to_string(&j.log).unwrap(), "first run\n");
        drop(guard);
        assert_eq!(run(&j).status, Status::Ok);
        assert_eq!(std::fs::read_to_string(&j.log).unwrap(), "new\n");
    }

    #[test]
    fn a_background_process_left_by_the_script_does_not_keep_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        // The sleep inherits the locked log as stdout and outlives the script;
        // the guard's explicit unlock on drop frees the lock anyway.
        let s = script(dir.path(), "sleep 5 &\necho done\n", 0o755);
        let j = job(dir.path(), s);
        assert_eq!(run(&j).status, Status::Ok);
        assert_eq!(run(&j).status, Status::Ok, "rerun while the sleep lives");
        assert_eq!(std::fs::read_to_string(&j.log).unwrap(), "done\n");
    }

    #[test]
    fn exit_code_and_stderr_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "echo boom >&2\nexit 3\n", 0o755);
        let out = run(&job(dir.path(), s));
        assert_eq!(out.status, Status::Failed(3));
        assert_eq!(tail(&out.log, 20), vec!["boom".to_string()]);
    }

    #[test]
    fn a_missing_non_executable_or_directory_script_could_not_start() {
        let dir = tempfile::tempdir().unwrap();
        let missing = run(&job(dir.path(), dir.path().join("nope.sh")));
        assert!(
            matches!(missing.status, Status::CouldNotStart(ref r) if r.contains("not a file")),
            "{missing:?}"
        );
        let s = script(dir.path(), "exit 0\n", 0o644);
        let plain = run(&job(dir.path(), s));
        assert!(
            matches!(plain.status, Status::CouldNotStart(ref r) if r.contains("not executable")),
            "{plain:?}"
        );
        let directory = run(&job(dir.path(), dir.path().to_path_buf()));
        assert!(
            matches!(directory.status, Status::CouldNotStart(_)),
            "{directory:?}"
        );
    }

    #[test]
    fn an_unwritable_log_could_not_start() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "exit 0\n", 0o755);
        let mut j = job(dir.path(), s);
        std::fs::write(dir.path().join("blocker"), "").unwrap();
        j.log = dir.path().join("blocker").join("api.log");
        assert!(matches!(run(&j).status, Status::CouldNotStart(_)));
    }

    #[test]
    fn tail_is_lossy_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("x.log");
        std::fs::write(&log, b"ok\n\xff\xfe\n").unwrap();
        assert_eq!(tail(&log, 1), vec!["\u{FFFD}\u{FFFD}".to_string()]);
        assert_eq!(tail(&log, 5).len(), 2);
        assert!(tail(&dir.path().join("absent.log"), 5).is_empty());
    }

    #[test]
    fn result_lines_render_each_status() {
        let home = std::path::Path::new("/Users/me");
        let log = std::path::PathBuf::from("/Users/me/acme/.git/hub/setup/feat-1/api.log");
        let outcome = |status| Outcome {
            role: "api".into(),
            status,
            elapsed: std::time::Duration::from_millis(1250),
            log: log.clone(),
        };
        assert_eq!(
            result_lines(&outcome(Status::Ok), 20, Some(home)),
            vec!["  api: setup ok (1.2s)"]
        );
        assert_eq!(
            result_lines(&outcome(Status::Failed(1)), 20, Some(home)),
            vec![
                "  api: setup failed (exit 1) after 1.2s; log ~/acme/.git/hub/setup/feat-1/api.log"
            ]
        );
        assert_eq!(
            result_lines(&outcome(Status::Signaled), 20, Some(home)),
            vec![
                "  api: setup killed by a signal after 1.2s; log ~/acme/.git/hub/setup/feat-1/api.log"
            ]
        );
        assert_eq!(
            result_lines(
                &outcome(Status::CouldNotStart("nope".into())),
                20,
                Some(home)
            ),
            vec!["  api: setup could not start: nope"]
        );
    }

    #[test]
    fn result_lines_indent_the_log_tail() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("api.log");
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
        let outcome = Outcome {
            role: "api".into(),
            status: Status::Failed(2),
            elapsed: std::time::Duration::from_secs(1),
            log,
        };
        let lines = result_lines(&outcome, 2, None);
        assert_eq!(lines.len(), 3);
        assert_eq!(&lines[1..], &["    two", "    three"]);
    }
}
