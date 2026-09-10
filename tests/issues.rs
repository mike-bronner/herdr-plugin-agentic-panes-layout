//! Config problems are shown in a popup pane, on both paths, and never gate the layout.
//!
//! A toast cannot carry this. Measured on 0.9.0 in one isolated server holding Mike's
//! own `ui.toast.delivery = "system"`: `notification.show` answered
//! `{"shown": false, "reason": "no_foreground_client"}` for every call, while
//! `plugin.pane.open` in the same server started the pane's process. So every config
//! diagnostic this plugin ever emitted was dropped before it rendered, which is why a
//! broken config felt silent.

mod support;

use serde_json::{json, Value};
use support::*;

/// A config with two problems: one unknown key and one unusable layout.
const TWO_PROBLEMS: &str = r#"default = "good"
colour = "purple"

[[layouts.bad.tabs]]
name = "t"
[[layouts.bad.tabs.panes]]
split = "right"

[[layouts.good.tabs]]
name = "agent"
[[layouts.good.tabs.panes]]
agent = "claude"
[[layouts.good.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"
"#;

fn config(dir: &TempDir, toml: &str) -> std::path::PathBuf {
    config_root_with(dir, toml)
}

fn popup_env(stub: &Stub) -> Vec<Value> {
    stub.params_for("plugin.pane.open")
        .into_iter()
        .filter(|p| p.get("entrypoint").and_then(Value::as_str) == Some("issues"))
        .collect()
}

fn issue_lines(stub: &Stub) -> Vec<String> {
    let opened = popup_env(stub);
    let path = opened[0]["env"]["AGENT_LAYOUT_ISSUES_FILE"]
        .as_str()
        .expect("the issues file path must reach the popup");
    std::fs::read_to_string(path)
        .expect("the issues file must exist when the popup is opened")
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn a_diagnostic_opens_the_issues_popup() {
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);

    let opened = popup_env(&stub);
    assert_eq!(opened.len(), 1, "exactly one popup: {:?}", opened);
    assert_eq!(opened[0]["placement"], json!("popup"));
    assert_eq!(
        opened[0]["plugin_id"],
        json!("mikebronner.agentic-panes-layout")
    );
}

#[test]
fn the_popup_carries_every_problem_rather_than_the_first() {
    // The failure this replaces. Only one toast can be live at a time, so the old
    // per-diagnostic loop could never show more than its first item.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));

    let lines = issue_lines(&stub);
    assert_eq!(lines.len(), 2, "{:?}", lines);
    assert!(
        lines.iter().any(|l| l.contains("unknown key")),
        "{:?}",
        lines
    );
    assert!(
        lines.iter().any(|l| l.contains("cannot carry a split")),
        "{:?}",
        lines
    );
}

#[test]
fn the_popup_opens_on_the_event_path_too() {
    // Mike chose both paths. A broken config must never be invisible, and that outranks
    // the cost of a popup nobody asked for.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    let payload =
        json!({"data": {"workspace": {"workspace_id": "w9", "focused": true}}}).to_string();
    let run = run_with_env(
        &stub,
        &["--from-event"],
        Some(root.as_path()),
        &[("HERDR_PLUGIN_EVENT_JSON", &payload)],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(popup_env(&stub).len(), 1, "{:?}", stub.methods());
}

#[test]
fn a_clean_config_opens_no_popup_at_all() {
    let dir = TempDir::new();
    let root = config(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(popup_env(&stub).is_empty(), "{:?}", stub.methods());
}

#[test]
fn a_clean_run_is_completely_inert_about_issues() {
    // The hot path. The popup fires on every worktree creation, so a clean config runs
    // this machinery every time and must cost nothing at all: no pane, no toast about
    // problems, no file left behind, nothing on stderr.
    //
    // The run gets its OWN temp directory. Counting entries in the shared one made this
    // test flaky, because sibling tests create and remove issue directories there at the
    // same time; a private TMPDIR means anything found in it came from this run.
    let dir = TempDir::new();
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let root = config(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run_with_env(
        &stub,
        &[],
        Some(root.as_path()),
        &[("TMPDIR", tmp.to_str().unwrap())],
    );

    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        popup_env(&stub).is_empty(),
        "a clean config must open no popup: {:?}",
        stub.methods()
    );
    assert!(
        !stub.params_for("notification.show").iter().any(|p| p
            .get("body")
            .and_then(Value::as_str)
            .is_some_and(|b| b.contains("problem"))),
        "a clean config must raise no problem toast"
    );
    assert!(
        !run.says("problem"),
        "a clean config must say nothing about problems: {}",
        run.stderr
    );
    let left: Vec<String> = std::fs::read_dir(&tmp)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        left.is_empty(),
        "a clean run must leave nothing in its temp directory: {:?}",
        left
    );
}

#[test]
fn a_run_with_problems_does_write_an_issues_file() {
    // The control for the test above. Without it, "a clean run leaves nothing" would
    // pass just as well on a build that never wrote an issues file at all.
    let dir = TempDir::new();
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    run_with_env(
        &stub,
        &[],
        Some(root.as_path()),
        &[("TMPDIR", tmp.to_str().unwrap())],
    );

    let left: Vec<String> = std::fs::read_dir(&tmp)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(left.len(), 1, "{:?}", left);
    assert!(left[0].starts_with("agent-layout-issues-"), "{:?}", left);
}

#[test]
fn no_config_file_at_all_opens_no_popup() {
    // A fresh install is not broken. Interrupting it would train people to dismiss the
    // popup without reading it.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(popup_env(&stub).is_empty(), "{:?}", stub.methods());
}

#[test]
fn the_popup_is_opened_after_the_layout_is_built() {
    // A config problem is a cosmetic warning. Gating pane creation on a dialog would
    // turn it into a stall, so every pane and the agent come first.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));

    let methods = stub.methods();
    let popup_at = methods
        .iter()
        .position(|m| m == "plugin.pane.open")
        .expect("the popup must be opened");
    for step in ["layout.apply", "agent.start"] {
        let last = methods
            .iter()
            .rposition(|m| m == step)
            .unwrap_or_else(|| panic!("{} never happened: {:?}", step, methods));
        assert!(
            last < popup_at,
            "{} happened after the popup was opened: {:?}",
            step,
            methods
        );
    }
}

#[test]
fn the_layout_is_still_built_when_the_popup_cannot_be_opened() {
    // The popup is the messenger, not the work.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default().failing("plugin.pane.open", "pane_limit_reached"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(stub.params_for("agent.start").len(), 1);
}

// ---------------------------------------------------------------------------
// The toast, which used to be a loop.
// ---------------------------------------------------------------------------

#[test]
fn many_problems_produce_at_most_one_toast_about_them() {
    // Measured on 0.9.0: a second toast answers Busy, and there is a rate limit. The old
    // loop sent one per diagnostic, so at best the first appeared. Collecting every
    // problem, which this release also does, made that strictly worse.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));

    let bodies: Vec<String> = stub
        .params_for("notification.show")
        .iter()
        .filter_map(|p| p.get("body")?.as_str().map(|s| s.to_string()))
        .collect();

    // No toast repeats a diagnostic. That is what the old loop did, and counting only
    // the toasts that mention "problem" would miss it, because a diagnostic does not.
    let lines = issue_lines(&stub);
    for issue in &lines {
        assert!(
            !bodies.iter().any(|b| b == issue),
            "a diagnostic was sent as its own toast: {:?}",
            issue
        );
    }

    let summaries: Vec<&String> = bodies.iter().filter(|b| b.contains("problem")).collect();
    assert_eq!(summaries.len(), 1, "one summary toast: {:?}", bodies);
    assert!(summaries[0].contains("2 problems"), "{:?}", summaries);
    assert!(summaries[0].contains("popup"), "{:?}", summaries);
}

#[test]
fn the_number_of_toasts_does_not_grow_with_the_number_of_problems() {
    // The property, stated directly. One problem and three must cost the same number of
    // toasts, or the Busy and RateLimited paths eat the difference silently.
    let count = |toml: &str| -> usize {
        let dir = TempDir::new();
        let root = config(&dir, toml);
        let stub = Stub::start(Script::default());
        run(&stub, &[], Some(root.as_path()));
        stub.params_for("notification.show").len()
    };

    let one = count(
        "default = \"good\"\ncolour = \"purple\"\n\
         [[layouts.good.tabs]]\nname = \"agent\"\n\
         [[layouts.good.tabs.panes]]\nagent = \"claude\"\n",
    );
    let three = count(
        "default = \"good\"\ncolour = \"purple\"\nsize = 3\nshape = \"round\"\n\
         [[layouts.good.tabs]]\nname = \"agent\"\n\
         [[layouts.good.tabs.panes]]\nagent = \"claude\"\n",
    );
    assert_eq!(
        one, three,
        "three problems produced {} toasts where one produced {}",
        three, one
    );
}

#[test]
fn every_problem_still_reaches_stderr() {
    // The only record that survives when nothing renders, and what reaches
    // `herdr plugin log list`.
    let dir = TempDir::new();
    let root = config(&dir, TWO_PROBLEMS);
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(run.says("unknown key"), "{}", run.stderr);
    assert!(run.says("cannot carry a split"), "{}", run.stderr);
}

// ---------------------------------------------------------------------------
// The popup process itself.
// ---------------------------------------------------------------------------

fn render(issues: &str, extra: &[(&str, &str)]) -> (String, i32) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = TempDir::new();
    let path = dir.join("issues");
    std::fs::write(&path, issues).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .arg("--issues")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("AGENT_LAYOUT_ISSUES_FILE", &path)
        .env("AGENT_LAYOUT_ISSUES_HEADING", "/some/agent-layout.toml")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (k, v) in extra {
        command.env(k, v);
    }
    let mut child = command.spawn().expect("cannot run the popup");
    // Any key dismisses it. With no tty the popup reads a line instead, so this stands
    // in for the keypress.
    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn the_popup_lists_every_issue_and_counts_them() {
    let (shown, code) = render("first problem\nsecond problem\n", &[]);
    assert_eq!(code, 0, "{}", shown);
    assert!(shown.contains("2 problems"), "{}", shown);
    assert!(shown.contains("first problem"), "{}", shown);
    assert!(shown.contains("second problem"), "{}", shown);
}

#[test]
fn one_issue_is_not_described_as_plural() {
    let (shown, _) = render("only one\n", &[]);
    assert!(shown.contains("1 problem in"), "{}", shown);
    assert!(!shown.contains("1 problems"), "{}", shown);
}

#[test]
fn the_popup_names_the_file_the_problems_came_from() {
    // Without it the reader has to guess which config is at fault, and the config root
    // is derived rather than fixed.
    let (shown, _) = render("a problem\n", &[]);
    assert!(shown.contains("/some/agent-layout.toml"), "{}", shown);
}

#[test]
fn the_popup_says_the_layout_was_still_applied() {
    // Otherwise a popup full of red looks like the run failed, when the workspace is
    // sitting there working.
    let (shown, _) = render("a problem\n", &[]);
    assert!(shown.contains("still applied"), "{}", shown);
    assert!(shown.contains("--check"), "{}", shown);
}

#[test]
fn the_popup_says_how_to_dismiss_it() {
    let (shown, _) = render("a problem\n", &[]);
    assert!(shown.contains("any key"), "{}", shown);
}

#[test]
fn the_popup_is_styled_as_a_problem_rather_than_merely_coloured() {
    // Herdr hardcodes every API notification to one kind, so a toast cannot look like an
    // error. Owning the terminal is the whole reason this is a pane, so the severity has
    // to actually show rather than the output merely containing some escape somewhere.
    let (shown, _) = render("a problem\n", &[]);
    for (code, what) in [
        ("\x1b[31m", "the heading must be red"),
        ("\x1b[33m", "each issue must be marked"),
        ("\x1b[1m", "the heading must be bold"),
        ("\x1b[2m", "the footer must be dim"),
        ("\x1b[0m", "styling must be reset"),
    ] {
        assert!(shown.contains(code), "{}: {:?}", what, shown);
    }
}

#[test]
fn the_popup_removes_the_file_it_was_given() {
    // The side that wrote it does not wait, so nothing else can clean it up.
    let dir = TempDir::new();
    let path = dir.join("issues");
    std::fs::write(&path, "a problem\n").unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .arg("--issues")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("AGENT_LAYOUT_ISSUES_FILE", &path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    let _ = child.wait();
    assert!(!path.exists(), "the popup must clean up after itself");
}

#[test]
fn the_popup_refuses_without_somewhere_to_read_from() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .arg("--issues")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .output()
        .expect("cannot run the popup");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("AGENT_LAYOUT_ISSUES_FILE"));
}

#[test]
fn the_popup_never_touches_the_socket() {
    let stub = Stub::start(Script::default());
    let (_shown, code) = render(
        "a problem\n",
        &[("HERDR_SOCKET_PATH", stub.socket().to_str().unwrap())],
    );
    assert_eq!(code, 0);
    assert!(stub.requests().is_empty(), "{:?}", stub.methods());
}

#[test]
fn the_issues_flag_is_refused_twice_like_any_other() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--issues", "--issues"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("given more than once"), "{}", run.stderr);
}
