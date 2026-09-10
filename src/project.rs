//! Working out which paths a workspace could be matched by.
//!
//! `[[projects]]` rules name one `path`, and it is matched against a workspace's
//! checkout path **and** its repo root, so naming a repository covers every worktree
//! of it. Those two come from Herdr's `worktree` object when it has one.
//!
//! It often does not. Measured on 0.9.0: the object is present only for spaces Herdr
//! created through its worktree path, and `is_linked_worktree: false` does not mean
//! "plain space". So git is the fallback, and it is also the only source available to
//! `--check`, which has no server to ask.

use std::process::Command;

/// Every path a `[[projects]]` rule could match this workspace by.
///
/// Sorted and deduplicated so the order does not depend on which sources answered.
///
/// **The workspace's own directory is a candidate**, which is what lets a rule select
/// a layout for a plain directory that is not a repository at all. Before that, every
/// candidate came from git or from Herdr's worktree object, so a rule naming a home
/// directory could never match and failed silently.
///
/// Note that candidate **order carries no meaning**. Rules are evaluated in file order
/// and the first matching rule wins, so precedence is a property of the file rather
/// than of which source produced a path. Adding the working directory therefore widens
/// what a rule can match without changing which rule wins when two of them do.
pub fn candidates(checkout_path: Option<&str>, repo_root: Option<&str>, cwd: &str) -> Vec<String> {
    let mut found: Vec<String> = [checkout_path, repo_root]
        .into_iter()
        .flatten()
        .map(|p| p.to_string())
        .collect();

    if !cwd.is_empty() {
        found.push(cwd.to_string());
    }

    if let Some(toplevel) = git(cwd, &["rev-parse", "--show-toplevel"]) {
        found.push(toplevel);
    }
    if let Some(common) = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        // A linked worktree's common dir is the main repo's `.git`, so stripping that
        // suffix gives the repo root a rule is most likely to name.
        found.push(
            common
                .strip_suffix("/.git")
                .map(|s| s.to_string())
                .unwrap_or(common),
        );
    }

    found.sort();
    found.dedup();
    found
}

/// Absolute path, because Herdr's server runs under launchd with a bare `PATH`.
pub fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
