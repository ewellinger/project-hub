use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::hub::Hub;
use crate::ops::resolve::{AddOptions, Created, open_change};
use crate::ops::session::{after_save, change_path, converge_tmux, write_workspace};

pub fn add(hub: &Hub, feature: &mut Feature, opts: &AddOptions) -> Result<Vec<String>> {
    feature.require_open()?;
    // Transaction: branch, worktree, and the record write succeed together or
    // are rolled back together. Workspace and tmux come after the write.
    let mut created = Created::new();
    let saved = open_change(hub, feature, opts, &mut created).and_then(|opened| {
        let mut lines = vec![format!(
            "{}: {} at {}",
            opts.role,
            opened.change.branch,
            opened.path.display()
        )];
        lines.extend(opened.lines.into_iter().map(|l| format!("  {l}")));
        feature.push_change(opened.change)?;
        hub.save_feature(feature)?;
        Ok(lines)
    });
    let mut lines = match saved {
        Ok(lines) => lines,
        Err(err) => {
            let mut warnings = created.rollback();
            if warnings.is_empty() {
                return Err(err);
            }
            warnings.insert(0, err.to_string());
            return Err(HubError::Precondition(warnings.join("\n")));
        }
    };
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );

    // A window left over from a merged change points at the main clone; re-point it.
    let session = feature.checkout.clone();
    let role = opts.role.to_string();
    let tmux_step = (|| -> Result<Vec<String>> {
        let had_window = match &hub.tmux {
            Some(tmux) => {
                tmux.has_session(&session)? && tmux.windows(&session)?.iter().any(|w| w == &role)
            }
            None => false,
        };
        let mut out = converge_tmux(hub, feature)?;
        if let Some(tmux) = hub.tmux.as_ref().filter(|_| had_window) {
            let change = feature.open_change(&role).expect("just pushed");
            tmux.respawn_window(&session, &role, &change_path(hub, change))?;
            out.push(format!("Re-pointed tmux window {role} at the new worktree"));
        }
        Ok(out)
    })();
    after_save(
        &mut lines,
        tmux_step,
        &format!("hub tmux --feature {}", feature.name),
    );
    Ok(lines)
}
