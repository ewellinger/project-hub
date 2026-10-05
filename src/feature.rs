use std::path::Path;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{HubError, Result};
use crate::manifest::{Manifest, write_json};
use crate::names;

pub const FEATURES_DIR: &str = "features";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Working,
    Review,
    Merged,
}

impl Stage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage::Working => "working",
            Stage::Review => "review",
            Stage::Merged => "merged",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    Hub,
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// Directory name under `$GIT_WORKTREE_DIR/<clone>/`.
    pub name: String,
    pub owner: Owner,
}

/// One branch in one repo, taking part in a feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub role: String,
    /// Directory name of the repo's main clone under `$PROJECT_HOME`, copied
    /// from the manifest when the change opened so history outlives `repo remove`.
    pub clone: String,
    pub branch: String,
    /// `None` means the branch is checked out in the main clone.
    pub worktree: Option<Worktree>,
    pub stage: Stage,
    #[serde(default)]
    pub review_url: Option<String>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub origin_seen: bool,
    /// Branch this change merges into, without `origin/`. Recorded when the
    /// change opens. `None` only on records written before the field
    /// existed; those fall back to the role's base in `hub.json`.
    #[serde(default)]
    pub base: Option<String>,
    /// Merge base of the branch and `origin/<base>` when the change opened
    /// (the base tip for a branch created from it). The first merge hint in
    /// `status` fires only once the branch has work beyond it.
    #[serde(default)]
    pub base_sha: Option<String>,
}

impl Change {
    pub fn is_open(&self) -> bool {
        self.stage != Stage::Merged
    }

    /// The branch this change merges into: the recorded base, else the
    /// role's base in `hub.json`, else `None` (a legacy record whose role
    /// has since been removed).
    pub fn effective_base(&self, manifest: &Manifest) -> Option<String> {
        self.base
            .clone()
            .or_else(|| manifest.repo(&self.role).map(|r| r.base.clone()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStatus {
    Open,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub checkout: String,
    pub status: FeatureStatus,
    pub created: String,
    pub changes: Vec<Change>,
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero nanoseconds is valid")
        .format(&Rfc3339)
        .expect("RFC 3339 formatting cannot fail")
}

impl Feature {
    pub fn new(name: &str, checkout: &str) -> Feature {
        Feature {
            name: name.to_string(),
            checkout: checkout.to_string(),
            status: FeatureStatus::Open,
            created: now_rfc3339(),
            changes: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Feature> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| HubError::io(format!("reading {}", path.display()), e))?;
        let feature: Feature = serde_json::from_str(&text).map_err(|e| HubError::Malformed {
            file: path.to_path_buf(),
            field: "(json)".into(),
            message: e.to_string(),
        })?;
        feature.validate(path)?;
        Ok(feature)
    }

    /// Validates first: a bad name never reaches disk.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate(path)?;
        write_json(path, self)
    }

    pub fn validate(&self, file: &Path) -> Result<()> {
        let bad = |field: &str, message: String| HubError::Malformed {
            file: file.to_path_buf(),
            field: field.to_string(),
            message,
        };
        names::validate_name("feature name", &self.name).map_err(|e| bad("name", e.to_string()))?;
        names::validate_name("checkout name", &self.checkout)
            .map_err(|e| bad("checkout", e.to_string()))?;
        let mut open_roles = std::collections::HashSet::new();
        for change in &self.changes {
            if change.role.is_empty() {
                return Err(bad("changes[].role", "must not be empty".into()));
            }
            if change.branch.is_empty() {
                return Err(bad(
                    "changes[].branch",
                    format!("role '{}' has an empty branch", change.role),
                ));
            }
            names::validate_clone(&change.clone)
                .map_err(|e| bad("changes[].clone", format!("role '{}': {e}", change.role)))?;
            if let Some(wt) = &change.worktree {
                names::validate_worktree_name(&wt.name).map_err(|e| {
                    bad(
                        "changes[].worktree.name",
                        format!("role '{}': {e}", change.role),
                    )
                })?;
            }
            if let Some(base) = &change.base
                && (base.is_empty() || base.starts_with("origin/"))
            {
                return Err(bad(
                    "changes[].base",
                    format!(
                        "role '{}': base must be a branch name without origin/",
                        change.role
                    ),
                ));
            }
            if change.is_open() && !open_roles.insert(change.role.as_str()) {
                return Err(bad(
                    "changes",
                    format!("role '{}' has more than one open change", change.role),
                ));
            }
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.status == FeatureStatus::Open
    }

    /// Lifecycle and environment commands only apply to open features.
    pub fn require_open(&self) -> Result<()> {
        if self.is_open() {
            Ok(())
        } else {
            Err(HubError::Precondition(format!(
                "feature '{}' is finished",
                self.name
            )))
        }
    }

    pub fn open_change(&self, role: &str) -> Option<&Change> {
        self.changes.iter().find(|c| c.role == role && c.is_open())
    }

    pub fn open_change_mut(&mut self, role: &str) -> Option<&mut Change> {
        self.changes
            .iter_mut()
            .find(|c| c.role == role && c.is_open())
    }

    pub fn open_changes(&self) -> Vec<&Change> {
        self.changes.iter().filter(|c| c.is_open()).collect()
    }

    pub fn last_change(&self, role: &str) -> Option<&Change> {
        self.changes.iter().rev().find(|c| c.role == role)
    }

    pub fn branches_for(&self, role: &str) -> Vec<&str> {
        self.changes
            .iter()
            .filter(|c| c.role == role)
            .map(|c| c.branch.as_str())
            .collect()
    }

    /// Append a change, refusing a second open change for the same role.
    pub fn push_change(&mut self, change: Change) -> Result<()> {
        if change.is_open()
            && let Some(existing) = self.open_change(&change.role)
        {
            return Err(HubError::Precondition(format!(
                "role '{}' already has an open change on branch '{}'",
                change.role, existing.branch
            )));
        }
        self.changes.push(change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RepoSpec;

    fn change(role: &str, branch: &str, stage: Stage) -> Change {
        Change {
            role: role.into(),
            clone: format!("{role}-clone"),
            branch: branch.into(),
            worktree: Some(Worktree {
                name: names::flatten_branch(branch),
                owner: Owner::Hub,
            }),
            stage,
            review_url: None,
            merged_at: None,
            origin_seen: false,
            base: None,
            base_sha: None,
        }
    }

    fn manifest_with(role: &str, base: &str) -> Manifest {
        let mut manifest = Manifest::new("acme", None);
        manifest.repos.push(RepoSpec {
            role: role.into(),
            clone: format!("{role}-clone"),
            remote: "git@example.com:x.git".into(),
            base: base.into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        });
        manifest
    }

    #[test]
    fn legacy_change_without_base_falls_back_to_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        std::fs::write(
            &path,
            r#"{"name":"f","checkout":"f","status":"open","created":"2026-09-08T00:00:00Z",
                "changes":[{"role":"ui","clone":"ui-clone","branch":"b","worktree":null,"stage":"working","base_sha":"abc"}]}"#,
        )
        .unwrap();
        let f = Feature::load(&path).unwrap();
        assert_eq!(f.changes[0].base, None);
        assert_eq!(
            f.changes[0].effective_base(&Manifest::new("acme", None)),
            None,
            "role gone from hub.json: no base at all"
        );
        assert_eq!(
            f.changes[0]
                .effective_base(&manifest_with("ui", "develop"))
                .as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn recorded_base_wins_over_the_manifest_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        let mut f = Feature::new("feat-1", "feat-1");
        let mut c = change("api", "feature/x", Stage::Working);
        c.base = Some("feature/canonical".into());
        f.push_change(c).unwrap();
        f.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"base\": \"feature/canonical\""), "{text}");
        let loaded = Feature::load(&path).unwrap();
        assert_eq!(loaded, f);
        assert_eq!(
            loaded.changes[0]
                .effective_base(&manifest_with("api", "master"))
                .as_deref(),
            Some("feature/canonical")
        );
    }

    #[test]
    fn timestamp_is_rfc3339_without_fraction() {
        let ts = now_rfc3339();
        assert!(ts.ends_with('Z'), "{ts}");
        assert!(!ts.contains('.'), "{ts}");
        assert_eq!(ts.len(), 20, "{ts}");
    }

    #[test]
    fn round_trips_and_serializes_lowercase_enums() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        let mut f = Feature::new("feat-1", "acme-feat_1");
        f.push_change(change("api", "feature/x", Stage::Merged))
            .unwrap();
        f.push_change(change("api", "feature/x-2", Stage::Working))
            .unwrap();
        f.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"stage\": \"merged\""));
        assert!(text.contains("\"owner\": \"hub\""));
        assert!(text.contains("\"status\": \"open\""));
        assert_eq!(Feature::load(&path).unwrap(), f);
    }

    #[test]
    fn missing_optional_change_fields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        std::fs::write(
            &path,
            r#"{"name":"f","checkout":"f","status":"open","created":"2026-09-08T00:00:00Z",
                "changes":[{"role":"ui","clone":"ui-clone","branch":"b","worktree":null,"stage":"working"}]}"#,
        )
        .unwrap();
        let f = Feature::load(&path).unwrap();
        assert_eq!(f.changes[0].review_url, None);
        assert_eq!(f.changes[0].merged_at, None);
        assert!(!f.changes[0].origin_seen);
        assert_eq!(f.changes[0].base, None);
        assert_eq!(f.changes[0].base_sha, None);
        assert_eq!(f.changes[0].worktree, None);
    }

    #[test]
    fn require_open_rejects_finished_features() {
        let mut f = Feature::new("feat-1", "feat-1");
        f.require_open().unwrap();
        f.status = FeatureStatus::Finished;
        let err = f.require_open().unwrap_err();
        assert!(matches!(err, HubError::Precondition(m) if m.contains("finished")));
    }

    #[test]
    fn push_change_enforces_one_open_change_per_role() {
        let mut f = Feature::new("feat-1", "feat-1");
        f.push_change(change("api", "a", Stage::Working)).unwrap();
        let err = f
            .push_change(change("api", "b", Stage::Working))
            .unwrap_err();
        assert!(matches!(err, HubError::Precondition(m) if m.contains("api") && m.contains("'a'")));
        f.push_change(change("ui", "c", Stage::Review)).unwrap();
        assert_eq!(f.open_changes().len(), 2);
    }

    #[test]
    fn open_and_last_change_lookups() {
        let mut f = Feature::new("feat-1", "feat-1");
        f.push_change(change("api", "a", Stage::Merged)).unwrap();
        f.push_change(change("api", "a-2", Stage::Working)).unwrap();
        assert_eq!(f.open_change("api").unwrap().branch, "a-2");
        assert_eq!(f.last_change("api").unwrap().branch, "a-2");
        assert_eq!(f.branches_for("api"), vec!["a", "a-2"]);
        assert!(f.open_change("ui").is_none());
        f.open_change_mut("api").unwrap().stage = Stage::Merged;
        assert!(f.open_change("api").is_none());
        assert_eq!(f.last_change("api").unwrap().branch, "a-2");
    }

    fn field_error(f: &Feature) -> String {
        match f.validate(Path::new("f.json")).unwrap_err() {
            HubError::Malformed { field, .. } => field,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validation_rejects_bad_state() {
        let mut f = Feature::new("bad.name", "x");
        assert_eq!(field_error(&f), "name");
        f.name = "ok".into();
        f.checkout = "a:b".into();
        assert_eq!(field_error(&f), "checkout");
        f.checkout = "ok".into();
        f.changes.push(change("api", "a", Stage::Working));
        f.changes.push(change("api", "b", Stage::Review));
        assert_eq!(field_error(&f), "changes");
        f.changes.pop();
        f.changes[0].branch.clear();
        assert_eq!(field_error(&f), "changes[].branch");
        f.changes[0].branch = "a".into();
        f.changes[0].worktree = Some(Worktree {
            name: "a/b".into(),
            owner: Owner::Hub,
        });
        assert_eq!(field_error(&f), "changes[].worktree.name");
        f.changes[0].worktree = Some(Worktree {
            name: "..".into(),
            owner: Owner::Hub,
        });
        assert_eq!(field_error(&f), "changes[].worktree.name");
        let dir = tempfile::tempdir().unwrap();
        assert!(
            f.save(&dir.path().join("f.json")).is_err(),
            "save validates too"
        );
        assert!(!dir.path().join("f.json").exists());
        f.changes[0].worktree = None;
        f.changes[0].base = Some("origin/main".into());
        assert_eq!(field_error(&f), "changes[].base");
        f.changes[0].base = Some(String::new());
        assert_eq!(field_error(&f), "changes[].base");
        f.changes[0].base = None;
        f.changes[0].clone = "bad_clone".into();
        assert_eq!(field_error(&f), "changes[].clone");
    }
}
