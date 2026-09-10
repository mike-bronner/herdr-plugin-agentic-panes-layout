//! Captures what this binary was built from, for `--version`.
//!
//! The reason it exists: a running binary was found two commits behind its source. The
//! manifest Herdr held was current, the compiled artifact was not, and two features
//! were registered without actually running. A crate version alone would not have
//! caught it, because under this repository's release convention the version only moves
//! on a release commit, so the stale binary and the current manifest both read 0.3.0.
//! **The commit is what tells them apart.**
//!
//! Diagnosing it took three commands and an inference. It should take one.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Everything that can change what gets compiled.
///
/// **This one list defines the marker's meaning**, and both halves use it: the paths
/// cargo watches for a rebuild, and the pathspec `git status` is asked about. Having
/// those two differ is what made the earlier version incoherent — it watched `src` but
/// asked about the whole tree, so editing the README could make the marker say dirty
/// while nothing rebuilt to notice, and editing `Cargo.toml` could leave it saying clean.
///
/// The meaning chosen is **what was compiled**, not what `git status` says about the
/// tree. A hash on this binary claims it was built from that commit, and only these
/// files can make that claim false. A modified README casts no doubt on the artifact, so
/// reporting it as dirty would be noise in a field whose whole job is signal.
const BUILD_INPUTS: [&str; 4] = ["src", "build.rs", "Cargo.toml", "Cargo.lock"];

fn main() {
    // WHEN TO RERUN. The build inputs, and the git head, and nothing else.
    //
    // With no directives at all, cargo reruns this whenever a package file changes. That
    // misses a commit made with no file edits, because `.git` is not a package file, and
    // this would then embed a stale commit — which would be a poor joke given what it is
    // for.
    //
    // With only the git directives, a source edit would not rerun this, and the build
    // timestamp would be older than the binary beside it.
    //
    // So: both. The build inputs keep the timestamp and the dirty marker honest, the git
    // files catch commits and branch switches. This script runs two short commands, so
    // rerunning it often costs nothing worth optimising.
    //
    // `.git/index` is deliberately absent. Staging a file moves it from unstaged to
    // staged in `git status --porcelain` and leaves the output non-empty either way, so
    // the marker cannot flip on a bare `git add`. Verified rather than assumed.
    //
    // **`rerun-if-changed` is keyed on mtimes**, so a bare `touch Cargo.toml` reruns this
    // script and refreshes the timestamp even though nothing resolved changed. Measured
    // 2026-09-10 against a control: two consecutive builds held the embedded build time
    // still, and a `touch` moved it.
    //
    // An earlier note here claimed the opposite, on the strength of cargo's PACKAGE
    // fingerprint being computed from the parsed manifest rather than its bytes. That
    // mechanism is real and it is not this one. It stopped being the mechanism in force
    // the moment `Cargo.toml` joined BUILD_INPUTS and started feeding these directives,
    // and the note survived the change describing a path we no longer take.
    for input in BUILD_INPUTS {
        println!("cargo:rerun-if-changed={}", input);
    }
    for path in git_watch_paths() {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    println!("cargo:rustc-env=AGENT_LAYOUT_COMMIT={}", commit());
    println!("cargo:rustc-env=AGENT_LAYOUT_BUILT_AT={}", built_at());
}

/// The git files whose change means the commit changed.
///
/// `HEAD` covers a branch switch and a detached checkout. **`refs` is watched as a whole
/// directory**, which covers a commit on the current branch, and it is watched as a
/// directory on purpose.
///
/// The obvious version names the one file `HEAD` points at, and it has a hole: `git gc`
/// packs the refs and deletes `.git/refs/heads/<branch>`. Two things then go wrong at
/// once, and the second is the expensive one.
///
/// **A directive naming a missing path reruns this script on every single build.**
/// Measured 2026-09-10, correcting an earlier claim here that cargo ignores such a
/// directive: adding one for a path that does not exist moved the embedded build time on
/// three consecutive builds, where the control held it still.
///
/// And watching the path is what would have to be dropped to fix that — at which point a
/// later commit writes the loose ref back, nothing is watching for it, and the binary
/// keeps a hash it was not built from. A stale hash is the failure this whole field
/// exists to catch, so that trade is not available.
///
/// A directory watch closes both. `.git/refs` survives `gc` (the subdirectories stay,
/// empty), so no directive ever names a missing path, and a loose ref written back after
/// a `gc` is seen. It costs one extra rerun per `git fetch`, which writes remote refs
/// underneath; this script runs two short git commands, so that is the cheap side.
///
/// `.git/packed-refs` is deliberately not watched. A ref that MOVES is always written
/// loose — only `gc` and `pack-refs` write that file, and they do not change what HEAD
/// resolves to.
///
/// **Both paths are resolved by git, not built by joining onto a literal `.git`.**
/// That is for the linked-worktree case, and it is the whole reason a subprocess call
/// stands where a path join would do.
///
/// **In a linked worktree `.git` is a FILE**, and the real git directory is elsewhere.
/// Measured 2026-09-10 in a worktree of this repository: `.git` is a 90-byte file, so a
/// bare `Path::new(".git").exists()` says yes, and `.git/HEAD` and `.git/refs` then both
/// fail to exist. Joining would leave **nothing at all watched**, and a commit would
/// rerun nothing — a silently stale hash, which is precisely the failure this field
/// exists to catch, arriving in the checkout layout this plugin is written to serve.
///
/// `--git-path` returns `.git/HEAD` and `.git/refs` unchanged in a normal checkout, the
/// per-worktree `HEAD` in a worktree, and the **shared** `refs` directory — which is
/// where a worktree's own branch ref lives, so one directory watch still covers a commit
/// made in either place. Outside a repository it exits 128, so a source tarball watches
/// nothing rather than naming files that are not there.
fn git_watch_paths() -> Vec<PathBuf> {
    ["HEAD", "refs"]
        .into_iter()
        .filter_map(|name| git(&["rev-parse", "--git-path", name]))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

/// The commit this was built from, or `unknown`.
///
/// **Never fails the build.** Git may be absent from the build environment, and the
/// plugin root may not be a repository at all: `herdr plugin install` clones, but a
/// source tarball would not. Herdr reports build failures and installs no toolchains,
/// so failing a build for want of a diagnostic string would be a bad trade.
///
/// A dirty tree is marked. Mike did not ask for that, and it is included because it is
/// the case a bare hash silently misrepresents: a binary built from uncommitted changes
/// reports a commit whose source is not what was compiled. That is the same class of
/// failure as the stale binary this whole flag exists to catch, so hiding it here would
/// undercut the point. It costs one word.
///
/// **Scoped to `BUILD_INPUTS`**, so the question asked matches the question the marker
/// answers. See that constant for why the meaning is "what was compiled" rather than
/// "what `git status` says".
fn commit() -> String {
    let Some(short) = git(&["rev-parse", "--short", "HEAD"]) else {
        return "unknown".to_string();
    };
    let mut args = vec!["status", "--porcelain", "--"];
    args.extend(BUILD_INPUTS);
    match git(&args) {
        Some(changes) if !changes.is_empty() => format!("{}-dirty", short),
        // A failed `status` is not evidence of a clean tree, so it is not claimed as one.
        None => format!("{}-unverified", short),
        Some(_) => short,
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Build time as UTC ISO 8601, computed rather than shelled out for.
///
/// `date` would be a second external command that can be absent, for something the
/// standard library can already answer.
fn built_at() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's days-from-civil, inverted. Public-domain algorithm, exact for any
/// date this will ever see, and it avoids a date crate for one line of output.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
