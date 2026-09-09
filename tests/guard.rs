//! The re-run guard, which is tab-name based, and its three first-tab cases.
//!
//! v0.2.0 guarded on the pane count of the active tab: more than one pane, or a
//! pane already hosting an agent, and the whole run was refused. That made a
//! partly applied layout permanently unfinishable. The guard is now per tab and
//! keyed on the tab's name, so a run that died halfway is resumed by the next one.
//!
//! The first tab is the exception, because it is not created — it takes over the
//! workspace's existing tab by being renamed. So it is skipped when a tab of its
//! name exists; it takes over the active tab when that tab holds a single pane
//! and no agent; and when the active tab is busy it is created as a new tab
//! instead of being wrecked.

mod support;

use serde_json::json;
use support::*;

fn two_tab_config() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "pair"

[[layouts.pair.tabs]]
name = "agent"
[[layouts.pair.tabs.panes]]
agent = "claude"

[[layouts.pair.tabs]]
name = "notes"
[[layouts.pair.tabs.panes]]
command = "less notes.md"
"#,
    );
    (dir, root)
}

#[test]
fn a_tab_whose_name_already_exists_is_skipped() {
    let stub = Stub::start(Script {
        tabs: tabs_labelled(&["agent"]),
        ..Script::default()
    });
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        run.says("left agent alone, already there"),
        "{}",
        run.stderr
    );
    assert!(stub.changing().is_empty(), "{:?}", stub.changing());
}

#[test]
fn the_first_tab_takes_over_a_free_active_tab_by_renaming_it() {
    // A free active tab is one pane and no agent. Taking it over is what makes a
    // freshly created worktree get its layout in the tab the user is looking at,
    // rather than in a second tab beside it.
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t1", "label": "agent"})]
    );
    assert!(stub.params_for("tab.create").is_empty());
}

#[test]
fn the_first_tab_is_created_when_the_active_tab_already_has_two_panes() {
    // v0.2.0 refused the whole run here. Creating a tab instead is what stops a
    // workspace the user has already arranged by hand from being wrecked, while
    // still giving them the layout they asked for.
    let stub = Stub::start(Script {
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p9", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
        ..Script::default()
    });
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.params_for("tab.rename").is_empty());
    assert_eq!(
        stub.params_for("tab.create"),
        vec![json!({"workspace_id": "w9", "cwd": "/tmp/proj",
                    "label": "agent", "focus": false})]
    );
}

#[test]
fn the_first_tab_is_created_when_the_active_tab_already_runs_an_agent() {
    let stub = Stub::start(Script {
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}),
        ..Script::default()
    });
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.params_for("tab.rename").is_empty());
    assert_eq!(stub.params_for("tab.create").len(), 1);
}

#[test]
fn a_created_first_tab_is_split_from_its_own_root_pane() {
    // The pane ids differ between the two paths: taking over reuses the active
    // tab's pane, creating one uses the pane tab.create answers with. Splitting
    // the old pane after creating a new tab would carve up the wrong tab.
    let stub = Stub::start(Script {
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p9", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
        ..Script::default()
    });
    run(&stub, &[], None);
    let splits = stub.params_for("pane.split");
    assert_eq!(splits[0]["target_pane_id"], json!("t2p1"));
}

#[test]
fn a_pane_in_another_tab_does_not_make_the_active_tab_look_busy() {
    let stub = Stub::start(Script {
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p9", "tab_id": "t2", "cwd": "/tmp/other"}]}),
        ..Script::default()
    });
    run(&stub, &[], None);
    assert_eq!(stub.params_for("tab.rename").len(), 1);
    assert!(stub.params_for("tab.create").is_empty());
}

#[test]
fn the_second_tab_is_created_rather_than_taking_anything_over() {
    // Only the FIRST tab may take over the active tab. A second take-over would
    // rename the tab the first one just built.
    let (_dir, root) = two_tab_config();
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(stub.params_for("tab.rename").len(), 1);
    assert_eq!(
        stub.params_for("tab.create"),
        vec![json!({"workspace_id": "w9", "cwd": "/tmp/proj",
                    "label": "notes", "focus": false})]
    );
}

#[test]
fn a_partly_applied_layout_is_resumed_rather_than_refused() {
    // The reason the guard moved off pane count. The first tab was built by an
    // earlier run that then died; this run must skip it and finish the second.
    let (_dir, root) = two_tab_config();
    let stub = Stub::start(Script {
        tabs: tabs_labelled(&["agent"]),
        ..Script::default()
    });
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.params_for("tab.rename").is_empty());
    assert_eq!(stub.params_for("tab.create").len(), 1);
    assert_eq!(stub.params_for("tab.create")[0]["label"], json!("notes"));
    assert!(run.says("laid out notes"), "{}", run.stderr);
    assert!(run.says("left agent alone"), "{}", run.stderr);
}

#[test]
fn a_fully_applied_layout_changes_nothing_at_all() {
    // Running twice must be a no-op, not a second layout.
    let (_dir, root) = two_tab_config();
    let stub = Stub::start(Script {
        tabs: tabs_labelled(&["agent", "notes"]),
        ..Script::default()
    });
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.changing().is_empty(), "{:?}", stub.changing());
}

#[test]
fn a_failed_tab_create_is_fatal_and_names_the_tab() {
    let (_dir, root) = two_tab_config();
    let stub = Stub::start(Script::default().failing("tab.create", "workspace_not_found"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 1);
    assert!(
        run.says("could not create the tab \"notes\""),
        "{}",
        run.stderr
    );
}

#[test]
fn a_failed_tab_list_is_fatal_and_changes_nothing() {
    // Fail closed. Without the tab list the guard cannot run, and proceeding
    // would rebuild a layout that is already there.
    let stub = Stub::start(Script::default().failing("tab.list", "workspace_not_found"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("cannot list the tabs"), "{}", run.stderr);
    assert!(stub.changing().is_empty(), "{:?}", stub.changing());
}

#[test]
fn the_tab_list_is_read_for_the_workspace_being_laid_out() {
    // The guard must guard the target. Reading the focused workspace's tabs while
    // laying out a named one would rebuild a target that is already laid out.
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        panes: one_bare_pane_in_t7(),
        ..Script::default()
    });
    run(&stub, &["--workspace", "w7"], None);
    assert_eq!(
        stub.params_for("tab.list"),
        vec![json!({"workspace_id": "w7"})]
    );
}
