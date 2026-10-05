use std::path::{Component, Path};

use crate::error::{HubError, Result};

pub const DEFAULT_CHECKOUT_TEMPLATE: &str = "{feature}";
pub const DEFAULT_BRANCH_TEMPLATE: &str = "{feature}";
const CHECKOUT_PLACEHOLDERS: [&str; 3] = ["{feature}", "{feature_snake}", "{hub}"];

/// `^[A-Za-z0-9][A-Za-z0-9_-]*$`: valid as a git branch, a tmux session, and a directory.
pub fn validate_name(kind: &str, value: &str) -> Result<()> {
    let mut chars = value.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => chars.all(is_name_char),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(HubError::Usage(format!(
            "invalid {kind} '{value}': must match ^[A-Za-z0-9][A-Za-z0-9_-]*$"
        )))
    }
}

/// `^[A-Za-z][A-Za-z0-9_-]*$`; `hub` and `ai` are reserved for the fixed tmux windows.
pub fn validate_role(role: &str) -> Result<()> {
    let mut chars = role.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => chars.all(is_name_char),
        _ => false,
    };
    if !ok {
        return Err(HubError::Usage(format!(
            "invalid role '{role}': must match ^[A-Za-z][A-Za-z0-9_-]*$"
        )));
    }
    if role == "hub" || role == "ai" {
        return Err(HubError::Usage(format!(
            "invalid role '{role}': 'hub' and 'ai' are reserved for the fixed tmux windows"
        )));
    }
    Ok(())
}

/// `[A-Za-z0-9-]+`
pub fn validate_clone(clone: &str) -> Result<()> {
    if !clone.is_empty() && clone.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(())
    } else {
        Err(HubError::Usage(format!(
            "invalid clone '{clone}': must match [A-Za-z0-9-]+"
        )))
    }
}

/// Exactly one normal path component: no separators, not `.` or `..`, not absolute.
pub fn validate_worktree_name(value: &str) -> Result<()> {
    let mut components = Path::new(value).components();
    let single =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if single && !value.contains('/') && !value.contains('\\') {
        Ok(())
    } else {
        Err(HubError::Usage(format!(
            "invalid worktree name '{value}': must be a single directory name, not a path"
        )))
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub fn has_checkout_placeholder(template: &str) -> bool {
    CHECKOUT_PLACEHOLDERS.iter().any(|p| template.contains(p))
}

pub fn checkout_name(template: &str, feature: &str, hub: &str) -> String {
    template
        .replace("{feature_snake}", &feature.replace('-', "_"))
        .replace("{feature}", feature)
        .replace("{hub}", hub)
}

pub fn branch_name(template: &str, feature: &str) -> String {
    template.replace("{feature}", feature)
}

/// `feature/foo` -> `feature-foo`
pub fn flatten_branch(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Name for a follow-up branch after `previous` merged. `earlier` are the
/// branches of earlier changes for the same role in the same feature; a
/// trailing `-<n>` on `previous` is only an ordinal if the stem is one of them.
pub fn next_branch(previous: &str, earlier: &[&str], exists: impl Fn(&str) -> bool) -> String {
    let (stem, start) = match previous.rsplit_once('-') {
        Some((stem, n))
            if earlier.contains(&stem)
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit()) =>
        {
            (stem, n.parse::<u64>().map(|n| n + 1).unwrap_or(2))
        }
        _ => (previous, 2),
    };
    let mut n = start;
    loop {
        let candidate = format!("{stem}-{n}");
        if !exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// `git check-ref-format --branch` without the subprocess: non-empty, no
/// whitespace or control characters, none of `~ ^ : ? * [ \`, no `..` or
/// `@{`, no component starting with `.` or ending with `.lock`, no leading
/// or trailing `/` or `.`, no empty component, no leading `-`. Additionally
/// rejects the lone `@`, which `git check-ref-format --branch` itself
/// accepts (its rule only bites a full ref that is exactly `@`, and
/// `refs/heads/@` never is) but which every git command that takes a
/// revision treats as shorthand for HEAD.
pub fn validate_branch_name(value: &str) -> Result<()> {
    let bad = |why: &str| {
        Err(HubError::Usage(format!(
            "'{value}' is not a valid branch name: {why}"
        )))
    };
    if value.is_empty() {
        return Err(HubError::Usage("a branch name is required".into()));
    }
    if value == "@" {
        return bad("'@' alone is git's shorthand for HEAD");
    }
    if value.starts_with('-') {
        return bad("must not start with '-'");
    }
    if value.starts_with('/') || value.ends_with('/') {
        return bad("must not start or end with '/'");
    }
    if value.ends_with('.') {
        return bad("must not end with '.'");
    }
    if value.contains("..") || value.contains("@{") || value.contains("//") {
        return bad("must not contain '..', '@{' or '//'");
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return bad("must not contain whitespace or any of ~ ^ : ? * [ \\");
    }
    for part in value.split('/') {
        if part.starts_with('.') {
            return bad("no path component may start with '.'");
        }
        if part.ends_with(".lock") {
            return bad("no path component may end with '.lock'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_accept_the_documented_pattern() {
        for ok in ["feat-1", "acme-port_e2e", "9lives", "A"] {
            validate_name("feature name", ok).unwrap();
        }
        for bad in ["", "-x", "_x", "a.b", "a:b", "a/b", "a b"] {
            let err = validate_name("feature name", bad).unwrap_err();
            assert!(
                matches!(err, HubError::Usage(m) if m.contains("feature name")),
                "{bad}"
            );
        }
    }

    #[test]
    fn roles_must_start_with_a_letter() {
        validate_role("api").unwrap();
        validate_role("ui-2").unwrap();
        assert!(validate_role("1ui").is_err());
        assert!(validate_role("").is_err());
        assert!(validate_role("a.b").is_err());
    }

    #[test]
    fn roles_named_hub_or_ai_are_reserved() {
        for reserved in ["hub", "ai"] {
            let err = validate_role(reserved).unwrap_err();
            assert!(
                matches!(err, HubError::Usage(m) if m.contains("reserved")),
                "{reserved}"
            );
        }
    }

    #[test]
    fn clones_are_alnum_and_dashes() {
        validate_clone("acme-web").unwrap();
        assert!(validate_clone("").is_err());
        assert!(validate_clone("a_b").is_err());
        assert!(validate_clone("a/b").is_err());
    }

    #[test]
    fn worktree_names_are_one_path_component() {
        for ok in [
            "feat-1",
            "release-v1.2",
            "api-review_368",
            "feature-ACME-1-x",
        ] {
            validate_worktree_name(ok).unwrap();
        }
        for bad in ["", ".", "..", "/tmp/x", "../escape", "a/b", "a/", "a\\b"] {
            let err = validate_worktree_name(bad).unwrap_err();
            assert!(
                matches!(err, HubError::Usage(m) if m.contains("worktree name")),
                "{bad}"
            );
        }
    }

    #[test]
    fn checkout_template_substitutes_all_placeholders() {
        assert_eq!(
            checkout_name("acme-{feature_snake}", "port-e2e", "acme"),
            "acme-port_e2e"
        );
        assert_eq!(
            checkout_name("{hub}-{feature}", "port-e2e", "acme"),
            "acme-port-e2e"
        );
        assert_eq!(checkout_name("{feature}", "x", "h"), "x");
        assert!(has_checkout_placeholder("{hub}-x"));
        assert!(!has_checkout_placeholder("static"));
    }

    #[test]
    fn branch_template_substitutes_feature() {
        assert_eq!(branch_name("feat/pc/{feature}", "x"), "feat/pc/x");
        assert_eq!(branch_name("{feature}", "x"), "x");
    }

    #[test]
    fn flatten_replaces_slashes() {
        assert_eq!(flatten_branch("feature/ACME-1/x"), "feature-ACME-1-x");
        assert_eq!(flatten_branch("plain"), "plain");
    }

    #[test]
    fn next_branch_appends_2_by_default() {
        assert_eq!(next_branch("feature/x", &[], |_| false), "feature/x-2");
    }

    #[test]
    fn next_branch_skips_taken_names() {
        let taken = ["feature/x-2", "feature/x-3"];
        assert_eq!(
            next_branch("feature/x", &[], |b| taken.contains(&b)),
            "feature/x-4"
        );
    }

    #[test]
    fn next_branch_continues_an_ordinal_when_the_stem_is_an_earlier_change() {
        assert_eq!(
            next_branch("feature/x-2", &["feature/x"], |_| false),
            "feature/x-3"
        );
    }

    #[test]
    fn next_branch_does_not_treat_ticket_numbers_as_ordinals() {
        assert_eq!(
            next_branch("feature/ACME-123", &[], |_| false),
            "feature/ACME-123-2"
        );
        assert_eq!(
            next_branch("feature/ACME-123-2", &["feature/ACME-123"], |_| false),
            "feature/ACME-123-3"
        );
    }

    #[test]
    fn validate_branch_name_follows_git_ref_rules() {
        for ok in ["feature/x", "acme-1", "a.b", "release/2026-09"] {
            validate_branch_name(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "", " ", "a b", "-x", "/x", "x/", "a//b", "a..b", "x.lock", "a~b", "a^b", "a:b", "a?b",
            "a*b", "a[b", "a\\b", "a.",
            "@", // git check-ref-format --branch accepts this; hub rejects it anyway because git treats it as HEAD shorthand
            "a@{b", "a/.b",
        ] {
            let err = validate_branch_name(bad).unwrap_err();
            assert!(matches!(err, HubError::Usage(_)), "{bad}: {err}");
        }
    }
}
