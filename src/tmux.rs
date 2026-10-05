use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output};

use crate::error::{HubError, Result};

/// Runs `tmux`. Sessions are always addressed as `=<name>` so tmux never prefix-matches.
#[derive(Debug, Clone)]
pub struct Tmux {
    program: OsString,
    envs: Vec<(OsString, OsString)>,
}

impl Default for Tmux {
    fn default() -> Tmux {
        Tmux::with_program("tmux")
    }
}

fn target(session: &str) -> String {
    format!("={session}")
}

/// Why session steps are skipped, and the warning to print when the
/// config asked for tmux but it is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxOff {
    pub reason: String,
    pub warning: Option<String>,
}

/// Whether sessions are on: `None` means on.
pub fn decide(setting: Option<bool>, found: bool, config_label: &str) -> Option<TmuxOff> {
    match (setting, found) {
        (Some(false), _) => Some(TmuxOff {
            reason: format!("tmux = false in {config_label}"),
            warning: None,
        }),
        (_, true) => None,
        (setting, false) => Some(TmuxOff {
            reason: "tmux is not on PATH".into(),
            warning: (setting == Some(true)).then(|| {
                format!(
                    "warning: tmux = true in {config_label} but tmux is not on PATH; skipping sessions"
                )
            }),
        }),
    }
}

/// True when some `PATH` entry holds an executable file named `tmux`.
/// Nothing is spawned.
pub fn on_path(path: Option<&std::ffi::OsStr>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_some_and(|p| {
        std::env::split_paths(p).any(|dir| {
            std::fs::metadata(dir.join("tmux"))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

impl Tmux {
    pub fn with_program(program: impl Into<OsString>) -> Tmux {
        Tmux {
            program: program.into(),
            envs: Vec::new(),
        }
    }

    /// Extra environment for the subprocess (tests point the fake at its state directory).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Tmux {
        self.envs.push((key.into(), value.into()));
        self
    }

    fn output(&self, args: &[&str]) -> Result<Output> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args);
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        cmd.output()
            .map_err(|e| HubError::io(format!("running {}", self.program.to_string_lossy()), e))
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(HubError::Command {
                command: format!("tmux {}", args.join(" ")),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    pub fn has_session(&self, name: &str) -> Result<bool> {
        Ok(self
            .output(&["has-session", "-t", &target(name)])?
            .status
            .success())
    }

    pub fn new_session(&self, name: &str, window: &str, dir: &Path) -> Result<()> {
        self.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-n",
            window,
            "-c",
            &dir.to_string_lossy(),
        ])
        .map(|_| ())
    }

    pub fn windows(&self, name: &str) -> Result<Vec<String>> {
        let text = self.run(&["list-windows", "-t", &target(name), "-F", "#{window_name}"])?;
        Ok(text.lines().map(str::to_string).collect())
    }

    /// Append a window at the next free index.
    pub fn new_window(&self, session: &str, window: &str, dir: &Path) -> Result<()> {
        self.run(&[
            "new-window",
            "-t",
            &target(session),
            "-n",
            window,
            "-c",
            &dir.to_string_lossy(),
        ])
        .map(|_| ())
    }

    /// Kill whatever runs in the window and start a fresh shell in `dir`.
    pub fn respawn_window(&self, session: &str, window: &str, dir: &Path) -> Result<()> {
        self.run(&[
            "respawn-pane",
            "-k",
            "-t",
            &format!("={session}:{window}"),
            "-c",
            &dir.to_string_lossy(),
        ])
        .map(|_| ())
    }

    /// Type `keys` into the window's shell, as `send-keys` does; `Enter` is a key name.
    pub fn send_keys(&self, session: &str, window: &str, keys: &[&str]) -> Result<()> {
        let target = format!("={session}:{window}");
        let mut args = vec!["send-keys", "-t", target.as_str()];
        args.extend_from_slice(keys);
        self.run(&args).map(|_| ())
    }

    pub fn rename_window(&self, session: &str, old: &str, new: &str) -> Result<()> {
        self.run(&["rename-window", "-t", &format!("={session}:{old}"), new])
            .map(|_| ())
    }

    pub fn kill_session(&self, name: &str) -> Result<()> {
        self.run(&["kill-session", "-t", &target(name)]).map(|_| ())
    }

    /// Name of the tmux session this process runs inside, if any. `$TMUX` is
    /// set for every process started from a tmux pane and `display-message`
    /// resolves the client it belongs to.
    pub fn current_session(&self) -> Result<Option<String>> {
        if std::env::var_os("TMUX").is_none() {
            return Ok(None);
        }
        self.run(&["display-message", "-p", "#S"]).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fake() -> (TempDir, Tmux) {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-bin/tmux");
        let tmux = Tmux::with_program(program)
            .env("FAKE_TMUX_STATE", state.as_os_str())
            .env("FAKE_TMUX_LOG", tmp.path().join("log").as_os_str());
        (tmp, tmux)
    }

    #[test]
    fn session_lifecycle_against_fake() {
        let (tmp, tmux) = fake();
        assert!(!tmux.has_session("s1").unwrap());
        tmux.new_session("s1", "hub", tmp.path()).unwrap();
        assert!(tmux.has_session("s1").unwrap());
        tmux.new_window("s1", "ai", tmp.path()).unwrap();
        tmux.new_window("s1", "api", tmp.path()).unwrap();
        assert_eq!(tmux.windows("s1").unwrap(), vec!["hub", "ai", "api"]);
        tmux.respawn_window("s1", "api", &tmp.path().join("elsewhere"))
            .unwrap();
        let windows = std::fs::read_to_string(tmp.path().join("state/s1/windows")).unwrap();
        assert!(windows.contains("api\t") && windows.contains("elsewhere"));
        assert!(tmux.respawn_window("s1", "nope", tmp.path()).is_err());
        tmux.rename_window("s1", "api", "platform").unwrap();
        assert_eq!(tmux.windows("s1").unwrap(), vec!["hub", "ai", "platform"]);
        assert!(tmux.rename_window("s1", "nope", "x").is_err());
        tmux.send_keys("s1", "hub", &["hub", "Enter"]).unwrap();
        assert!(tmux.send_keys("nope", "hub", &["x"]).is_err());
        tmux.kill_session("s1").unwrap();
        assert!(!tmux.has_session("s1").unwrap());
        assert!(tmux.kill_session("s1").is_err());
        let log = std::fs::read_to_string(tmp.path().join("log")).unwrap();
        assert!(log.contains("new-session -d -s s1 -n hub -c"));
        assert!(log.contains("has-session -t =s1"));
        assert!(log.contains("send-keys -t =s1:hub hub Enter"));
    }

    #[test]
    fn forced_failure_surfaces_as_command_error() {
        let (tmp, tmux) = fake();
        let tmux = tmux.env("FAKE_TMUX_FAIL", "new-session");
        let err = tmux.new_session("s1", "hub", tmp.path()).unwrap_err();
        assert!(
            matches!(err, HubError::Command { stderr, .. } if stderr.contains("forced failure"))
        );
    }

    #[test]
    fn decide_covers_every_setting() {
        assert_eq!(decide(None, true, "~/c.toml"), None);
        assert_eq!(decide(Some(true), true, "~/c.toml"), None);
        assert_eq!(
            decide(Some(false), true, "~/c.toml"),
            Some(TmuxOff {
                reason: "tmux = false in ~/c.toml".into(),
                warning: None
            })
        );
        assert_eq!(
            decide(None, false, "~/c.toml"),
            Some(TmuxOff {
                reason: "tmux is not on PATH".into(),
                warning: None
            })
        );
        assert_eq!(
            decide(Some(true), false, "~/c.toml"),
            Some(TmuxOff {
                reason: "tmux is not on PATH".into(),
                warning: Some(
                    "warning: tmux = true in ~/c.toml but tmux is not on PATH; skipping sessions"
                        .into()
                )
            })
        );
    }

    #[test]
    fn on_path_needs_an_executable_named_tmux() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let (with, plain, empty) = (
            tmp.path().join("with"),
            tmp.path().join("plain"),
            tmp.path().join("empty"),
        );
        for d in [&with, &plain, &empty] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(with.join("tmux"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(with.join("tmux"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(plain.join("tmux"), "not executable").unwrap();
        let path = |dirs: &[&std::path::PathBuf]| std::env::join_paths(dirs).unwrap();
        assert!(on_path(Some(path(&[&empty, &with]).as_os_str())));
        assert!(!on_path(Some(path(&[&empty, &plain]).as_os_str())));
        assert!(!on_path(None));
    }
}
