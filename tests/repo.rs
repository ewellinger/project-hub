mod common;

use common::Fixture;
use predicates::prelude::*;

#[test]
fn add_infers_remote_and_base_and_commits() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.hub(
        &hub,
        &[
            "repo",
            "add",
            "api",
            "api-clone",
            "--description",
            "Agent platform",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Registered role 'api'"));
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hub.join("hub.json")).unwrap()).unwrap();
    let repo = &manifest["repos"][0];
    assert_eq!(repo["role"], "api");
    assert_eq!(repo["clone"], "api-clone");
    assert_eq!(repo["base"], "master");
    assert_eq!(repo["branch_template"], "{feature}");
    assert_eq!(repo["description"], "Agent platform");
    assert!(repo["remote"].as_str().unwrap().ends_with("api-clone.git"));
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo add api"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    let clone = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&clone, &["status", "--porcelain"]), "");
    assert_eq!(fx.git(&clone, &["branch", "--list"]).lines().count(), 1);
}

#[test]
fn add_honors_overrides_and_rejects_bad_input() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("ui-clone", "develop");
    fx.hub(
        &hub,
        &[
            "repo",
            "add",
            "ui",
            "ui-clone",
            "--base",
            "release",
            "--branch-template",
            "pc_{feature}-ew",
        ],
    )
    .assert()
    .success();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hub.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["repos"][0]["base"], "release");
    assert_eq!(manifest["repos"][0]["branch_template"], "pc_{feature}-ew");

    fx.hub(&hub, &["repo", "add", "ui", "ui-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("duplicate role"));
    fx.hub(&hub, &["repo", "add", "ui2", "ui-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already registered"));
    fx.hub(&hub, &["repo", "add", "x", "missing"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a git repository"));
    fx.hub(&hub, &["repo", "add", "1bad", "ui-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid role"));
    fx.make_repo("shim", "main");
    fx.hub(
        &hub,
        &["repo", "add", "shim", "shim", "--branch-template", "nope"],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("{feature}"));
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hub.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["repos"].as_array().unwrap().len(), 1);
}

#[test]
fn add_rejects_roles_reserved_for_the_fixed_tmux_windows() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.hub(&hub, &["repo", "add", "hub", "api-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
    fx.hub(&hub, &["repo", "add", "ai", "api-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn list_shows_deployment_order_and_json() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    let out = fx
        .hub(&hub, &["repo", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("ROLE"));
    assert!(lines[1].starts_with("api"));
    assert!(lines[2].starts_with("ui"));
    let out = fx
        .hub(&hub, &["repo", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json[1]["role"], "ui");
}

#[test]
fn remove_unregisters_without_touching_the_clone() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    let clone = fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["repo", "remove", "api"]).assert().success();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hub.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["repos"].as_array().unwrap().len(), 0);
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo remove api"
    );
    assert!(clone.is_dir());
    fx.hub(&hub, &["repo", "remove", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown role"));
}

#[test]
fn a_failed_manifest_commit_leaves_hub_json_as_head_has_it() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.make_repo("ui-clone", "develop");
    let before = std::fs::read_to_string(hub.join("hub.json")).unwrap();
    fx.block_commits(&hub);
    fx.hub(&hub, &["repo", "add", "ui", "ui-clone"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("commits blocked"));
    assert_eq!(
        std::fs::read_to_string(hub.join("hub.json")).unwrap(),
        before,
        "restored from HEAD"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    fx.allow_commits(&hub);
    fx.hub(&hub, &["repo", "add", "ui", "ui-clone"])
        .assert()
        .success();
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo add ui"
    );
}

#[test]
fn commands_need_the_environment_and_a_hub() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.hub(&hub, &["repo", "list"])
        .env_remove("PROJECT_HOME")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("PROJECT_HOME"));
    fx.hub(&fx.project_home, &["repo", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not inside a hub"));
}

#[test]
fn the_config_file_supplies_the_roots_when_the_variables_are_unset() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    let without_vars = |args: &[&str]| {
        let mut cmd = fx.hub(&hub, args);
        cmd.env_remove("PROJECT_HOME")
            .env_remove("GIT_WORKTREE_DIR");
        cmd
    };
    without_vars(&["repo", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "PROJECT_HOME and GIT_WORKTREE_DIR are not set; create",
        ))
        .stderr(predicate::str::contains("/xdg/hub/config.toml"))
        .stderr(predicate::str::contains("project_home = \"~/workspace\""));

    fx.write_config(&format!(
        "project_home = \"{}\"\nworktree_dir = \"{}\"\n",
        fx.project_home.display(),
        fx.worktree_dir.display()
    ));
    without_vars(&["repo", "list"]).assert().success();
    let fresh = fx.project_home.join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    let mut init = fx.hub(&fresh, &["init"]);
    init.env_remove("PROJECT_HOME")
        .env_remove("GIT_WORKTREE_DIR");
    init.assert().success();
    fx.make_repo("ui-clone", "develop");
    without_vars(&["repo", "add", "ui", "ui-clone"])
        .assert()
        .success();
    without_vars(&["feature", "start", "feat-1", "--repo", "ui"])
        .assert()
        .success();
    assert!(fx.worktree_dir.join("ui-clone").join("feat-1").is_dir());

    fx.write_config("worktree_dir = \"/nope\"\nprojects_home = \"/x\"\n");
    without_vars(&["repo", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("/xdg/hub/config.toml:"))
        .stderr(predicate::str::contains("projects_home"));
}

#[test]
fn repo_add_records_a_provider_remote_that_git_rewrites_locally() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    let api = fx.make_provider_repo(
        "api-clone",
        "master",
        "https://gitlab.example.com/g/sub/proj.git",
    );
    fx.add_repo(&hub, "api", "api-clone");
    let json: serde_json::Value = serde_json::from_str(
        &String::from_utf8(
            fx.hub(&hub, &["repo", "list", "--json"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        json[0]["remote"],
        "https://gitlab.example.com/g/sub/proj.git"
    );
    fx.remote_branch("api-clone", "feature/x");
    assert_eq!(
        fx.remote_sha("api-clone", "feature/x").len(),
        40,
        "fetch goes through the rewrite"
    );
    assert_eq!(
        fx.git(&api, &["config", "--get", "remote.origin.url"]),
        "https://gitlab.example.com/g/sub/proj.git"
    );
}

fn manifest(hub: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(hub.join("hub.json")).unwrap()).unwrap()
}

#[test]
fn set_edits_fields_in_place() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.make_repo("bff-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(
        &hub,
        &[
            "repo",
            "add",
            "ui",
            "ui-clone",
            "--branch-template",
            "pc_{feature}",
        ],
    )
    .assert()
    .success();
    fx.add_repo(&hub, "bff", "bff-clone");
    fx.hub(
        &hub,
        &[
            "repo",
            "set",
            "ui",
            "--description",
            "Web UI",
            "--base",
            "release",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("description: \"\" -> \"Web UI\""))
    .stdout(predicate::str::contains("base: \"develop\" -> \"release\""));
    let m = manifest(&hub);
    let roles: Vec<&str> = m["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["api", "ui", "bff"]);
    assert_eq!(m["repos"][1]["description"], "Web UI");
    assert_eq!(m["repos"][1]["base"], "release");
    assert_eq!(m["repos"][1]["branch_template"], "pc_{feature}");
    assert_eq!(m["repos"][1]["clone"], "ui-clone");
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo set ui"
    );
}

#[test]
fn set_refuses_nothing_to_do_and_bad_values() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["repo", "set", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs at least one of"));
    fx.hub(&hub, &["repo", "set", "nope", "--base", "x"])
        .assert()
        .failure();
    fx.hub(&hub, &["repo", "set", "api", "--branch-template", "fixed"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must contain {feature}"));
    for base in ["origin/master", ""] {
        fx.hub(&hub, &["repo", "set", "api", "--base", base])
            .assert()
            .failure()
            .stderr(predicate::str::contains("branch name without origin/"));
    }
    let before = fx.git(&hub, &["rev-parse", "HEAD"]);
    fx.hub(&hub, &["repo", "set", "api", "--base", "master"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to commit"));
    assert_eq!(fx.git(&hub, &["rev-parse", "HEAD"]), before);
}

#[test]
fn set_clone_refreshes_remote_and_guards_open_changes() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-old", "master");
    fx.make_repo("api-new", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-old");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(&hub, &["repo", "set", "api", "--clone", "ui-clone"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already registered as role 'ui'"));
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "set", "api", "--clone", "api-new"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "feature 'feat-1' has an open change for role 'api'",
        ));
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "set", "api", "--clone", "api-new"])
        .assert()
        .success();
    let m = manifest(&hub);
    assert_eq!(m["repos"][0]["clone"], "api-new");
    assert!(
        m["repos"][0]["remote"]
            .as_str()
            .unwrap()
            .ends_with("api-new.git")
    );
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(
        f["changes"][0]["clone"], "api-old",
        "history keeps its clone"
    );
}

fn roles(hub: &std::path::Path) -> Vec<String> {
    manifest(hub)["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["role"].as_str().unwrap().to_string())
        .collect()
}

fn change_roles(fx: &Fixture, hub: &std::path::Path, feature: &str) -> Vec<(String, String)> {
    fx.feature_json(hub, feature)["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["role"].as_str().unwrap().to_string(),
                c["clone"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn rename_rewrites_the_manifest_records_window_and_workspace() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(&hub, &["feature", "start", "done-1", "--repo", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "done-1"])
        .assert()
        .success();
    fx.hub(
        &hub,
        &["feature", "start", "live", "--repo", "api", "--repo", "ui"],
    )
    .assert()
    .success();
    fx.hub(&hub, &["repo", "rename", "api", "platform"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Renamed 2 changes in 2 features"));
    assert_eq!(roles(&hub), ["platform", "ui"]);
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo rename api platform"
    );
    assert_eq!(change_roles(&fx, &hub, "done-1")[0].0, "platform");
    assert_eq!(change_roles(&fx, &hub, "live")[0].0, "platform");
    let checkout = fx.feature_json(&hub, "live")["checkout"]
        .as_str()
        .unwrap()
        .to_string();
    let windows = fx.window_names(&checkout);
    assert!(windows.contains(&"platform".to_string()), "{windows:?}");
    assert!(!windows.contains(&"api".to_string()), "{windows:?}");
    let workspace = fx
        .worktree_dir
        .join("acme")
        .join(&checkout)
        .join("acme.code-workspace");
    let text = std::fs::read_to_string(workspace).unwrap();
    assert!(text.contains("\"platform\""), "{text}");
    assert!(!text.contains("\"api\""), "{text}");
}

#[test]
fn a_failed_window_rename_warns_with_the_tmux_command_that_retries_it() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "live", "--repo", "api"])
        .assert()
        .success();
    let checkout = fx.feature_json(&hub, "live")["checkout"]
        .as_str()
        .unwrap()
        .to_string();
    fx.hub(&hub, &["repo", "rename", "api", "platform"])
        .env("FAKE_TMUX_FAIL", "rename-window")
        .assert()
        .success()
        .stdout(predicate::str::contains("warning:"))
        .stdout(predicate::str::contains(format!(
            "run `tmux rename-window -t ={checkout}:api platform` to retry"
        )));
}

#[test]
fn rename_refuses_unknown_taken_or_recorded_names() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(&hub, &["repo", "rename", "nope", "x"])
        .assert()
        .failure();
    fx.hub(&hub, &["repo", "rename", "api", "ui"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("'ui' is already registered"));
    fx.hub(&hub, &["repo", "rename", "api", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("'api' is already registered"));
    fx.hub(&hub, &["repo", "rename", "api", "hub"])
        .assert()
        .failure();
    fx.hub(
        &hub,
        &["repo", "rename", "api", "x", "--clone", "api-clone"],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains(
        "--clone only applies with --records-only",
    ));
    // History of a removed role named `old-ui` would merge with api's.
    fx.hub(&hub, &["feature", "start", "f1", "--repo", "ui"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "f1"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "rename", "ui", "old-ui"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "remove", "old-ui"])
        .assert()
        .success();
    let before = fx.git(&hub, &["rev-parse", "HEAD"]);
    fx.hub(&hub, &["repo", "rename", "api", "old-ui"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "feature 'f1' already records changes for role 'old-ui'",
        ));
    assert_eq!(fx.git(&hub, &["rev-parse", "HEAD"]), before);
}

#[test]
fn the_swap_keeps_order_and_history() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("ams-clone", "develop");
    fx.make_repo("check-old", "master");
    fx.make_repo("check-new", "master");
    fx.make_repo("bff-clone", "develop");
    fx.add_repo(&hub, "ui", "ams-clone");
    fx.add_repo(&hub, "ui-split", "check-old");
    fx.add_repo(&hub, "bff", "bff-clone");
    fx.hub(
        &hub,
        &[
            "feature", "start", "f1", "--repo", "ui", "--repo", "ui-split",
        ],
    )
    .assert()
    .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "f1"])
        .assert()
        .success();
    for args in [
        &["repo", "rename", "ui", "ui-legacy"][..],
        &["repo", "rename", "ui-split", "ui"][..],
        &["repo", "set", "ui", "--clone", "check-new"][..],
    ] {
        fx.hub(&hub, args).assert().success();
    }
    assert_eq!(roles(&hub), ["ui-legacy", "ui", "bff"]);
    assert_eq!(manifest(&hub)["repos"][1]["clone"], "check-new");
    let mut changes = change_roles(&fx, &hub, "f1");
    changes.sort();
    assert_eq!(
        changes,
        [
            ("ui".to_string(), "check-old".to_string()),
            ("ui-legacy".to_string(), "ams-clone".to_string())
        ]
    );
}

#[test]
fn records_only_catches_up_after_a_pulled_rename() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "live", "--repo", "api"])
        .assert()
        .success();
    // Another machine renamed the role; this one still has the old records.
    let record = hub.join(".git/hub/features/live.json");
    let stale = std::fs::read_to_string(&record).unwrap();
    fx.hub(&hub, &["repo", "rename", "api", "platform"])
        .assert()
        .success();
    std::fs::write(&record, stale).unwrap();
    let head = fx.git(&hub, &["rev-parse", "HEAD"]);
    fx.hub(
        &hub,
        &["repo", "rename", "api", "platform", "--records-only"],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Renamed 1 changes in 1 features"));
    assert_eq!(change_roles(&fx, &hub, "live")[0].0, "platform");
    assert_eq!(fx.git(&hub, &["rev-parse", "HEAD"]), head, "no commit");
    fx.hub(
        &hub,
        &["repo", "rename", "nope", "missing", "--records-only"],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains(
        "role 'missing' is not in hub.json",
    ));
}

#[test]
fn records_only_with_clone_repairs_a_remove_and_add() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("ams-clone", "develop");
    fx.make_repo("check-clone", "master");
    fx.add_repo(&hub, "ui", "ams-clone");
    fx.hub(&hub, &["feature", "start", "f1", "--repo", "ui"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "f1"])
        .assert()
        .success();
    // The old way: remove and re-add, leaving f1's `ui` pointing at ams-clone.
    fx.hub(&hub, &["repo", "remove", "ui"]).assert().success();
    fx.add_repo(&hub, "ui", "check-clone");
    fx.add_repo(&hub, "ui-legacy", "ams-clone");
    fx.hub(&hub, &["feature", "start", "f2", "--repo", "ui"])
        .assert()
        .success();
    fx.hub(
        &hub,
        &[
            "repo",
            "rename",
            "ui",
            "ui-legacy",
            "--records-only",
            "--clone",
            "ams-clone",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Renamed 1 changes in 1 features"));
    assert_eq!(
        change_roles(&fx, &hub, "f1"),
        [("ui-legacy".to_string(), "ams-clone".to_string())]
    );
    assert_eq!(
        change_roles(&fx, &hub, "f2"),
        [("ui".to_string(), "check-clone".to_string())]
    );
    // Nothing left to match: a no-op, not an error.
    fx.hub(
        &hub,
        &[
            "repo",
            "rename",
            "ui",
            "ui-legacy",
            "--records-only",
            "--clone",
            "ams-clone",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("No changes recorded for role ui"));
}

#[test]
fn records_only_refuses_two_open_changes_for_one_role() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(
        &hub,
        &["feature", "start", "f1", "--repo", "api", "--repo", "ui"],
    )
    .assert()
    .success();
    // A stray record names api's open change `legacy`.
    let record = hub.join(".git/hub/features/f1.json");
    let mut f: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    for c in f["changes"].as_array_mut().unwrap() {
        if c["role"] == "api" {
            c["role"] = "legacy".into();
        }
    }
    std::fs::write(&record, serde_json::to_string_pretty(&f).unwrap()).unwrap();
    fx.hub(&hub, &["repo", "rename", "legacy", "ui", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "feature 'f1' already has an open change for role 'ui'",
        ));
}

#[test]
fn an_interrupted_rename_is_finished_by_records_only() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "live", "--repo", "api"])
        .assert()
        .success();
    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["repo", "rename", "api", "platform"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "renamed in hub.json; finish the records with: hub repo rename api platform --records-only",
        ));
    assert_eq!(roles(&hub), ["platform"]);
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo rename api platform"
    );
    fx.allow_feature_writes(&hub);
    let head = fx.git(&hub, &["rev-parse", "HEAD"]);
    fx.hub(
        &hub,
        &["repo", "rename", "api", "platform", "--records-only"],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Renamed 1 changes in 1 features"));
    assert_eq!(fx.git(&hub, &["rev-parse", "HEAD"]), head, "no commit");
    assert_eq!(change_roles(&fx, &hub, "live")[0].0, "platform");
}

#[test]
fn records_only_refuses_two_open_changes_in_a_finished_feature() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(
        &hub,
        &["feature", "start", "f1", "--repo", "api", "--repo", "ui"],
    )
    .assert()
    .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "f1"])
        .assert()
        .success();
    let record = hub.join(".git/hub/features/f1.json");
    let mut f: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    for c in f["changes"].as_array_mut().unwrap() {
        if c["role"] == "api" {
            c["role"] = "legacy".into();
        }
    }
    std::fs::write(&record, serde_json::to_string_pretty(&f).unwrap()).unwrap();
    let before = std::fs::read_to_string(&record).unwrap();
    fx.hub(&hub, &["repo", "rename", "legacy", "ui", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "feature 'f1' already has an open change for role 'ui'",
        ));
    assert_eq!(std::fs::read_to_string(&record).unwrap(), before);
}

#[test]
fn records_only_refuses_the_same_role() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["repo", "rename", "api", "api", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "OLD and NEW are the same role 'api'",
        ));
}
