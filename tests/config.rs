mod common;

use common::Fixture;
use predicates::prelude::*;

/// `hub` with HOME at the fixture root and neither root variable set: a
/// fresh install whose only configuration is the config file.
fn bare(fx: &Fixture, args: &[&str]) -> assert_cmd::Command {
    let mut cmd = fx.hub(&fx.root, args);
    cmd.env("HOME", &fx.root)
        .env_remove("PROJECT_HOME")
        .env_remove("GIT_WORKTREE_DIR");
    cmd
}

fn config_text(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.xdg_config_home.join("hub/config.toml")).unwrap()
}

#[test]
fn a_fresh_install_configures_itself_with_set() {
    let fx = Fixture::new();
    bare(&fx, &["repo", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hub config set project_home"));
    bare(&fx, &["config", "set", "project_home", "~/projects"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Set project_home"));
    bare(&fx, &["config", "set", "worktree_dir", "~/worktrees"])
        .assert()
        .success();
    assert_eq!(
        config_text(&fx),
        "project_home = \"~/projects\"\nworktree_dir = \"~/worktrees\"\n"
    );
    let hub = fx.init_hub("acme");
    let mut cmd = fx.hub(&hub, &["repo", "list"]);
    cmd.env("HOME", &fx.root)
        .env_remove("PROJECT_HOME")
        .env_remove("GIT_WORKTREE_DIR");
    cmd.assert().success();
}

#[test]
fn show_lists_each_key_with_its_value_and_source() {
    let fx = Fixture::new();
    fx.write_config("# mine\nworktree_dir = \"~/worktrees\"\ntmux = false\n");
    let out = bare(&fx, &["config"])
        .env("PROJECT_HOME", &fx.project_home)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("~/xdg/hub/config.toml"), "{out}");
    let row = |key: &str| {
        out.lines()
            .find(|l| l.starts_with(key))
            .unwrap_or_else(|| panic!("{key} missing:\n{out}"))
            .split_whitespace()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        row("project_home"),
        ["project_home", "~/projects", "PROJECT_HOME"]
    );
    assert_eq!(row("worktree_dir"), ["worktree_dir", "~/worktrees", "file"]);
    assert_eq!(row("editor"), ["editor", "code", "default"]);
    assert_eq!(row("tmux"), ["tmux", "false", "file"]);

    let json = bare(&fx, &["config", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(
        json["path"],
        fx.xdg_config_home
            .join("hub/config.toml")
            .display()
            .to_string()
    );
    assert_eq!(json["keys"][0]["key"], "project_home");
    assert_eq!(json["keys"][0]["value"], serde_json::Value::Null);
    assert_eq!(json["keys"][0]["source"], "unset");
    assert_eq!(json["keys"][3]["value"], "false");
}

#[test]
fn set_and_unset_keep_comments_and_other_keys() {
    let fx = Fixture::new();
    fx.write_config(
        "# where clones live\nproject_home = \"~/projects\" # trailing\n\neditor = \"vim\"\n",
    );
    bare(&fx, &["config", "set", "tmux", "false"])
        .assert()
        .success();
    bare(&fx, &["config", "unset", "editor"]).assert().success();
    assert_eq!(
        config_text(&fx),
        "# where clones live\nproject_home = \"~/projects\" # trailing\ntmux = false\n"
    );
    bare(&fx, &["config", "unset", "editor"])
        .assert()
        .success()
        .stdout(predicate::str::contains("editor is not set"));
}

#[test]
fn set_refuses_bad_keys_and_values_without_writing() {
    let fx = Fixture::new();
    fx.write_config("editor = \"vim\"\n");
    for (args, message) in [
        (&["set", "projct_home", "/"][..], "unknown key projct_home"),
        (&["unset", "projct_home"][..], "unknown key projct_home"),
        (&["set", "tmux", "yes"][..], "tmux must be true or false"),
        (&["set", "editor", "  "][..], "editor must name a command"),
        (&["set", "project_home", "relative"][..], "must be absolute"),
        (&["set", "worktree_dir", "~/nope"][..], "not a directory"),
    ] {
        let mut full = vec!["config"];
        full.extend_from_slice(args);
        bare(&fx, &full)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(message));
    }
    assert_eq!(config_text(&fx), "editor = \"vim\"\n");
}

#[test]
fn set_refuses_a_file_that_does_not_parse_as_config() {
    let fx = Fixture::new();
    fx.write_config("edtor = \"vim\"\n");
    bare(&fx, &["config", "set", "tmux", "true"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("edtor"));
    assert_eq!(config_text(&fx), "edtor = \"vim\"\n");
}
