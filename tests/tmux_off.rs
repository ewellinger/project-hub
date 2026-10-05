mod common;

use common::Fixture;
use predicates::prelude::*;

/// Hub `acme` with `api` and `ui`, and `tmux = false` in the config.
fn world() -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::new();
    fx.write_config("tmux = false\n");
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    (fx, hub)
}

#[test]
fn start_add_and_finish_work_without_sessions() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux is off; no session created"))
        .stdout(predicate::str::contains("Attach with").not())
        .stdout(predicate::str::contains("warning").not());
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["status"], "open");
    fx.hub(&hub, &["feature", "add", "ui", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux is off; no session created"))
        .stdout(predicate::str::contains("warning").not());
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Killed tmux session").not());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
    assert_eq!(fx.tmux_log(), "", "tmux was never run");
}

#[test]
fn hub_tmux_refuses_and_names_the_reason() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tmux is off (tmux = false in"));
}

#[test]
fn a_missing_binary_turns_sessions_off_silently() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .env("PATH", fx.path_without_tmux())
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux is off; no session created"))
        .stdout(predicate::str::contains("warning").not());
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .env("PATH", fx.path_without_tmux())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "tmux is off (tmux is not on PATH)",
        ));
}

#[test]
fn tmux_true_with_a_missing_binary_warns_once() {
    let fx = Fixture::new();
    fx.write_config("tmux = true\n");
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    let out = fx
        .hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .env("PATH", fx.path_without_tmux())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out.matches("but tmux is not on PATH; skipping sessions")
            .count(),
        1,
        "{out}"
    );
    assert!(out.contains("tmux is off; no session created"), "{out}");
}

#[test]
fn status_omits_the_session_line() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Feature: feat-1"))
        .stdout(predicate::str::contains("tmux Session").not());
}
