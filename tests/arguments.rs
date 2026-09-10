//! The argument list, the workspace it targets, and the event gate.
//!
//! Every parse case asserts that nothing changed, which is the point: the parse
//! happens first, so a usage error must cost the caller nothing. Asserting the
//! message alone would pass just as well on a program that died halfway through a
//! layout it had already half-applied.

mod support;

use serde_json::json;
use support::*;

fn payload(focused: bool) -> String {
    json!({"event": "worktree_created",
           "data": {"type": "worktree_created",
                    "workspace": {"workspace_id": "w9", "focused": focused}}})
    .to_string()
}

#[test]
fn an_unknown_argument_is_fatal_and_reads_nothing() {
    // Fail closed. A caller that misspells a flag must get an error rather than a
    // layout aimed somewhere it did not ask for. Stronger than the guard alone:
    // not one request is sent, so a usage error cannot leave a workspace
    // half-inspected either.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--workspaces", "w7"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("unknown argument: --workspaces"), "{}", run.stderr);
    assert!(
        stub.methods_besides_the_toast().is_empty(),
        "{:?}",
        stub.methods()
    );
}

#[test]
fn a_removed_flag_reads_as_unknown_rather_than_being_ignored() {
    // --no-agent was considered and deliberately not built. It must read as an
    // unknown flag, or a caller written against the discarded design would
    // silently get an agent it asked not to have.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--no-agent"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("unknown argument: --no-agent"), "{}", run.stderr);
}

#[test]
fn the_removed_recipe_setting_has_no_flag_either() {
    // AGENT_LAYOUT_RECIPE is gone, and the declarative config is the thing it was
    // a workaround for. A flag spelled after it must not quietly appear.
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--recipe", "/tmp/mine"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("unknown argument: --recipe"), "{}", run.stderr);
}

#[test]
fn a_flag_without_a_value_is_fatal() {
    for flag in ["--workspace", "--agent-name"] {
        let stub = Stub::start(Script::default());
        let run = run(&stub, &[flag], None);
        assert_eq!(run.status, 1, "{}", flag);
        assert!(run.says("needs a"), "{}", run.stderr);
        assert!(stub.changing().is_empty());
    }
}

#[test]
fn an_empty_flag_value_is_fatal_rather_than_the_default_it_replaces() {
    // The fail-open this closes. `--workspace ""` reads as "no id given" to a bare
    // emptiness test, which would silently lay out the FOCUSED workspace: the one
    // workspace a caller passing --workspace certainly did not mean.
    for flag in ["--workspace", "--agent-name"] {
        let stub = Stub::start(Script::default());
        let run = run(&stub, &[flag, ""], None);
        assert_eq!(run.status, 1, "{}", flag);
        assert!(stub.changing().is_empty());
    }
}

#[test]
fn a_repeated_flag_is_fatal_even_with_the_same_value() {
    // Last-wins would hide the caller's own confusion at the one moment it could
    // still be fixed. Comparing values instead would make the rule depend on a
    // coincidence, so an identical repeat is refused too.
    for args in [
        vec!["--workspace", "w7", "--workspace", "w9"],
        vec!["--workspace", "w7", "--workspace", "w7"],
        vec!["--agent-name", "one", "--agent-name", "two"],
        vec!["--from-event", "--from-event"],
    ] {
        let stub = Stub::start(Script::default());
        let run = run(&stub, &args, None);
        assert_eq!(run.status, 1, "{:?}", args);
        assert!(run.says("given more than once"), "{}", run.stderr);
        assert!(stub.methods_besides_the_toast().is_empty());
    }
}

#[test]
fn both_flags_together_are_not_a_repeat() {
    // The repeat check is per flag. One shared guard would reject every call that
    // passes both.
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        panes: one_bare_pane_in_t7(),
        ..Script::default()
    });
    let run = run(
        &stub,
        &["--workspace", "w7", "--agent-name", "reserved-2"],
        None,
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "reserved-2", "kind": "claude", "pane_id": "p1"})]
    );
}

#[test]
fn no_arguments_picks_the_focused_workspace() {
    // The bare invocation, on the very fixture that could hide a regression in it.
    // The event hook and the keybinding both pass no target, so the focused
    // workspace must still win when another one exists to be chosen by mistake.
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        ..Script::default()
    });
    run(&stub, &[], None);
    assert_eq!(
        stub.params_for("pane.list"),
        vec![json!({"workspace_id": "w9"})]
    );
}

#[test]
fn the_named_workspace_is_laid_out_though_it_is_not_focused() {
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        panes: one_bare_pane_in_t7(),
        ..Script::default()
    });
    run(&stub, &["--workspace", "w7"], None);
    assert_eq!(
        stub.params_for("pane.list"),
        vec![json!({"workspace_id": "w7"})]
    );
    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t7", "label": "agent"})]
    );
    assert_eq!(stub.params_for("pane.split")[0]["cwd"], json!("/tmp/other"));
}

#[test]
fn the_agent_name_comes_from_the_named_workspaces_label() {
    // The derivation reads the label, so targeting the wrong workspace would start
    // the agent under the wrong name even if every id were right.
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        panes: one_bare_pane_in_t7(),
        ..Script::default()
    });
    run(&stub, &["--workspace", "w7"], None);
    assert_eq!(
        stub.params_for("agent.start")[0]["name"],
        json!("other-proj")
    );
}

#[test]
fn an_unknown_workspace_id_is_fatal_and_lays_nothing_out() {
    // Falling back to the focused workspace on a stale id would lay out whatever
    // the user happens to be looking at, which is the exact accident the flag
    // exists to prevent.
    let stub = Stub::start(Script {
        workspaces: two_workspaces(),
        ..Script::default()
    });
    let run = run(&stub, &["--workspace", "w404"], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("no workspace \"w404\" in the Herdr server"),
        "{}",
        run.stderr
    );
    assert!(
        !run.says("cannot read a focused workspace"),
        "{}",
        run.stderr
    );
    assert!(stub.changing().is_empty());
}

#[test]
fn no_focused_workspace_does_not_stop_a_targeted_run() {
    // A workspace can be created without ever being focused, so this program can
    // be reached with nothing focused at all.
    let stub = Stub::start(Script {
        workspaces: json!({"workspaces": [
            {"workspace_id": "w7", "active_tab_id": "t7", "label": "other proj",
             "focused": false}]}),
        panes: one_bare_pane_in_t7(),
        ..Script::default()
    });
    let run = run(&stub, &["--workspace", "w7"], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("agent.start")[0]["name"],
        json!("other-proj")
    );
}

// ---------------------------------------------------------------------------
// The event gate.
//
// v0.2.0 answered "is the new workspace the focused one?" in bin/on-event, with a
// Python one-liner. The gate now lives behind --from-event, which the hook passes
// and nothing else does. Its reason is unchanged: `herdr worktree create
// --no-focus` emits worktree.created with "focused": false, and laying out an
// unfocused workspace would start an agent in whatever the user is looking at.
//
// The flag is explicit rather than inferred from the presence of the environment
// variable, so that an ambient HERDR_PLUGIN_EVENT_JSON cannot silently change what
// a keybinding press does.
// ---------------------------------------------------------------------------

#[test]
fn a_focused_event_workspace_is_laid_out() {
    let stub = Stub::start(Script::default());
    let run = run_with_env(
        &stub,
        &["--from-event"],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &payload(true))],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(stub.params_for("tab.rename").len(), 1);
}

#[test]
fn an_unfocused_event_workspace_is_refused_silently() {
    let stub = Stub::start(Script::default());
    let run = run_with_env(
        &stub,
        &["--from-event"],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &payload(false))],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.requests().is_empty(), "{:?}", stub.requests());
}

#[test]
fn a_payload_that_cannot_be_read_is_refused() {
    // Fail closed on every shape that is not a focused workspace. A gate that
    // fell open on malformed input would act on exactly the events nobody
    // designed for.
    for raw in [
        "not json",
        "",
        r#"{"event":"worktree_created","data":{}}"#,
        r#"{"data":{"workspace":{}}}"#,
        r#"{"data":{"workspace":{"focused":"yes"}}}"#,
    ] {
        let stub = Stub::start(Script::default());
        let run = run_with_env(
            &stub,
            &["--from-event"],
            None,
            &[("HERDR_PLUGIN_EVENT_JSON", raw)],
        );
        assert_eq!(run.status, 0, "{:?} -> {}", raw, run.stderr);
        assert!(
            stub.requests().is_empty(),
            "{:?} -> {:?}",
            raw,
            stub.requests()
        );
    }
}

#[test]
fn an_opened_workspace_is_laid_out_like_a_created_one() {
    // Resolved by Mike: opening a workspace should lay it out, because that is what the
    // plugin is for. worktree.opened used to be subscribed as log-only instrumentation.
    //
    // Safe as a second ACTING subscription because the two events are disjoint per
    // action, measured on 0.9.0: `worktree create` emits only worktree.created, with and
    // without --focus, and `worktree open` emits only worktree.opened. The race the old
    // design note feared, two runs splitting one pane, cannot arise from one action.
    let stub = Stub::start(Script::default());
    let opened = json!({"event": "worktree_opened",
                        "data": {"type": "worktree_opened", "already_open": false,
                                 "workspace": {"workspace_id": "w9", "focused": true}}})
    .to_string();
    let run = run_with_env(
        &stub,
        &["--from-event"],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &opened)],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("tab.rename").len(),
        1,
        "{:?}",
        stub.methods()
    );
    assert_eq!(stub.params_for("agent.start").len(), 1);
}

#[test]
fn an_unfocused_opened_workspace_is_still_refused() {
    // The gate has to hold on BOTH acting paths. A second subscription doubles the
    // chances of laying out whatever the user is actually looking at.
    let stub = Stub::start(Script::default());
    let opened = json!({"event": "worktree_opened",
                        "data": {"type": "worktree_opened", "already_open": false,
                                 "workspace": {"workspace_id": "w9", "focused": false}}})
    .to_string();
    let run = run_with_env(
        &stub,
        &["--from-event"],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &opened)],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.requests().is_empty(), "{:?}", stub.requests());
}

#[test]
fn reopening_an_already_open_workspace_changes_nothing() {
    // `already_open: true` is what reopening produces, measured on 0.9.0. The tab-name
    // guard is what makes it a no-op rather than a second layout, and the event path
    // keeps that guard. Confirmed here rather than assumed.
    let stub = Stub::start(Script {
        tabs: tabs_labelled(&["agent"]),
        ..Script::default()
    });
    let opened = json!({"event": "worktree_opened",
                        "data": {"type": "worktree_opened", "already_open": true,
                                 "workspace": {"workspace_id": "w9", "focused": true}}})
    .to_string();
    let run = run_with_env(
        &stub,
        &["--from-event"],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &opened)],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.changing().is_empty(),
        "reopening must be a no-op: {:?}",
        stub.changing()
    );
    assert!(
        run.says("left agent alone, already there"),
        "{}",
        run.stderr
    );
}

#[test]
fn a_missing_payload_variable_is_refused() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--from-event"], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.requests().is_empty());
}

#[test]
fn the_gate_applies_only_when_the_flag_is_given() {
    // Without --from-event an ambient payload must not gate anything, or a
    // keybinding press in a shell that happens to carry one would do nothing.
    let stub = Stub::start(Script::default());
    let run = run_with_env(
        &stub,
        &[],
        None,
        &[("HERDR_PLUGIN_EVENT_JSON", &payload(false))],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(stub.params_for("tab.rename").len(), 1);
}
