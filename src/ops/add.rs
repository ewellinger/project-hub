use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::git::Git;
use crate::hub::Hub;
use crate::ops::freeze::{frozen_removal_line, require_on_pin};
use crate::ops::merged::refuse_in_use;
use crate::ops::resolve::{AddOptions, Created, open_change};
use crate::ops::session::{
    after_save, change_path, converge_tmux, pin_path, repoint_window, write_workspace,
};

pub fn add(hub: &Hub, feature: &mut Feature, opts: &AddOptions) -> Result<Vec<String>> {
    feature.require_open()?;
    // A frozen reference role gives its worktree up to the change.
    let frozen = feature
        .pin(opts.role)
        .filter(|p| p.worktree.is_some())
        .cloned();
    let frozen_path = frozen
        .as_ref()
        .and_then(|p| pin_path(hub, p))
        .filter(|p| p.exists());
    if let Some(pin) = &frozen
        && opts.keeps_pin()
    {
        // The effective change base: a `hub repo set --base` after the
        // freeze moves it as surely as `--base` does.
        let base = match opts.base {
            Some(b) => b.to_string(),
            None => hub.manifest.require_repo(opts.role)?.base.clone(),
        };
        if base != pin.base {
            return Err(HubError::Precondition(format!(
                "role '{}' is frozen at {} on {}; a change merging into {base} cannot keep that pin. \
                 Pass --from-latest to drop it, or run `hub feature unfreeze {}` first",
                opts.role,
                pin.short(),
                pin.base,
                opts.role
            )));
        }
    }
    if let (Some(pin), Some(path)) = (&frozen, &frozen_path) {
        let git = Git::new(path);
        if git.is_dirty()? {
            return Err(HubError::Dirty {
                path: path.clone(),
                status: git.status_short()?,
            });
        }
        // Same rule as EW-95's refreeze/unfreeze/finish: commits made on
        // the detached HEAD would be orphaned by the removal below.
        require_on_pin(path, pin)?;
    }

    // Transaction: branch, worktree, and the record write succeed together or
    // are rolled back together. `open_change` runs before anything touches
    // the role's tmux window, so a failed fetch or a taken name leaves the
    // window and its processes alone. The frozen reference worktree is
    // removed only after the write, so every failure before it leaves the
    // role frozen as recorded. Workspace and tmux come after the write.
    let mut lines = Vec::new();
    let mut created = Created::new();
    let mut repointed = false;
    let saved = (|| -> Result<()> {
        let opened = open_change(hub, feature, opts, &mut created)?;
        if let (Some(pin), Some(path)) = (&frozen, &frozen_path) {
            // The role's window sits in the reference worktree; free it the
            // way `unfreeze` does before checking for processes.
            repointed = true;
            after_save(
                &mut lines,
                repoint_window(hub, feature, opts.role, &hub.env.clone_path(&pin.clone)),
                &format!("hub tmux --feature {}", feature.name),
            );
            lines.extend(refuse_in_use(std::slice::from_ref(path), false)?);
        }
        lines.push(format!(
            "{}: {} at {}",
            opts.role,
            opened.change.branch,
            opened.path.display()
        ));
        lines.extend(opened.lines.into_iter().map(|l| format!("  {l}")));
        feature.push_change(opened.change)?;
        feature.remove_pin(opts.role);
        feature.pins.extend(opened.pin);
        hub.save_feature(feature)
    })();
    if let Err(err) = saved {
        let mut warnings = created.rollback();
        if repointed
            && let Some(path) = &frozen_path
            && let Err(e) = repoint_window(hub, feature, opts.role, path)
        {
            warnings.push(format!(
                "rollback: could not re-point tmux window {} at {}: {e}",
                opts.role,
                path.display()
            ));
        }
        if warnings.is_empty() {
            return Err(err);
        }
        warnings.insert(0, err.to_string());
        return Err(HubError::Precondition(warnings.join("\n")));
    }

    // The record no longer names the reference worktree: a failed removal
    // leaves a stray directory, never a wrong record.
    if let (Some(pin), Some(path)) = (&frozen, &frozen_path) {
        match Git::new(hub.env.clone_path(&pin.clone)).worktree_remove(path, false) {
            Ok(removal) => lines.push(frozen_removal_line(&removal, path, opts.role)),
            Err(e) => lines.push(format!(
                "warning: {}: frozen worktree {} is no longer recorded but was not removed: {e}; inspect it and remove it with `git worktree remove`",
                opts.role,
                path.display()
            )),
        }
    }
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );

    // A window left over from a merged change or a frozen role points
    // elsewhere; re-point it.
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
