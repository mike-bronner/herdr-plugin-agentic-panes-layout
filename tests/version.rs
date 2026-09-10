//! `--version`: what this binary is, and whether it matches the manifest.
//!
//! It exists because a running binary was found two commits behind its source. The
//! manifest Herdr held was current, the compiled artifact was not, and two features were
//! registered without actually running. Diagnosing that took three commands and an
//! inference; it should take one.
//!
//! The crate version alone would not have caught it. Under this repository's release
//! convention the version only moves on a release commit, so the stale binary and the
//! current manifest both read 0.3.0. The commit is what tells them apart within a
//! release, and the manifest comparison is what tells them apart across one.

mod support;

use std::process::Command;

use support::*;

fn version(extra: &[(&str, &str)]) -> (String, i32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .arg("--version")
        .env_clear()
        .env("PATH", LAUNCHD_PATH);
    for (k, v) in extra {
        command.env(k, v);
    }
    let out = command.output().expect("cannot run the binary");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn it_reports_the_crate_version_the_commit_and_the_build_time() {
    let (shown, code) = version(&[]);
    assert_eq!(code, 0, "{}", shown);

    let first = shown.lines().next().unwrap_or_default();
    assert!(
        first.contains(env!("CARGO_PKG_VERSION")),
        "the crate version must appear: {:?}",
        first
    );
    assert!(first.contains("built "), "{:?}", first);
    // An ISO 8601 UTC instant, not a locale-dependent string somebody has to interpret.
    assert!(
        first.contains('T') && first.contains('Z'),
        "the build time must be ISO 8601 UTC: {:?}",
        first
    );
}

#[test]
fn the_build_time_is_a_real_date_rather_than_the_epoch() {
    // The date maths is hand-rolled to avoid a crate for one line of output, so it is
    // worth pinning that it produces a plausible instant rather than 1970.
    let (shown, _) = version(&[]);
    let first = shown.lines().next().unwrap_or_default();
    let year: i32 = first
        .split("built ")
        .nth(1)
        .and_then(|s| s.get(..4))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(
        (2026..2100).contains(&year),
        "implausible build year in {:?}",
        first
    );
}

#[test]
fn the_commit_is_reported_and_is_not_empty() {
    // Either a real short hash or the word `unknown`. Both are answers; an empty field
    // would be neither.
    let (shown, _) = version(&[]);
    let first = shown.lines().next().unwrap_or_default();
    let commit = first
        .split('(')
        .nth(1)
        .and_then(|s| s.split(',').next())
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(!commit.is_empty(), "{:?}", first);
    assert!(
        commit == "unknown" || commit.chars().next().is_some_and(|c| c.is_ascii_hexdigit()),
        "expected a short hash or `unknown`, got {:?}",
        commit
    );
}

#[test]
fn it_reports_the_manifest_version_from_disk() {
    // A different fact from the crate version, not a duplicate of it. CARGO_PKG_VERSION
    // is baked in at compile time; this is read at run time, and it is what Herdr itself
    // reads to decide what the plugin is.
    let dir = TempDir::new();
    std::fs::write(
        dir.join("herdr-plugin.toml"),
        "id = \"x\"\nversion = \"9.9.9\"\n",
    )
    .unwrap();
    let (shown, code) = version(&[("HERDR_PLUGIN_ROOT", dir.path().to_str().unwrap())]);
    assert_eq!(code, 0, "{}", shown);
    assert!(shown.contains("manifest 9.9.9"), "{}", shown);
    assert!(
        shown.contains(&dir.join("herdr-plugin.toml").display().to_string()),
        "it must say WHERE it read that from: {}",
        shown
    );
}

#[test]
fn a_binary_older_than_its_manifest_is_called_stale_outright() {
    // Mike's failure, one release later. The manifest moves on a release commit and the
    // binary does not get rebuilt, so the two numbers diverge. Saying it plainly beats
    // printing two numbers and leaving the reader to compare them, which invites a bug
    // report rather than a diagnosis.
    let dir = TempDir::new();
    std::fs::write(
        dir.join("herdr-plugin.toml"),
        "id = \"x\"\nversion = \"99.0.0\"\n",
    )
    .unwrap();
    let (shown, code) = version(&[("HERDR_PLUGIN_ROOT", dir.path().to_str().unwrap())]);
    assert_eq!(code, 0, "a stale binary still reports rather than failing");
    assert!(shown.contains("STALE"), "{}", shown);
    assert!(shown.contains(env!("CARGO_PKG_VERSION")), "{}", shown);
    assert!(shown.contains("99.0.0"), "{}", shown);
    assert!(
        shown.contains("cargo build --release"),
        "it must say what to do about it: {}",
        shown
    );
}

#[test]
fn matching_versions_are_not_called_stale() {
    let dir = TempDir::new();
    std::fs::write(
        dir.join("herdr-plugin.toml"),
        format!("id = \"x\"\nversion = \"{}\"\n", env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    let (shown, _) = version(&[("HERDR_PLUGIN_ROOT", dir.path().to_str().unwrap())]);
    assert!(!shown.contains("STALE"), "{}", shown);
}

#[test]
fn an_unreadable_manifest_degrades_rather_than_failing() {
    // This is the command that has to work when everything else is broken. One that
    // errored because it could not find its own manifest would be worse than one that
    // says less.
    let dir = TempDir::new();
    let (shown, code) = version(&[("HERDR_PLUGIN_ROOT", dir.path().to_str().unwrap())]);
    assert_eq!(code, 0, "{}", shown);
    assert!(shown.contains("manifest unknown"), "{}", shown);
    assert!(shown.contains(env!("CARGO_PKG_VERSION")), "{}", shown);
}

#[test]
fn an_unparseable_manifest_degrades_rather_than_failing() {
    let dir = TempDir::new();
    std::fs::write(dir.join("herdr-plugin.toml"), "this is not toml [[[\n").unwrap();
    let (shown, code) = version(&[("HERDR_PLUGIN_ROOT", dir.path().to_str().unwrap())]);
    assert_eq!(code, 0, "{}", shown);
    assert!(shown.contains("manifest unknown"), "{}", shown);
}

#[test]
fn it_reaches_no_socket_and_needs_no_server() {
    // "What is this?" must be answerable when everything else is broken, so it is
    // handled before anything is resolved. A live socket is offered and must go untouched.
    let stub = Stub::start(Script::default());
    let (shown, code) = version(&[("HERDR_SOCKET_PATH", stub.socket().to_str().unwrap())]);
    assert_eq!(code, 0, "{}", shown);
    assert!(
        stub.requests().is_empty(),
        "--version sent {:?}",
        stub.methods()
    );
}

#[test]
fn the_short_form_matches_the_long_one() {
    let (long, _) = version(&[]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    let out = command
        .arg("-V")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), long);
}

#[test]
fn the_flag_is_refused_twice_like_any_other() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--version", "--version"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("given more than once"), "{}", run.stderr);
}

#[test]
fn the_build_script_watches_both_the_sources_and_the_commit() {
    // Neither cargo default is right. With no rerun-if-changed at all, this reruns on a
    // package change and misses a commit made with no file edits, embedding a stale
    // commit. With only the git paths, a source edit would not rerun it and the build
    // time would predate the binary beside it.
    let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).unwrap();
    assert!(
        build.contains("cargo:rerun-if-changed=src"),
        "the sources must be watched, or the build time goes stale"
    );
    assert!(
        build.contains("HEAD"),
        "the commit must be watched, or the commit goes stale"
    );
}

#[test]
fn the_build_script_never_fails_for_want_of_git() {
    // Herdr reports build failures and installs no toolchains, so failing a build for a
    // diagnostic string would be a bad trade. Verified for real by building a copy of
    // this crate outside any repository: it compiled and reported the commit as
    // `unknown`. Pinned here so the degradation is not refactored into a `?` or an
    // `unwrap`.
    let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).unwrap();
    assert!(build.contains("\"unknown\""), "{}", build);
    assert!(
        !build.contains(".expect(") && !build.contains(".unwrap()"),
        "the build script must not panic on a missing git or a missing repository"
    );
}
