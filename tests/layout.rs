//! The requests that make up a layout, and the settings each one reads.

mod support;

use serde_json::{json, Value};
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

    // ONE call builds the tab, and the tree carries the same geometry the sequential
    // engine produced: split right at 0.5, then the remainder split down at 0.6.
    let applied = stub.params_for("layout.apply");
    assert_eq!(applied.len(), 1, "one call per tab: {:?}", stub.methods());
    assert_eq!(applied[0]["tab_label"], json!("agent"));
    assert_eq!(
        applied[0]["root"],
        json!({
            "type": "split", "direction": "right", "ratio": 0.5,
            "first": {"type": "pane", "cwd": "/tmp/proj"},
            "second": {
                "type": "split", "direction": "down", "ratio": 0.6,
                "first": {"type": "pane", "cwd": "/tmp/proj"},
                "second": {"type": "pane", "cwd": "/tmp/proj"}
            }
        })
    );
    // Commands and agents still go through their own calls, on ids from the response.
    assert_eq!(
        stub.params_for("pane.send_input"),
        vec![json!({"pane_id": "t2p2", "text": "lazygit", "keys": ["enter"]})]
    );
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "t2p1"})]
    );
}

#[test]
fn no_leaf_carries_a_command_or_a_label() {
    // Both are deliberate and both were measured. The leaf's `command` execs raw argv
    // against the SERVER's PATH, which under launchd would not resolve a bare `lazygit`,
    // and a whole command line in one element is looked up as one executable name. The
    // leaf's `label` cannot express an empty string: it comes back as null, which is what
    // an absent label also looks like, and this plugin's contract turns on telling those
    // apart.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\ncommand = \"lazygit\"\nlabel = \"git\"\n",
    );
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));
    let tree = stub.params_for("layout.apply")[0]["root"].clone();
    assert!(tree.get("command").is_none(), "{:?}", tree);
    assert!(tree.get("label").is_none(), "{:?}", tree);
    assert_eq!(
        tree["cwd"],
        json!("/tmp/proj"),
        "every leaf sets its own cwd"
    );
}

#[test]
fn every_leaf_sets_its_own_cwd_rather_than_inheriting() {
    // Measured on 0.9.0: a leaf omitting cwd inherits its SIBLING's, not the workspace's.
    // Setting it everywhere removes that surprise.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    fn every_leaf(node: &Value, seen: &mut usize) {
        match node["type"].as_str() {
            Some("pane") => {
                assert_eq!(node["cwd"], json!("/tmp/proj"), "{:?}", node);
                *seen += 1;
            }
            Some("split") => {
                every_leaf(&node["first"], seen);
                every_leaf(&node["second"], seen);
            }
            _ => panic!("unexpected node {:?}", node),
        }
    }
    let mut seen = 0;
    every_leaf(&stub.params_for("layout.apply")[0]["root"], &mut seen);
    assert_eq!(seen, 3);
}

#[test]
fn no_tab_the_layout_builds_takes_the_focus() {
    // Measured on 0.9.0: `focus: true` on an added tab really does move the user to it,
    // and `focus: false` really does leave them where they were. So a multi-tab layout
    // sending true would end with the user staring at the last tab it happened to
    // build, which is not the one they were working in.
    //
    // Replacing a workspace's only tab is the exception the measurement also settled:
    // the replacement comes back focused whatever is sent, because nothing else is left
    // to focus. That is Herdr's choice, not this plugin asking for it.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"pair\"\n\
         [[layouts.pair.tabs]]\nname = \"agent\"\n\
         [[layouts.pair.tabs.panes]]\nagent = \"claude\"\n\
         [[layouts.pair.tabs]]\nname = \"notes\"\n\
         [[layouts.pair.tabs.panes]]\ncommand = \"true\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    let applied = stub.params_for("layout.apply");
    assert_eq!(applied.len(), 2);
    for call in applied {
        assert_eq!(call["focus"], json!(false), "{:?}", call);
    }
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
fn the_command_runs_in_its_own_pane_and_no_other() {
    // The second leaf's pane, taken from the apply response. Running lazygit in the first
    // would replace the agent, and in the third would fill the bare shell.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    let runs = stub.params_for("pane.send_input");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["pane_id"], json!("t2p2"));
}

#[test]
fn a_failed_apply_is_fatal_and_names_the_tab() {
    // The call is atomic, so a rejected tree leaves no tab behind. There is nothing
    // half-built to report and nothing to clean up.
    let stub = Stub::start(Script::default().failing("layout.apply", "layout_apply_failed"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("could not build tab \"agent\""), "{}", run.stderr);
    assert!(stub.params_for("pane.send_input").is_empty());
    assert!(stub.params_for("agent.start").is_empty());
    assert!(stub.params_for("pane.rename").is_empty());
}

#[test]
fn a_response_with_the_wrong_pane_count_is_fatal_rather_than_misapplied() {
    // Commands, labels and agents are matched to panes by position in the tree. A
    // response carrying a different number of panes would silently put them in the wrong
    // places, which is worse than stopping.
    let stub = Stub::start(Script::default().apply_returns_panes(2));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("wanted 3 panes and Herdr made 2"),
        "{}",
        run.stderr
    );
    assert!(stub.params_for("agent.start").is_empty());
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
            json!({"pane_id": "t2p1", "label": "agent"}),
            json!({"pane_id": "t2p2", "label": "lazygit"}),
            json!({"pane_id": "t2p3", "label": "shell"}),
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
        vec![json!({"pane_id": "t2p1", "label": "my tool pane"})]
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
        vec![json!({"pane_id": "t2p1", "label": ""})]
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
