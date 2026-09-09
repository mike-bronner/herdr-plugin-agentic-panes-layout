//! The requests that make up a layout, and the settings each one reads.

mod support;

use serde_json::json;
use support::*;

#[test]
fn the_built_in_default_reproduces_the_v0_2_0_arrangement() {
    // The geometry of v0.2.0 exactly: tab renamed `agent`, the pane split right
    // at 0.5, the new pane split down at 0.6, lazygit in the middle pane, claude
    // in the original one. The ONE deliberate difference is that no pane.rename
    // is sent, because labels became opt-in — that is the change this release is
    // for, and it is asserted separately below rather than left implied.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);

    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t1", "label": "agent"})]
    );
    assert_eq!(
        stub.params_for("pane.split"),
        vec![
            json!({"target_pane_id": "p1", "direction": "right", "ratio": 0.5,
                   "cwd": "/tmp/proj", "focus": false}),
            json!({"target_pane_id": "p2", "direction": "down", "ratio": 0.6,
                   "cwd": "/tmp/proj", "focus": false}),
        ]
    );
    assert_eq!(
        stub.params_for("pane.send_input"),
        vec![json!({"pane_id": "p2", "text": "lazygit", "keys": ["enter"]})]
    );
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "p1"})]
    );
}

#[test]
fn the_built_in_default_labels_nothing() {
    // The originating request. v0.2.0 always sent three pane.rename calls; the
    // default must now send none, or the change did not happen.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("pane.rename"),
        Vec::<serde_json::Value>::new()
    );
}

#[test]
fn the_second_split_targets_the_pane_the_first_one_made() {
    // The crux of the two-split layout. The middle pane's id is not knowable in
    // advance: it exists only in pane.split's answer. Splitting p1 twice would
    // stack three panes down the agent's side instead of dividing the column
    // beside it, and every ratio would then apply to the wrong pane.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    let splits = stub.params_for("pane.split");
    assert_eq!(splits.len(), 2);
    assert_eq!(splits[0]["target_pane_id"], json!("p1"));
    assert_eq!(splits[1]["target_pane_id"], json!("p2"));
}

#[test]
fn the_command_runs_in_its_own_pane_and_no_other() {
    // p2 is the pane the first split made. Running lazygit in p1 would replace
    // the agent, and in p3 would fill the bare shell.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    let runs = stub.params_for("pane.send_input");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["pane_id"], json!("p2"));
}

#[test]
fn a_command_is_sent_as_text_plus_enter() {
    // pane.run is not an API method at either protocol 20 or 22; the CLI's
    // `pane run` is sugar for this pair. Measured 2026-09-09 against a live
    // isolated server: text plus keys ["enter"] echoes and executes the line.
    // Sending the text without the Enter would leave the command sitting
    // unexecuted at the prompt, which looks identical in a screenshot.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    let runs = stub.params_for("pane.send_input");
    assert_eq!(runs[0]["text"], json!("lazygit"));
    assert_eq!(runs[0]["keys"], json!(["enter"]));
}

#[test]
fn the_agent_name_is_derived_from_the_workspace_label() {
    let stub = Stub::start(Script {
        workspaces: json!({"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1",
             "label": "9 Bible/Models", "focused": true}]}),
        ..Script::default()
    });
    run(&stub, &[], None);
    assert_eq!(
        stub.params_for("agent.start")[0]["name"],
        json!("a9-bible-models")
    );
}

#[test]
fn a_failed_tab_rename_is_fatal_and_stops_before_any_split() {
    // The tab rename happens before any pane of ours exists, so dying leaves a
    // clean single pane the next run can lay out.
    let stub = Stub::start(Script::default().failing("tab.rename", "tab_not_found"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("could not rename the tab"), "{}", run.stderr);
    assert!(stub.params_for("pane.split").is_empty());
}

#[test]
fn a_failed_split_is_fatal_and_names_the_direction_and_ratio_it_used() {
    let stub = Stub::start(Script::default().fail_split(1));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("(right, ratio 0.5)"), "{}", run.stderr);
}

#[test]
fn a_failed_first_split_never_reaches_the_second() {
    // The second split needs the first one's answer. Carrying on with an empty
    // pane id would aim pane.split and pane.send_input at nothing.
    let stub = Stub::start(Script::default().fail_split(1));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert_eq!(stub.params_for("pane.split").len(), 1);
    assert!(stub.params_for("pane.send_input").is_empty());
}

#[test]
fn a_failed_second_split_names_its_own_direction_and_ratio() {
    // Distinct from the first split's message, which names the other pair.
    // Reporting the wrong ratio sends the reader to the wrong line of their file.
    let stub = Stub::start(Script::default().fail_split(2));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("(down, ratio 0.6)"), "{}", run.stderr);
    assert!(!run.says("(right, ratio 0.5)"), "{}", run.stderr);
}

#[test]
fn a_failed_second_split_stops_before_running_the_command_or_the_agent() {
    let stub = Stub::start(Script::default().fail_split(2));
    run(&stub, &[], None);
    assert!(stub.params_for("pane.send_input").is_empty());
    assert!(stub.params_for("pane.rename").is_empty());
    assert!(stub.params_for("agent.start").is_empty());
}

#[test]
fn no_focused_workspace_is_fatal() {
    let stub = Stub::start(Script {
        workspaces: json!({"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1", "label": "x",
             "focused": false}]}),
        ..Script::default()
    });
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("cannot read a focused workspace"),
        "{}",
        run.stderr
    );
}

#[test]
fn a_workspace_whose_panes_report_no_working_directory_is_fatal() {
    let stub = Stub::start(Script {
        panes: json!({"panes": [{"pane_id": "p1", "tab_id": "t1"}]}),
        ..Script::default()
    });
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("no pane reports a working directory"),
        "{}",
        run.stderr
    );
    assert!(stub.changing().is_empty());
}

#[test]
fn an_unreachable_socket_is_fatal_and_changes_nothing() {
    // Fail closed. The socket is the only way this program can act at all, so a
    // missing one must be an error rather than a silent success.
    let stub = Stub::start(Script::default());
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    let output = command
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env(
            "HERDR_SOCKET_PATH",
            "/private/tmp/agent-layout-no-such.sock",
        )
        .output()
        .expect("cannot run the binary");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot reach"));
    assert!(stub.changing().is_empty());
}

#[test]
fn a_missing_socket_variable_is_fatal() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    let output = command
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .output()
        .expect("cannot run the binary");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HERDR_SOCKET_PATH is not set"));
}

// ---------------------------------------------------------------------------
// Once the panes exist, a failure is reported and the run still succeeds.
//
// The split of responsibility mirrors the fatal cases above. A failed tab rename
// happens before any pane is created, so dying leaves a clean tab. A failed
// command or label happens after the panes exist, where the tab-name guard makes
// every later run skip that tab — so exiting non-zero would record a layout that
// was in fact built as a failure, and nothing could ever finish it.
// ---------------------------------------------------------------------------

#[test]
fn a_failed_command_warns_but_the_layout_stands() {
    let stub = Stub::start(Script::default().failing("pane.send_input", "pane_not_found"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("\"lazygit\" would not run"), "{}", run.stderr);
}

#[test]
fn a_failed_command_does_not_stop_the_agent() {
    let stub = Stub::start(Script::default().failing("pane.send_input", "pane_not_found"));
    run(&stub, &[], None);
    assert_eq!(stub.params_for("agent.start").len(), 1);
}

#[test]
fn a_failed_label_warns_but_the_layout_stands() {
    let stub = Stub::start(Script::default().failing("pane.rename", "pane_not_found"));
    let root = labelled_config();
    let run = run(&stub, &[], Some(root.1.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("could not label pane"), "{}", run.stderr);
}

#[test]
fn a_failed_label_costs_neither_the_other_labels_nor_the_agent() {
    // One unlabelled pane must not cost the other two their labels.
    let stub = Stub::start(Script::default().failing("pane.rename", "pane_not_found"));
    let root = labelled_config();
    run(&stub, &[], Some(root.1.as_path()));
    assert_eq!(stub.params_for("pane.rename").len(), 3);
    assert_eq!(stub.params_for("agent.start").len(), 1);
}

/// The v0.2.0 layout with its three labels restored, for the label tests.
fn labelled_config() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "three-pane"

[[layouts.three-pane.tabs]]
name = "agent"

[[layouts.three-pane.tabs.panes]]
agent = "claude"
label = "agent"

[[layouts.three-pane.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"
label = "lazygit"

[[layouts.three-pane.tabs.panes]]
split = "down"
ratio = 0.6
label = "shell"
"#,
    );
    (dir, root)
}

#[test]
fn labels_are_written_when_the_config_asks_for_them() {
    let stub = Stub::start(Script::default());
    let root = labelled_config();
    let run = run(&stub, &[], Some(root.1.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("pane.rename"),
        vec![
            json!({"pane_id": "p1", "label": "agent"}),
            json!({"pane_id": "p2", "label": "lazygit"}),
            json!({"pane_id": "p3", "label": "shell"}),
        ]
    );
}

#[test]
fn a_label_containing_a_space_reaches_herdr_whole() {
    // The old shell stub could lose this to word splitting, and needed a second
    // log to prove it had not. A JSON request cannot lose it, but the value still
    // has to arrive unmangled, so it is pinned here rather than assumed.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "one"
[[layouts.one.tabs]]
name = "agent"
[[layouts.one.tabs.panes]]
label = "my tool pane"
"#,
    );
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));
    assert_eq!(
        stub.params_for("pane.rename"),
        vec![json!({"pane_id": "p1", "label": "my tool pane"})]
    );
}

#[test]
fn an_empty_label_renames_and_an_absent_label_does_not() {
    // The distinction the whole change rests on. `label = ""` is a rename to
    // nothing; omitting the key is no request at all. Treating them alike would
    // reintroduce the imposed label this release removes.
    let dir = TempDir::new();
    let empty = config_root_with(
        &dir,
        "default = \"one\"\n\
         [[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\nlabel = \"\"\n",
    );
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(empty.as_path()));
    assert_eq!(
        stub.params_for("pane.rename"),
        vec![json!({"pane_id": "p1", "label": ""})]
    );

    let other = TempDir::new();
    let absent = config_root_with(
        &other,
        "default = \"one\"\n\
         [[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
    );
    let stub2 = Stub::start(Script::default());
    run(&stub2, &[], Some(absent.as_path()));
    assert!(stub2.params_for("pane.rename").is_empty());
}
