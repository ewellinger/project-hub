use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HubError, Result};
use crate::names;

pub const MANIFEST_FILE: &str = "hub.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSpec {
    pub role: String,
    pub clone: String,
    pub remote: String,
    pub base: String,
    #[serde(default = "default_branch_template")]
    pub branch_template: String,
    #[serde(default)]
    pub description: String,
}

fn default_branch_template() -> String {
    names::DEFAULT_BRANCH_TEMPLATE.to_string()
}

/// `hub.json`. `repos` order is deployment order and tmux window order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_template: Option<String>,
    #[serde(default)]
    pub repos: Vec<RepoSpec>,
}

impl Manifest {
    pub fn new(name: &str, checkout_template: Option<&str>) -> Manifest {
        Manifest {
            name: name.to_string(),
            checkout_template: checkout_template.map(str::to_string),
            repos: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Manifest> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| HubError::io(format!("reading {}", path.display()), e))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| HubError::Malformed {
            file: path.to_path_buf(),
            field: "(json)".into(),
            message: e.to_string(),
        })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json(path, self)
    }

    pub fn validate(&self, file: &Path) -> Result<()> {
        let bad = |field: &str, message: String| HubError::Malformed {
            file: file.to_path_buf(),
            field: field.to_string(),
            message,
        };
        if self.name.is_empty() {
            return Err(bad("name", "must not be empty".into()));
        }
        if let Some(template) = &self.checkout_template
            && !names::has_checkout_placeholder(template)
        {
            return Err(bad(
                "checkout_template",
                format!("'{template}' contains none of {{feature}}, {{feature_snake}}, {{hub}}"),
            ));
        }
        let mut roles = HashSet::new();
        let mut clones = HashSet::new();
        for repo in &self.repos {
            names::validate_role(&repo.role).map_err(|e| bad("repos[].role", e.to_string()))?;
            names::validate_clone(&repo.clone).map_err(|e| bad("repos[].clone", e.to_string()))?;
            if !roles.insert(repo.role.as_str()) {
                return Err(bad(
                    "repos[].role",
                    format!("duplicate role '{}'", repo.role),
                ));
            }
            if !clones.insert(repo.clone.as_str()) {
                return Err(bad(
                    "repos[].clone",
                    format!("duplicate clone '{}'", repo.clone),
                ));
            }
            if repo.remote.is_empty() {
                return Err(bad(
                    "repos[].remote",
                    format!("role '{}' has an empty remote", repo.role),
                ));
            }
            if repo.base.is_empty() {
                return Err(bad(
                    "repos[].base",
                    format!("role '{}' has an empty base", repo.role),
                ));
            }
            if !repo.branch_template.contains("{feature}") {
                return Err(bad(
                    "repos[].branch_template",
                    format!(
                        "role '{}': '{}' must contain {{feature}}",
                        repo.role, repo.branch_template
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn checkout_template(&self) -> &str {
        self.checkout_template
            .as_deref()
            .unwrap_or(names::DEFAULT_CHECKOUT_TEMPLATE)
    }

    pub fn repo(&self, role: &str) -> Option<&RepoSpec> {
        self.repos.iter().find(|r| r.role == role)
    }

    pub fn require_repo(&self, role: &str) -> Result<&RepoSpec> {
        self.repo(role).ok_or_else(|| {
            let known: Vec<&str> = self.repos.iter().map(|r| r.role.as_str()).collect();
            HubError::Usage(format!(
                "unknown role '{role}'; known roles: {}",
                if known.is_empty() {
                    "(none)".to_string()
                } else {
                    known.join(", ")
                }
            ))
        })
    }

    pub fn role_index(&self, role: &str) -> Option<usize> {
        self.repos.iter().position(|r| r.role == role)
    }
}

/// Pretty JSON with a trailing newline, written to a temp file beside `path`
/// and renamed into place so a concurrent reader never sees a truncated file.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| HubError::Precondition(format!("serializing {}: {e}", path.display())))?;
    text.push('\n');
    write_atomic(path, &text)
}

/// `text` written to a temp file beside `path` and renamed into place.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // Since 0.13.0 a feature record has no second copy in git, so the temp
    // file is flushed to disk before the rename: a crash in between leaves
    // the previous record, never a truncated one.
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    };
    write().map_err(|e| HubError::io(format!("writing {}", tmp.display()), e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        HubError::io(
            format!("renaming {} to {}", tmp.display(), path.display()),
            e,
        )
    })?;
    // The rename is what makes the record durable; syncing the directory
    // only shortens the window, and it is unsupported on some filesystems.
    // A failure here must not turn a completed write into an error that
    // callers roll back.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(role: &str, clone: &str) -> RepoSpec {
        RepoSpec {
            role: role.into(),
            clone: clone.into(),
            remote: "git@example.com:x.git".into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        }
    }

    fn manifest() -> Manifest {
        let mut m = Manifest::new("acme", Some("acme-{feature_snake}"));
        m.repos.push(repo("api", "api-clone"));
        m.repos.push(repo("ui", "ui-clone"));
        m
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.json");
        manifest().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        assert_eq!(Manifest::load(&path).unwrap(), manifest());
    }

    #[test]
    fn missing_optional_fields_take_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.json");
        std::fs::write(
            &path,
            r#"{"name":"h","repos":[{"role":"a","clone":"a","remote":"r","base":"main"}]}"#,
        )
        .unwrap();
        let m = Manifest::load(&path).unwrap();
        assert_eq!(m.checkout_template(), "{feature}");
        assert_eq!(m.repos[0].branch_template, "{feature}");
        assert_eq!(m.repos[0].description, "");
    }

    #[test]
    fn invalid_json_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.json");
        std::fs::write(&path, "{").unwrap();
        let err = Manifest::load(&path).unwrap_err();
        assert!(matches!(err, HubError::Malformed { ref file, .. } if file == &path));
    }

    fn field_error(m: &Manifest) -> String {
        match m.validate(Path::new("hub.json")).unwrap_err() {
            HubError::Malformed { field, .. } => field,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validation_rejects_bad_fields() {
        let mut m = manifest();
        m.name.clear();
        assert_eq!(field_error(&m), "name");

        let mut m = manifest();
        m.checkout_template = Some("static".into());
        assert_eq!(field_error(&m), "checkout_template");

        let mut m = manifest();
        m.repos[1].role = "api".into();
        assert_eq!(field_error(&m), "repos[].role");

        let mut m = manifest();
        m.repos[1].clone = "api-clone".into();
        assert_eq!(field_error(&m), "repos[].clone");

        let mut m = manifest();
        m.repos[0].role = "1bad".into();
        assert_eq!(field_error(&m), "repos[].role");

        let mut m = manifest();
        m.repos[0].branch_template = "no-placeholder".into();
        assert_eq!(field_error(&m), "repos[].branch_template");

        let mut m = manifest();
        m.repos[0].base.clear();
        assert_eq!(field_error(&m), "repos[].base");

        let mut m = manifest();
        m.repos[0].remote.clear();
        assert_eq!(field_error(&m), "repos[].remote");
    }

    #[test]
    fn lookups_by_role() {
        let m = manifest();
        assert_eq!(m.repo("ui").unwrap().clone, "ui-clone");
        assert_eq!(m.role_index("ui"), Some(1));
        assert!(m.repo("nope").is_none());
        let err = m.require_repo("nope").unwrap_err();
        assert!(matches!(err, HubError::Usage(msg) if msg.contains("api, ui")));
    }
}
