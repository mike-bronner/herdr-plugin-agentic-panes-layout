//! `--check` driven through the real binary, with no server anywhere.
//!
//! The unit tests in src/check.rs cover what the report says. These cover the
//! property that makes it worth having: it reaches config parsing without a socket,
//! and it is safe to run against a config you already suspect.
//!
//! A real run cannot do this. It resolves the workspace before it reads the config,
//! so pointing it at a dead socket gives a workspace error rather than a config one.
//! That ordering is why the only way to find out whether a config was good used to be
//! to create a worktree.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::*;

/// Mike's own config, verbatim, as he hand-wrote it after v0.3.0 shipped.
const MIKES_BROKEN_CONFIG: &str = r#"default = "agentic-layout"

[[layouts.home.tabs]]
path="~"
name="shells"

[[layouts.home.tabs.panes]]
split="right"
ratio=0.5

[[layouts.agentic-layout.tabs]]
name = "agent"

[[layouts.agentic-layout.tabs.panes]]
agent = "claude"

[[layouts.agentic-layout.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"

[[layouts.agentic-layout.tabs.panes]]
split = "down"
ratio = 0.7
label = "shell"
"#;

struct Checked {
    stdout: String,
    stderr: String,
    status: i32,
}

/// Run `--check` with an environment built from scratch and **no socket variable**.
fn check(toml: Option<&str>, extra: &[(&str, &str)]) -> (Checked, TempDir) {
    let dir = TempDir::new();
    let root = dir.join("herdr");
    std::fs::create_dir_all(&root).unwrap();
    if let Some(toml) = toml {
        std::fs::write(root.join("agent-layout.toml"), toml).unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .arg("--check")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", dir.path())
        .env("HERDR_CONFIG_PATH", &root);
    for (k, v) in extra {
        command.env(k, v);
    }
    let out = command.output().expect("cannot run the check");
    (
        Checked {
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            status: out.status.code().unwrap_or(-1),
        },
        dir,
    )
}

#[test]
fn the_check_needs_no_socket_at_all() {
    // HERDR_SOCKET_PATH is not set, and this still works. The contrast test below
    // shows a real run in the same environment refusing.
    let (out, _dir) = check(Some(MIKES_BROKEN_CONFIG), &[]);
    assert!(
        !out.stderr.contains("HERDR_SOCKET_PATH"),
        "the check must not want a socket: {}",
        out.stderr
    );
    assert!(out.stdout.contains("problems found"), "{}", out.stdout);
}

#[test]
fn a_real_run_in_the_same_environment_refuses_for_want_of_a_socket() {
    // The contrast that gives the test above its meaning. Without it, "the check needs
    // no socket" would pass just as well on a binary that never needed one anywhere.
    let dir = TempDir::new();
    let out = Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", dir.path())
        .output()
        .expect("cannot run the binary");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("HERDR_SOCKET_PATH is not set"));
}

#[test]
fn the_check_opens_no_socket_even_when_one_is_offered() {
    // Stronger than the absence of the variable: a live socket is handed to it and
    // must go untouched. A check that quietly listed workspaces would still pass a
    // "no variable set" test.
    let stub = Stub::start(Script::default());
    let (out, _dir) = check(
        Some(MIKES_BROKEN_CONFIG),
        &[("HERDR_SOCKET_PATH", stub.socket().to_str().unwrap())],
    );
    assert_eq!(out.status, 1, "{}", out.stdout);
    assert!(
        stub.requests().is_empty(),
        "the check sent {} request(s): {:?}",
        stub.requests().len(),
        stub.methods()
    );
}

#[test]
fn the_check_creates_nothing_on_disk_beyond_what_it_was_given() {
    // "Side-effect free" includes not writing a corrected file, a backup, or a cache.
    let (_out, dir) = check(Some(MIKES_BROKEN_CONFIG), &[]);
    let mut found: Vec<String> = Vec::new();
    walk(dir.path(), &mut |p| {
        found.push(p.strip_prefix(dir.path()).unwrap().display().to_string())
    });
    assert_eq!(
        found,
        vec!["herdr/agent-layout.toml".to_string()],
        "{:?}",
        found
    );
}

fn walk(dir: &Path, seen: &mut impl FnMut(&PathBuf)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, seen);
        } else {
            seen(&path);
        }
    }
}

#[test]
fn mikes_config_fails_the_check_and_explains_both_mistakes() {
    let (out, _dir) = check(Some(MIKES_BROKEN_CONFIG), &[]);
    assert_eq!(out.status, 1, "{}", out.stdout);
    assert!(out.stdout.contains("2 problems found"), "{}", out.stdout);
    assert!(
        out.stdout.contains("write TWO pane blocks"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("[[projects]] key, not a tab key"),
        "{}",
        out.stdout
    );
}

#[test]
fn mikes_broken_layout_no_longer_takes_his_working_one_with_it() {
    // This used to discard the whole file, so his correct `agentic-layout` was inactive
    // and the run looked like it had worked. Now only `home` is skipped.
    let (out, _dir) = check(Some(MIKES_BROKEN_CONFIG), &[]);
    assert!(out.stdout.contains("home  SKIPPED"), "{}", out.stdout);
    assert!(
        out.stdout.contains("agentic-layout  usable"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("Layout \"agentic-layout\""),
        "his own layout must be the one applied: {}",
        out.stdout
    );
}

#[test]
fn the_projects_hint_points_at_a_place_that_now_works() {
    // Mike's `path = "~"` meant "use this layout at home", and that is now possible. An
    // earlier version of this hint said a non-repository could not be matched, which
    // was true then and would be a trap now.
    let (out, _dir) = check(Some(MIKES_BROKEN_CONFIG), &[]);
    assert!(
        out.stdout.contains("[[projects]] key, not a tab key"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("a plain directory works as well as a repository"),
        "the hint must say a plain directory can match: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("cannot be matched"),
        "the old, now-false limit must be gone: {}",
        out.stdout
    );
}

#[test]
fn a_plain_directory_offers_itself_as_something_a_rule_can_match() {
    // The feature. The temp directory is not a git repository, and it is still a
    // candidate, which is what lets a rule select a layout for a home directory.
    let (out, dir) = check(
        Some(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
              [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
        ),
        &[],
    );
    assert!(
        out.stdout.contains(&dir.path().display().to_string()),
        "the working directory must be offered as a candidate: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("no repository here"),
        "the old message claimed nothing could match here: {}",
        out.stdout
    );
}

#[test]
fn a_rule_naming_a_plain_directory_selects_its_layout() {
    // Mike's intent for `home`, proven end to end before he is told to write it: two
    // shells side by side at 50/50, chosen by a rule naming a directory that is not a
    // repository.
    let dir = TempDir::new();
    let root = dir.join("herdr");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("agent-layout.toml"),
        format!(
            "default = \"other\"\n\n\
             [[projects]]\npath = \"{}\"\nlayout = \"home\"\n\n\
             [[layouts.home.tabs]]\nname = \"shells\"\n\
             [[layouts.home.tabs.panes]]\n\
             [[layouts.home.tabs.panes]]\nsplit = \"right\"\nratio = 0.5\n\n\
             [[layouts.other.tabs]]\nname = \"other\"\n\
             [[layouts.other.tabs.panes]]\nagent = \"claude\"\n",
            dir.path().display()
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .arg("--check")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", dir.path())
        .env("HERDR_CONFIG_PATH", &root)
        .output()
        .expect("cannot run the check");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    assert_eq!(out.status.code(), Some(0), "{}", stdout);
    assert!(
        stdout.contains("Layout \"home\", matched by the [[projects]] rule"),
        "the rule must select the layout: {}",
        stdout
    );
    assert!(stdout.contains("tab \"shells\""), "{}", stdout);
    assert!(
        stdout.contains("pane 2: split right from pane 1, pane 1 keeps 0.5"),
        "two panes side by side at 50/50: {}",
        stdout
    );
}

#[test]
fn no_config_file_is_reported_as_such_rather_than_as_a_failure() {
    // A fresh install is not broken, so it must not fail the check.
    let (out, _dir) = check(None, &[]);
    assert_eq!(out.status, 0, "{}", out.stdout);
    assert!(out.stdout.contains("config: none at"), "{}", out.stdout);
    assert!(out.stdout.contains("Layout \"built-in\""), "{}", out.stdout);
}

#[test]
fn a_good_config_passes_and_names_the_file_it_read() {
    let (out, _dir) = check(
        Some(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"work\"\n\
              [[layouts.one.tabs.panes]]\nagent = \"codex\"\nlabel = \"here\"\n",
        ),
        &[],
    );
    assert_eq!(out.status, 0, "{}", out.stdout);
    assert!(out.stdout.contains("agent-layout.toml"), "{}", out.stdout);
    assert!(out.stdout.contains("No problems found."), "{}", out.stdout);
    assert!(out.stdout.contains("tab \"work\""), "{}", out.stdout);
    assert!(
        out.stdout.contains("runs the codex agent"),
        "{}",
        out.stdout
    );
}

#[test]
fn the_check_flag_is_refused_twice_like_any_other() {
    let (out, _dir) = check(None, &[]);
    assert_eq!(out.status, 0, "{}", out.stdout);

    let dir = TempDir::new();
    let twice = Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .args(["--check", "--check"])
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", dir.path())
        .output()
        .expect("cannot run the binary");
    assert_eq!(twice.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&twice.stderr).contains("given more than once"));
}
