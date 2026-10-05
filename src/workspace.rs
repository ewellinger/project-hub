use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{HubError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    pub name: String,
    pub path: String,
}

/// Render a `.code-workspace`. Only `folders` is replaced; every other
/// top-level key in `existing` survives.
pub fn render(existing: Option<&str>, folders: &[Folder]) -> Result<String> {
    let mut root = match existing.map(str::trim) {
        Some(text) if !text.is_empty() => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                return Err(HubError::Precondition(
                    "existing workspace file is not a JSON object".into(),
                ));
            }
            Err(e) => {
                return Err(HubError::Precondition(format!(
                    "existing workspace file is not valid JSON: {e}"
                )));
            }
        },
        _ => Map::new(),
    };
    let folders = serde_json::to_value(folders)
        .map_err(|e| HubError::Precondition(format!("serializing folders: {e}")))?;
    root.insert("folders".to_string(), folders);
    let mut text = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| HubError::Precondition(format!("serializing workspace: {e}")))?;
    text.push('\n');
    Ok(text)
}

/// Relative path from directory `from` to `to`. Both must be absolute.
pub fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel = PathBuf::new();
    for _ in common..from.len() {
        rel.push("..");
    }
    for component in &to[common..] {
        rel.push(component.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                name: "acme".into(),
                path: ".".into(),
            },
            Folder {
                name: "api".into(),
                path: "../../api-clone/feat".into(),
            },
        ]
    }

    #[test]
    fn renders_folders_from_scratch() {
        let text = render(None, &folders()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["folders"][0]["name"], "acme");
        assert_eq!(value["folders"][1]["path"], "../../api-clone/feat");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn preserves_other_top_level_keys() {
        let existing = r#"{"folders":[{"name":"old","path":"x"}],"settings":{"editor.tabSize":2}}"#;
        let text = render(Some(existing), &folders()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["settings"]["editor.tabSize"], 2);
        assert_eq!(value["folders"].as_array().unwrap().len(), 2);
        assert_eq!(value["folders"][0]["name"], "acme");
    }

    #[test]
    fn is_idempotent() {
        let once = render(None, &folders()).unwrap();
        let twice = render(Some(&once), &folders()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn rejects_invalid_existing_json() {
        let err = render(Some("{nope"), &folders()).unwrap_err();
        assert!(matches!(err, HubError::Precondition(m) if m.contains("not valid JSON")));
        let err = render(Some("[]"), &folders()).unwrap_err();
        assert!(matches!(err, HubError::Precondition(m) if m.contains("not a JSON object")));
    }

    #[test]
    fn relative_paths() {
        let from = Path::new("/w/worktrees/acme/acme-x");
        assert_eq!(
            relative_path(from, Path::new("/w/worktrees/api/feat")),
            PathBuf::from("../../api/feat")
        );
        assert_eq!(
            relative_path(from, Path::new("/w/projects/api")),
            PathBuf::from("../../../projects/api")
        );
        assert_eq!(relative_path(from, from), PathBuf::from("."));
        assert_eq!(
            relative_path(from, Path::new("/w/worktrees/acme/acme-x/sub")),
            PathBuf::from("sub")
        );
    }
}
