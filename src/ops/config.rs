use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::env::{self, Config};
use crate::error::{HubError, Result};
use crate::manifest::write_atomic;
use crate::ops::table;

const KEYS: [&str; 4] = ["project_home", "worktree_dir", "editor", "tmux"];

/// Where `hub config` reads and writes, and the variables that override
/// the file. Built from the process for the CLI, by hand in tests.
pub struct ConfigFile {
    pub path: PathBuf,
    pub home: Option<PathBuf>,
    pub project_home_var: Option<OsString>,
    pub worktree_dir_var: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub key: &'static str,
    /// As written in the file or the variable; `None` when unset.
    pub value: Option<String>,
    /// `file`, `default`, `unset`, or the overriding variable's name.
    pub source: String,
}

#[derive(Serialize)]
struct Shown<'a> {
    path: &'a Path,
    keys: &'a [Entry],
}

impl ConfigFile {
    pub fn from_process() -> Result<ConfigFile> {
        let home = env::home_dir();
        let path = env::config_path(std::env::var_os("XDG_CONFIG_HOME"), home.as_deref())
            .ok_or_else(|| {
                HubError::Usage("cannot locate the config file: HOME is not set".into())
            })?;
        Ok(ConfigFile {
            path,
            home,
            project_home_var: std::env::var_os("PROJECT_HOME"),
            worktree_dir_var: std::env::var_os("GIT_WORKTREE_DIR"),
        })
    }

    fn label(&self) -> String {
        env::shorten_home(&self.path, self.home.as_deref())
    }

    /// Every key's effective value and where it comes from, in `KEYS`
    /// order. Values are not validated; the commands that use them do that.
    pub fn show(&self) -> Result<Vec<Entry>> {
        let config = Config::load(&self.path)?;
        let file = |key, value: &Option<String>, default: Option<&str>| match (value, default) {
            (Some(v), _) => Entry {
                key,
                value: Some(v.clone()),
                source: "file".into(),
            },
            (None, Some(d)) => Entry {
                key,
                value: Some(d.into()),
                source: "default".into(),
            },
            (None, None) => Entry {
                key,
                value: None,
                source: "unset".into(),
            },
        };
        let root = |key, var: &str, value: &Option<OsString>, from_file| match value
            .as_ref()
            .filter(|v| !v.is_empty())
        {
            Some(v) => Entry {
                key,
                value: Some(env::shorten_home(Path::new(v), self.home.as_deref())),
                source: var.into(),
            },
            None => file(key, from_file, None),
        };
        Ok(vec![
            root(
                "project_home",
                "PROJECT_HOME",
                &self.project_home_var,
                &config.project_home,
            ),
            root(
                "worktree_dir",
                "GIT_WORKTREE_DIR",
                &self.worktree_dir_var,
                &config.worktree_dir,
            ),
            file("editor", &config.editor, Some("code")),
            file(
                "tmux",
                &config.tmux.map(|t| t.to_string()),
                Some("auto (on when tmux is on PATH)"),
            ),
        ])
    }

    pub fn render(&self, entries: &[Entry]) -> Vec<String> {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                vec![
                    e.key.to_string(),
                    e.value.clone().unwrap_or_else(|| "-".into()),
                    e.source.clone(),
                ]
            })
            .collect();
        let mut lines = vec![format!("Config file: {}", self.label()), String::new()];
        lines.extend(table(&["KEY", "VALUE", "SOURCE"], &rows));
        lines
    }

    pub fn render_json(&self, entries: &[Entry]) -> Result<String> {
        serde_json::to_string_pretty(&Shown {
            path: &self.path,
            keys: entries,
        })
        .map_err(|e| HubError::Precondition(format!("serializing config: {e}")))
    }

    /// Validate `value` for `key` as `Env::resolve` would, then write it,
    /// keeping the rest of the file as it was.
    pub fn set(&self, key: &str, value: &str) -> Result<Vec<String>> {
        let key = known(key)?;
        let item = match key {
            "project_home" | "worktree_dir" => {
                env::existing_dir(key, env::expand_tilde(value, self.home.as_deref()))?;
                toml_edit::value(value)
            }
            "editor" if value.split_whitespace().next().is_none() => {
                return Err(HubError::Usage("editor must name a command".into()));
            }
            "editor" => toml_edit::value(value),
            _ => match value {
                "true" => toml_edit::value(true),
                "false" => toml_edit::value(false),
                _ => return Err(HubError::Usage("tmux must be true or false".into())),
            },
        };
        let mut doc = self.read()?;
        doc[key] = item;
        self.write(&doc)?;
        let mut lines = vec![format!("Set {key} to {value} in {}", self.label())];
        let var = match key {
            "project_home" => Some(("PROJECT_HOME", &self.project_home_var)),
            "worktree_dir" => Some(("GIT_WORKTREE_DIR", &self.worktree_dir_var)),
            _ => None,
        };
        if let Some((name, Some(v))) = var
            && !v.is_empty()
        {
            lines.push(format!("note: {name} is set and overrides this value"));
        }
        Ok(lines)
    }

    pub fn unset(&self, key: &str) -> Result<Vec<String>> {
        let key = known(key)?;
        let mut doc = self.read()?;
        if doc.remove(key).is_none() {
            return Ok(vec![format!("{key} is not set in {}", self.label())]);
        }
        self.write(&doc)?;
        Ok(vec![format!("Removed {key} from {}", self.label())])
    }

    /// The file as an editable document; a missing file is empty.
    fn read(&self) -> Result<toml_edit::DocumentMut> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(HubError::io(format!("reading {}", self.path.display()), e)),
        };
        text.parse()
            .map_err(|e| HubError::Usage(format!("{}: {e}", self.path.display())))
    }

    /// Refuse to write anything the next command would refuse to read.
    fn write(&self, doc: &toml_edit::DocumentMut) -> Result<()> {
        let text = doc.to_string();
        Config::parse(&text, &self.path)?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| HubError::io(format!("creating {}", dir.display()), e))?;
        }
        write_atomic(&self.path, &text)
    }
}

fn known(key: &str) -> Result<&'static str> {
    KEYS.into_iter().find(|k| *k == key).ok_or_else(|| {
        HubError::Usage(format!(
            "unknown key {key}; known keys: {}",
            KEYS.join(", ")
        ))
    })
}
