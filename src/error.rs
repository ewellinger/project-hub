use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, HubError>;

#[derive(Debug, Error)]
pub enum HubError {
    /// Bad arguments or a missing prerequisite the user controls. Exit 1.
    #[error("{0}")]
    Usage(String),
    /// A state check failed; nothing was changed. Exit 1.
    #[error("{0}")]
    Precondition(String),
    /// A worktree has uncommitted changes and --force was not given. Exit 20.
    #[error("dirty worktree at {}\n{status}", path.display())]
    Dirty { path: PathBuf, status: String },
    /// A process still runs inside a worktree about to be removed and --force
    /// was not given. Exit 21.
    #[error("worktree in use at {}\n{}", path.display(), processes.join("\n"))]
    InUse {
        path: PathBuf,
        processes: Vec<String>,
    },
    /// `status` found at least one structurally inconsistent row. Exit 30.
    #[error("status found drift")]
    Drift,
    /// hub.json or a feature file failed validation.
    #[error("{}: {field}: {message}", file.display())]
    Malformed {
        file: PathBuf,
        field: String,
        message: String,
    },
    /// A git, tmux, or code subprocess exited non-zero.
    #[error("command failed: {command}\n{stderr}")]
    Command { command: String, stderr: String },
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl HubError {
    pub fn exit_code(&self) -> i32 {
        match self {
            HubError::Dirty { .. } => 20,
            HubError::InUse { .. } => 21,
            HubError::Drift => 30,
            _ => 1,
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> HubError {
        HubError::Io {
            context: context.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_spec() {
        assert_eq!(HubError::Usage("x".into()).exit_code(), 1);
        assert_eq!(HubError::Precondition("x".into()).exit_code(), 1);
        assert_eq!(
            HubError::Dirty {
                path: "/p".into(),
                status: "M a".into()
            }
            .exit_code(),
            20
        );
        assert_eq!(
            HubError::InUse {
                path: "/p".into(),
                processes: vec!["4242 vite".into()]
            }
            .exit_code(),
            21
        );
        assert_eq!(HubError::Drift.exit_code(), 30);
    }
}
