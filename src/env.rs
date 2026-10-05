use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{HubError, Result};

/// Example printed when neither the variables nor the config file give
/// the two roots.
const EXAMPLE: &str = "project_home = \"~/workspace\"\nworktree_dir = \"~/workspace/worktrees\"";
const DEFAULT_CONFIG_PATH: &str = "~/.config/hub/config.toml";

/// The two directories every command needs, plus the editor `hub open`
/// launches. The directories are canonicalized so they compare equal to
/// the real paths git prints.
#[derive(Debug, Clone)]
pub struct Env {
    pub project_home: PathBuf,
    pub worktree_dir: PathBuf,
    /// The editor command and its arguments; the workspace path is appended.
    pub editor: Vec<String>,
    /// `tmux` from the config file as written; `Hub::locate` combines it
    /// with a `PATH` lookup.
    pub tmux: Option<bool>,
    /// The config file path as messages show it (`~/…`).
    pub config_label: String,
}

/// `config.toml`: every key optional, unknown keys refused so a typo does
/// not silently fall through to the variables.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project_home: Option<String>,
    pub worktree_dir: Option<String>,
    pub editor: Option<String>,
    pub tmux: Option<bool>,
}

/// The user's home directory from `HOME`, for display only.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Print `path` with a leading `home` replaced by `~`.
pub fn shorten_home(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => format!("~/{}", rel.display()),
        _ => path.display().to_string(),
    }
}

/// `$XDG_CONFIG_HOME/hub/config.toml`, or `~/.config/hub/config.toml`
/// when the variable is unset, empty, or relative (the XDG spec says to
/// ignore a relative value); `None` without a home directory.
pub fn config_path(xdg_config_home: Option<OsString>, home: Option<&Path>) -> Option<PathBuf> {
    let base = match xdg_config_home.map(PathBuf::from) {
        Some(xdg) if xdg.is_absolute() => xdg,
        _ => home?.join(".config"),
    };
    Some(base.join("hub").join("config.toml"))
}

/// `~` and `~/rest` against `home`; anything else, or no home, unchanged.
pub fn expand_tilde(value: &str, home: Option<&Path>) -> PathBuf {
    match (value.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(value),
    }
}

impl Config {
    /// Read `path`; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(HubError::io(format!("reading {}", path.display()), e)),
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Config> {
        toml::from_str(text).map_err(|e| HubError::Usage(format!("{}: {e}", path.display())))
    }
}

impl Env {
    pub fn from_process() -> Result<Env> {
        let home = home_dir();
        let path = config_path(std::env::var_os("XDG_CONFIG_HOME"), home.as_deref());
        let config = match &path {
            Some(path) => Config::load(path)?,
            None => Config::default(),
        };
        Env::resolve(
            std::env::var_os("PROJECT_HOME"),
            std::env::var_os("GIT_WORKTREE_DIR"),
            &config,
            path.as_deref(),
            home.as_deref(),
        )
    }

    /// The variables alone, with the default editor: what tests build.
    pub fn from_vars(
        project_home: Option<OsString>,
        worktree_dir: Option<OsString>,
    ) -> Result<Env> {
        Env::resolve(project_home, worktree_dir, &Config::default(), None, None)
    }

    /// Each root comes from its variable when that is set and non-empty,
    /// else from `config`; `config_path` and `home` only shape messages
    /// and expand `~`.
    pub fn resolve(
        project_home: Option<OsString>,
        worktree_dir: Option<OsString>,
        config: &Config,
        config_path: Option<&Path>,
        home: Option<&Path>,
    ) -> Result<Env> {
        let shown_path = config_path
            .map(|p| shorten_home(p, home))
            .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        let source =
            |var: &str, key: &str, value: Option<OsString>, from_config: &Option<String>| {
                match value.filter(|v| !v.is_empty()) {
                    Some(value) => Some((var.to_string(), PathBuf::from(value))),
                    None => from_config
                        .as_ref()
                        .map(|v| (format!("{key} in {shown_path}"), expand_tilde(v, home))),
                }
            };
        let project_home = source(
            "PROJECT_HOME",
            "project_home",
            project_home,
            &config.project_home,
        );
        let worktree_dir = source(
            "GIT_WORKTREE_DIR",
            "worktree_dir",
            worktree_dir,
            &config.worktree_dir,
        );
        let missing: Vec<&str> = [
            (project_home.is_none(), "PROJECT_HOME"),
            (worktree_dir.is_none(), "GIT_WORKTREE_DIR"),
        ]
        .into_iter()
        .filter_map(|(missing, name)| missing.then_some(name))
        .collect();
        if !missing.is_empty() {
            let verb = if missing.len() == 1 { "is" } else { "are" };
            return Err(HubError::Usage(format!(
                "{} {verb} not set; create {shown_path} with\n\n{}\n\nor run `hub config set project_home <dir>` and `hub config set worktree_dir <dir>`,\nor export the variable(s)",
                missing.join(" and "),
                EXAMPLE
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )));
        }
        let (ph_label, ph) = project_home.expect("checked above");
        let (wt_label, wt) = worktree_dir.expect("checked above");
        let editor = match &config.editor {
            None => vec!["code".to_string()],
            Some(editor) => {
                let words: Vec<String> = editor.split_whitespace().map(String::from).collect();
                if words.is_empty() {
                    return Err(HubError::Usage(format!(
                        "editor in {shown_path} must name a command"
                    )));
                }
                words
            }
        };
        Ok(Env {
            project_home: existing_dir(&ph_label, ph)?,
            worktree_dir: existing_dir(&wt_label, wt)?,
            editor,
            tmux: config.tmux,
            config_label: shown_path,
        })
    }

    pub fn clone_path(&self, clone: &str) -> PathBuf {
        self.project_home.join(clone)
    }

    pub fn worktree_path(&self, clone: &str, name: &str) -> PathBuf {
        self.worktree_dir.join(clone).join(name)
    }

    /// If `path` is exactly one level below `<worktree_dir>/<clone>/`,
    /// return that directory name.
    pub fn worktree_name(&self, clone: &str, path: &Path) -> Option<String> {
        let parent = self.worktree_dir.join(clone);
        let rel = path.strip_prefix(&parent).ok()?;
        let mut components = rel.components();
        let first = components.next()?;
        if components.next().is_some() {
            return None;
        }
        Some(first.as_os_str().to_string_lossy().into_owned())
    }
}

/// `label` names the source in messages: the variable, or `key in <file>`.
pub(crate) fn existing_dir(label: &str, path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(HubError::Usage(format!(
            "{label} must be absolute, got {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(HubError::Usage(format!(
            "{label} is not a directory: {}",
            path.display()
        )));
    }
    path.canonicalize()
        .map_err(|e| HubError::io(format!("resolving {label} {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, Env) {
        let tmp = tempfile::tempdir().unwrap();
        let ph = tmp.path().join("projects");
        let wt = tmp.path().join("worktrees");
        std::fs::create_dir_all(&ph).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        let env = Env::from_vars(Some(ph.into()), Some(wt.into())).unwrap();
        (tmp, env)
    }

    #[test]
    fn missing_variable_is_a_usage_error() {
        let err = Env::from_vars(None, Some("/tmp".into())).unwrap_err();
        assert!(matches!(err, HubError::Usage(m) if m.contains("PROJECT_HOME")));
    }

    #[test]
    fn relative_path_is_rejected() {
        let err = Env::from_vars(Some("relative".into()), Some("/tmp".into())).unwrap_err();
        assert!(matches!(err, HubError::Usage(m) if m.contains("absolute")));
    }

    #[test]
    fn missing_directory_is_rejected() {
        let err =
            Env::from_vars(Some("/definitely/not/here".into()), Some("/tmp".into())).unwrap_err();
        assert!(matches!(err, HubError::Usage(m) if m.contains("not a directory")));
    }

    #[test]
    fn paths_are_derived_from_the_two_roots() {
        let (_tmp, env) = env();
        assert_eq!(env.clone_path("api"), env.project_home.join("api"));
        assert_eq!(
            env.worktree_path("api", "feat-1"),
            env.worktree_dir.join("api").join("feat-1")
        );
    }

    #[test]
    fn worktree_name_only_matches_direct_children() {
        let (_tmp, env) = env();
        let direct = env.worktree_dir.join("api").join("api-review_368");
        assert_eq!(
            env.worktree_name("api", &direct).as_deref(),
            Some("api-review_368")
        );
        let nested = env.worktree_dir.join("api").join("a").join("b");
        assert_eq!(env.worktree_name("api", &nested), None);
        let other_clone = env.worktree_dir.join("bff").join("x");
        assert_eq!(env.worktree_name("api", &other_clone), None);
        assert_eq!(
            env.worktree_name("api", &env.project_home.join("api")),
            None
        );
    }

    fn config(text: &str) -> Config {
        Config::parse(text, Path::new("/cfg/config.toml")).unwrap()
    }

    #[test]
    fn config_path_prefers_xdg_config_home_then_home() {
        let home = Path::new("/Users/me");
        assert_eq!(
            config_path(Some("/xdg".into()), Some(home)),
            Some(PathBuf::from("/xdg/hub/config.toml"))
        );
        assert_eq!(
            config_path(None, Some(home)),
            Some(PathBuf::from("/Users/me/.config/hub/config.toml"))
        );
        assert_eq!(
            config_path(Some("".into()), Some(home)),
            Some(PathBuf::from("/Users/me/.config/hub/config.toml")),
            "an empty XDG_CONFIG_HOME is unset"
        );
        assert_eq!(
            config_path(Some("relative".into()), Some(home)),
            Some(PathBuf::from("/Users/me/.config/hub/config.toml")),
            "a relative XDG_CONFIG_HOME is ignored, as the spec says"
        );
        assert_eq!(config_path(None, None), None);
    }

    #[test]
    fn config_parses_the_three_keys_and_defaults_the_rest() {
        assert_eq!(config(""), Config::default());
        let c = config("project_home = \"~/w\"\nworktree_dir = \"/wt\"\neditor = \"code -n\"\n");
        assert_eq!(c.project_home.as_deref(), Some("~/w"));
        assert_eq!(c.worktree_dir.as_deref(), Some("/wt"));
        assert_eq!(c.editor.as_deref(), Some("code -n"));
    }

    #[test]
    fn config_rejects_unknown_keys_and_wrong_types_naming_the_file() {
        let err =
            Config::parse("projects_home = \"/x\"\n", Path::new("/cfg/config.toml")).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.starts_with("/cfg/config.toml:") && m.contains("projects_home")),
            "{err}"
        );
        let err = Config::parse("project_home = 1\n", Path::new("/cfg/config.toml")).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.starts_with("/cfg/config.toml:")),
            "{err}"
        );
    }

    #[test]
    fn config_load_treats_a_missing_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hub").join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "editor = \"vim\"\n").unwrap();
        assert_eq!(Config::load(&path).unwrap().editor.as_deref(), Some("vim"));
    }

    #[test]
    fn env_vars_override_the_config_per_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for d in ["env-ph", "env-wt", "cfg-ph", "cfg-wt"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let c = config(&format!(
            "project_home = \"{}\"\nworktree_dir = \"{}\"\n",
            root.join("cfg-ph").display(),
            root.join("cfg-wt").display()
        ));
        let env = Env::resolve(
            Some(root.join("env-ph").into()),
            None,
            &c,
            Some(Path::new("/cfg/config.toml")),
            None,
        )
        .unwrap();
        assert_eq!(env.project_home, root.join("env-ph"));
        assert_eq!(env.worktree_dir, root.join("cfg-wt"));
        assert_eq!(env.editor, vec!["code".to_string()], "the default editor");
        let env = Env::resolve(
            Some("".into()),
            Some(root.join("env-wt").into()),
            &c,
            Some(Path::new("/cfg/config.toml")),
            None,
        )
        .unwrap();
        assert_eq!(
            env.project_home,
            root.join("cfg-ph"),
            "an empty variable is unset"
        );
        assert_eq!(env.worktree_dir, root.join("env-wt"));
    }

    #[test]
    fn config_paths_expand_a_leading_tilde_against_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join("w").join("wt")).unwrap();
        let c = config("project_home = \"~/w\"\nworktree_dir = \"~/w/wt\"\n");
        let env = Env::resolve(
            None,
            None,
            &c,
            Some(Path::new("/cfg/config.toml")),
            Some(&home),
        )
        .unwrap();
        assert_eq!(env.project_home, home.join("w"));
        assert_eq!(env.worktree_dir, home.join("w").join("wt"));
        assert_eq!(expand_tilde("~", Some(&home)), home);
        assert_eq!(
            expand_tilde("~user/x", Some(&home)),
            PathBuf::from("~user/x")
        );
        assert_eq!(expand_tilde("~/x", None), PathBuf::from("~/x"));
    }

    #[test]
    fn a_bad_config_path_names_the_key_and_the_file() {
        let c = config("project_home = \"relative\"\nworktree_dir = \"/definitely/not/here\"\n");
        let err =
            Env::resolve(None, None, &c, Some(Path::new("/cfg/config.toml")), None).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.contains("project_home in /cfg/config.toml must be absolute")),
            "{err}"
        );
        let c = config("project_home = \"/\"\nworktree_dir = \"/definitely/not/here\"\n");
        let err =
            Env::resolve(None, None, &c, Some(Path::new("/cfg/config.toml")), None).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.contains("worktree_dir in /cfg/config.toml is not a directory")),
            "{err}"
        );
    }

    #[test]
    fn nothing_set_names_the_config_file_and_shows_the_example() {
        let home = Path::new("/Users/me");
        let path = Path::new("/Users/me/.config/hub/config.toml");
        let err = Env::resolve(None, None, &Config::default(), Some(path), Some(home)).unwrap_err();
        let HubError::Usage(m) = err else {
            panic!("{err}")
        };
        assert!(
            m.starts_with("PROJECT_HOME and GIT_WORKTREE_DIR are not set"),
            "{m}"
        );
        assert!(m.contains("~/.config/hub/config.toml"), "{m}");
        assert!(m.contains("project_home = \"~/workspace\""), "{m}");
        assert!(
            m.contains("worktree_dir = \"~/workspace/worktrees\""),
            "{m}"
        );
        let c = config("worktree_dir = \"/\"\n");
        let err = Env::resolve(None, None, &c, Some(path), Some(home)).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.starts_with("PROJECT_HOME is not set")),
            "{err}"
        );
        let err = Env::resolve(None, None, &Config::default(), None, None).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.contains("~/.config/hub/config.toml")),
            "{err}"
        );
    }

    #[test]
    fn editor_is_split_on_whitespace_and_must_not_be_blank() {
        let c = config(
            "project_home = \"/\"\nworktree_dir = \"/\"\neditor = \"  code  --new-window \"\n",
        );
        let env = Env::resolve(None, None, &c, Some(Path::new("/cfg/config.toml")), None).unwrap();
        assert_eq!(
            env.editor,
            vec!["code".to_string(), "--new-window".to_string()]
        );
        let c = config("project_home = \"/\"\nworktree_dir = \"/\"\neditor = \" \"\n");
        let err =
            Env::resolve(None, None, &c, Some(Path::new("/cfg/config.toml")), None).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.contains("editor in /cfg/config.toml must name a command")),
            "{err}"
        );
    }

    #[test]
    fn shorten_home_replaces_only_a_home_prefix() {
        let home = Path::new("/Users/me");
        assert_eq!(
            shorten_home(Path::new("/Users/me/wt/api/x"), Some(home)),
            "~/wt/api/x"
        );
        assert_eq!(shorten_home(Path::new("/opt/x"), Some(home)), "/opt/x");
        assert_eq!(shorten_home(Path::new("/Users/me/x"), None), "/Users/me/x");
    }

    #[test]
    fn tmux_key_is_a_boolean() {
        assert_eq!(config("tmux = false\n").tmux, Some(false));
        assert_eq!(config("").tmux, None);
        let err = Config::parse("tmux = \"no\"\n", Path::new("/cfg/config.toml")).unwrap_err();
        assert!(
            matches!(&err, HubError::Usage(m) if m.contains("/cfg/config.toml")),
            "{err}"
        );
    }
}
