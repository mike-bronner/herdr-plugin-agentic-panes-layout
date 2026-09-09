//! Explicit invocation rebuilds; the event path still skips.
//!
//! The tab-name guard is right when the trigger is `worktree.created` and wrong
//! when a human pressed a key: asking for a layout explicitly is a statement of
//! intent that a guard should not silently ignore.
//!
//! The two paths are told apart by inverting the sense rather than by adding a
//! flag to the keybinding. `bin/on-event` passes `--from-event` and asks for the
//! guard; a bare invocation, which is what the documented `[[keys.command]]`
//! binding sends, means rebuild. Mike's own config therefore needs no edit.
//!
//! A rebuild closes panes, and a pane running an agent holds work that cannot be
//! recovered, so that case asks first and every failure answers no.

mod support;

use serde_json::json;
use support::*;

/// A one-pane layout, so a rebuild's pane arithmetic is easy to read.
fn solo_config(dir: &TempDir) -> std::path::PathBuf {
    config_root_with(
        dir,
        "default = \"solo\"\n\
         [[layouts.solo.tabs]]\nname = \"agent\"\n\
         [[layouts.solo.tabs.panes]]\nagent = \"claude\"\nlabel = \"agent\"\n",
    )
}

/// The tab already exists, holding two panes, neither running an agent.
fn existing_plain_tab() -> Script {
    Script {
        tabs: tabs_labelled(&["agent"]),
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
        ..Script::default()
    }
}

/// The same tab, but p2 is running an agent.
fn existing_tab_with_an_agent() -> Script {
    Script {
        tabs: tabs_labelled(&["agent"]),
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}),
        ..Script::default()
    }
}

fn closed(stub: &Stub) -> Vec<String> {
    stub.params_for("pane.close")
        .iter()
        .filter_map(|p| p.get("pane_id")?.as_str().map(|s| s.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The two paths.
// ---------------------------------------------------------------------------

#[test]
fn the_event_path_still_skips_an_existing_tab() {
    // Unchanged, and the reason it exists is unchanged: a second event must not
    // rebuild a workspace nobody asked to rebuild.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    let payload =
        json!({"data": {"workspace": {"workspace_id": "w9", "focused": true}}}).to_string();
    let run = run_with_env(
        &stub,
        &["--from-event"],
        Some(root.as_path()),
        &[("HERDR_PLUGIN_EVENT_JSON", &payload)],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        run.says("left agent alone, already there"),
        "{}",
        run.stderr
    );
    assert!(stub.changing().is_empty(), "{:?}", stub.changing());
}

#[test]
fn a_bare_invocation_rebuilds_an_existing_tab() {
    // The keybinding path, which passes no arguments.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("rebuilt agent"), "{}", run.stderr);
    assert!(!run.says("left agent alone"), "{}", run.stderr);
}

#[test]
fn a_rebuild_needs_no_change_to_the_documented_keybinding() {
    // The whole reason the sense is inverted. The README documents a
    // [[keys.command]] entry whose command is the shim path with no arguments, and
    // Mike's live config matches it. If rebuilding needed a flag, his binding would
    // silently keep skipping.
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let binding = readme
        .lines()
        .find(|l| l.trim_start().starts_with("command = ") && l.contains("bin/agent-layout"))
        .expect("the README must document a [[keys.command]] binding");
    assert!(
        !binding.contains("--"),
        "the documented binding grew a flag: {}",
        binding
    );
}

#[test]
fn a_rebuild_still_creates_a_tab_the_layout_names_but_the_workspace_lacks() {
    // Rebuilding is about existing tabs. A missing one is still just built.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"pair\"\n\
         [[layouts.pair.tabs]]\nname = \"agent\"\n\
         [[layouts.pair.tabs.panes]]\nagent = \"claude\"\n\
         [[layouts.pair.tabs]]\nname = \"notes\"\n\
         [[layouts.pair.tabs.panes]]\ncommand = \"true\"\n",
    );
    let stub = Stub::start(existing_plain_tab());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("rebuilt agent"), "{}", run.stderr);
    assert!(run.says("laid out notes"), "{}", run.stderr);
    assert_eq!(
        stub.params_for("tab.create")[0]["label"],
        json!("notes"),
        "the missing tab was not created"
    );
}

// ---------------------------------------------------------------------------
// Rebuilding a tab with no agent in it.
// ---------------------------------------------------------------------------

#[test]
fn rebuilding_a_plain_tab_closes_the_extra_panes_and_asks_nothing() {
    // No agent means no unrecoverable work, so no popup. The first pane survives
    // because a tab must keep one, and the rest go.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.params_for("plugin.pane.open").is_empty(),
        "a plain rebuild must not ask anything"
    );
    assert_eq!(closed(&stub), vec!["p2"]);
}

#[test]
fn a_rebuilt_tab_is_relabelled_and_gets_its_agent() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    assert_eq!(
        stub.params_for("pane.rename"),
        vec![json!({"pane_id": "p1", "label": "agent"})]
    );
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "p1"})]
    );
}

#[test]
fn a_rebuild_does_not_rename_the_tab_it_is_rebuilding() {
    // The tab already carries the layout's name, which is how it was found. Renaming
    // it again is a wasted call, and taking over the active tab would be wrong.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    assert!(stub.params_for("tab.rename").is_empty());
    assert!(stub.params_for("tab.create").is_empty());
}

#[test]
fn a_rebuild_splits_from_the_surviving_pane() {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"two\"\n\
         [[layouts.two.tabs]]\nname = \"agent\"\n\
         [[layouts.two.tabs.panes]]\nagent = \"claude\"\n\
         [[layouts.two.tabs.panes]]\nsplit = \"right\"\nratio = 0.5\ncommand = \"lazygit\"\n",
    );
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    let splits = stub.params_for("pane.split");
    assert_eq!(splits.len(), 1);
    assert_eq!(splits[0]["target_pane_id"], json!("p1"));
}

#[test]
fn panes_of_another_tab_are_not_closed_by_a_rebuild() {
    // A rebuild is scoped to the tab being rebuilt. Closing a sibling tab's panes
    // would destroy work in a tab the layout never mentioned.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(Script {
        tabs: tabs_labelled(&["agent", "something else"]),
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p9", "tab_id": "t2", "cwd": "/tmp/proj"}]}),
        ..Script::default()
    });
    run(&stub, &[], Some(root.as_path()));
    assert_eq!(closed(&stub), vec!["p2"]);
}

// ---------------------------------------------------------------------------
// Rebuilding a tab that holds a running agent.
// ---------------------------------------------------------------------------

#[test]
fn a_rebuild_that_would_close_an_agent_asks_first() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("close"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    let asked = stub.params_for("plugin.pane.open");
    assert_eq!(asked.len(), 1, "exactly one question");
    assert_eq!(asked[0]["placement"], json!("popup"));
    assert_eq!(asked[0]["entrypoint"], json!("confirm"));
    assert_eq!(
        asked[0]["plugin_id"],
        json!("mikebronner.agentic-panes-layout")
    );
}

#[test]
fn the_question_names_the_pane_and_the_agent_and_warns_about_the_work() {
    // The user cannot answer well without knowing which pane and which agent, and
    // the point of asking at all is that the work is unrecoverable.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    run(&stub, &[], Some(root.as_path()));
    let question = stub.params_for("plugin.pane.open")[0]["env"]["AGENT_LAYOUT_QUESTION"]
        .as_str()
        .expect("the question must reach the popup")
        .to_string();
    assert!(question.contains("p2"), "{}", question);
    assert!(question.contains("claude"), "{}", question);
    assert!(question.contains("cannot be recovered"), "{}", question);
}

#[test]
fn an_affirmative_answer_closes_the_agent_pane_and_rebuilds_clean() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("close"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(closed(&stub), vec!["p2"], "the agent pane must be closed");
    // Rebuilt clean: the survivor gets the layout's label and a fresh agent.
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "p1"})]
    );
}

#[test]
fn a_refusal_leaves_the_agent_running_and_closes_nothing_of_its_own() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        !closed(&stub).contains(&"p2".to_string()),
        "the agent pane must survive a refusal: {:?}",
        closed(&stub)
    );
}

#[test]
fn a_refusal_still_produces_the_configured_layout_around_the_survivor() {
    // Mike's words were that it should work around it. Aborting the run would leave
    // the user with neither the old layout nor the new one.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"three\"\n\
         [[layouts.three.tabs]]\nname = \"agent\"\n\
         [[layouts.three.tabs.panes]]\nagent = \"claude\"\nlabel = \"agent\"\n\
         [[layouts.three.tabs.panes]]\nsplit = \"right\"\nratio = 0.5\n\
         command = \"lazygit\"\nlabel = \"lazygit\"\n\
         [[layouts.three.tabs.panes]]\nsplit = \"down\"\nratio = 0.6\nlabel = \"shell\"\n",
    );
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);

    // The survivor is the agent's own pane, and the layout is built outward from it.
    let splits = stub.params_for("pane.split");
    assert_eq!(splits.len(), 2);
    assert_eq!(splits[0]["target_pane_id"], json!("p2"));

    // It is relabelled as the layout's agent pane, which is what was asked for.
    assert_eq!(
        stub.params_for("pane.rename")[0],
        json!({"pane_id": "p2", "label": "agent"})
    );
    // And the rest of the layout really happened.
    assert_eq!(stub.params_for("pane.rename").len(), 3);
    assert_eq!(
        stub.params_for("pane.send_input")[0]["text"],
        json!("lazygit")
    );
}

#[test]
fn a_refusal_does_not_start_a_second_agent_in_the_surviving_pane() {
    // Starting another agent where one is already running is the very thing the
    // refusal refused.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.params_for("agent.start").is_empty(),
        "{:?}",
        stub.params_for("agent.start")
    );
    assert!(run.says("already runs an agent"), "{}", run.stderr);
}

#[test]
fn a_refusal_does_not_type_a_command_into_the_surviving_agents_pane() {
    // The pane's foreground process is an agent, so text sent to it is a prompt, not
    // a shell command. That would put words in the agent's mouth.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n\
         [[layouts.one.tabs]]\nname = \"agent\"\n\
         [[layouts.one.tabs.panes]]\nagent = \"claude\"\ncommand = \"echo hello\"\n",
    );
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.params_for("pane.send_input").is_empty(),
        "{:?}",
        stub.params_for("pane.send_input")
    );
    assert!(run.says("did not run the command"), "{}", run.stderr);
}

#[test]
fn a_refusal_still_closes_the_panes_that_hold_no_agent() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        Script {
            tabs: tabs_labelled(&["agent"]),
            panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"},
            {"pane_id": "p3", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
            ..Script::default()
        }
        .answers("keep"),
    );
    run(&stub, &[], Some(root.as_path()));
    let shut = closed(&stub);
    assert!(shut.contains(&"p1".to_string()), "{:?}", shut);
    assert!(shut.contains(&"p3".to_string()), "{:?}", shut);
    assert!(!shut.contains(&"p2".to_string()), "{:?}", shut);
}

#[test]
fn a_refusal_spares_every_agent_pane_not_only_the_survivor() {
    // Found by mutation testing, which is the only reason this is here: deleting the
    // agent check inside the close loop left all 31 other tests green, because every
    // one of them had a single agent pane and that pane was the survivor. A tab with
    // two agents would have had the second one closed after the user said no.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        Script {
            tabs: tabs_labelled(&["agent"]),
            panes: json!({"panes": [
                {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
                {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "claude"},
                {"pane_id": "p3", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "codex"}]}),
            ..Script::default()
        }
        .answers("keep"),
    );
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);

    let shut = closed(&stub);
    assert!(
        !shut.contains(&"p2".to_string()),
        "the survivor was closed: {:?}",
        shut
    );
    assert!(
        !shut.contains(&"p3".to_string()),
        "a second agent pane was closed after the user said no: {:?}",
        shut
    );
    assert!(shut.contains(&"p1".to_string()), "{:?}", shut);
    assert!(run.says("left pane p3 running codex"), "{}", run.stderr);
}

#[test]
fn the_question_names_every_agent_pane_when_there_is_more_than_one() {
    // The user is consenting to destroy all of them, so all of them have to be in
    // the question. Naming one and closing two would make the answer meaningless.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        Script {
            tabs: tabs_labelled(&["agent"]),
            panes: json!({"panes": [
                {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "claude"},
                {"pane_id": "p3", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "codex"}]}),
            ..Script::default()
        }
        .answers("keep"),
    );
    run(&stub, &[], Some(root.as_path()));
    let question = stub.params_for("plugin.pane.open")[0]["env"]["AGENT_LAYOUT_QUESTION"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(question.contains("2 panes"), "{}", question);
    assert!(question.contains("p2"), "{}", question);
    assert!(question.contains("p3"), "{}", question);
}

#[test]
fn an_affirmative_answer_closes_every_agent_pane() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        Script {
            tabs: tabs_labelled(&["agent"]),
            panes: json!({"panes": [
                {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
                {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "claude"},
                {"pane_id": "p3", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "codex"}]}),
            ..Script::default()
        }
        .answers("close"),
    );
    run(&stub, &[], Some(root.as_path()));
    let shut = closed(&stub);
    assert!(shut.contains(&"p2".to_string()), "{:?}", shut);
    assert!(shut.contains(&"p3".to_string()), "{:?}", shut);
}

// ---------------------------------------------------------------------------
// The third choice: change nothing.
//
// `esc` exists so a misfire is free. Without it the cheapest available answer still
// rearranges every other pane in the tab, so an accidental keypress would cost a
// rearranged workspace. Every failure to ask or be answered means this one too.
// ---------------------------------------------------------------------------

#[test]
fn cancelling_changes_nothing_at_all() {
    // Not even the panes holding no agent, which `keep` would have closed. That is the
    // whole difference between the two non-destructive answers.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        Script {
            tabs: tabs_labelled(&["agent"]),
            panes: json!({"panes": [
                {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
                {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
                 "agent": "claude"},
                {"pane_id": "p3", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
            ..Script::default()
        }
        .answers("nothing"),
    );
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.changing().is_empty(),
        "cancelling must touch nothing: {:?}",
        stub.changing()
    );
    assert!(run.says("cancelled"), "{}", run.stderr);
}

#[test]
fn cancelling_and_keeping_differ_in_exactly_one_way() {
    // The pair that pins the distinction. Same fixture, two answers: `keep` closes the
    // bare pane and rebuilds around the agent, `nothing` leaves the tab as it was.
    let fixture = || Script {
        tabs: tabs_labelled(&["agent"]),
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}),
        ..Script::default()
    };

    let keep_dir = TempDir::new();
    let keep_root = solo_config(&keep_dir);
    let keeping = Stub::start(fixture().answers("keep"));
    run(&keeping, &[], Some(keep_root.as_path()));
    assert_eq!(
        closed(&keeping),
        vec!["p1"],
        "keep must close the bare pane"
    );

    let nothing_dir = TempDir::new();
    let nothing_root = solo_config(&nothing_dir);
    let cancelling = Stub::start(fixture().answers("nothing"));
    run(&cancelling, &[], Some(nothing_root.as_path()));
    assert!(
        closed(&cancelling).is_empty(),
        "cancelling must close nothing: {:?}",
        closed(&cancelling)
    );
}

#[test]
fn a_cancelled_tab_does_not_stop_the_other_tabs_of_the_layout() {
    // Cancelling is about the tab whose agent was at risk. A layout's later tabs were
    // never in question, and abandoning them would make one `esc` cost the whole run.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"pair\"\n\
         [[layouts.pair.tabs]]\nname = \"agent\"\n\
         [[layouts.pair.tabs.panes]]\nagent = \"claude\"\n\
         [[layouts.pair.tabs]]\nname = \"notes\"\n\
         [[layouts.pair.tabs.panes]]\ncommand = \"true\"\n",
    );
    let stub = Stub::start(existing_tab_with_an_agent().answers("nothing"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(closed(&stub).is_empty(), "{:?}", closed(&stub));
    assert_eq!(
        stub.params_for("tab.create")[0]["label"],
        json!("notes"),
        "the untouched tab still had to be built"
    );
}

#[test]
fn a_rebuild_with_no_agent_at_risk_never_asks_and_never_stalls() {
    // The common case, and the one that must not be made slow by the confirmation
    // machinery. Nothing is at risk, so there is nothing to consent to.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    let started = std::time::Instant::now();
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.params_for("plugin.pane.open").is_empty(),
        "nothing was at risk, so nothing should have been asked"
    );
    assert!(run.says("rebuilt agent"), "{}", run.stderr);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "a rebuild with nothing at risk must not wait on a popup: {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Every way the question can go wrong changes nothing.
// ---------------------------------------------------------------------------

#[test]
fn a_dismissed_question_closes_nothing_and_says_so() {
    // The stub writes no answer and never lists the popup pane, which is what
    // dismissing it looks like. Silence must never read as consent to destroy an
    // agent's work, and it must not hang for the full timeout either: Herdr may kill
    // the popup's process outright, so it cannot be relied on to write "no" on its
    // way out.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().popup_dismissed());
    let started = std::time::Instant::now();
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        !closed(&stub).contains(&"p2".to_string()),
        "{:?}",
        closed(&stub)
    );
    assert!(run.says("was dismissed"), "{}", run.stderr);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "a dismissed popup must not wait out the whole timeout: {:?}",
        started.elapsed()
    );
}

#[test]
fn an_answer_that_is_none_of_the_three_changes_nothing_and_says_so() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("maybe"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.changing().is_empty(),
        "an unrecognised answer must change nothing at all: {:?}",
        stub.changing()
    );
    assert!(run.says("none of the three choices"), "{}", run.stderr);
}

#[test]
fn a_popup_that_will_not_open_closes_nothing_and_says_so() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(
        existing_tab_with_an_agent()
            .answers("close")
            .failing("plugin.pane.open", "pane_limit_reached"),
    );
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        !closed(&stub).contains(&"p2".to_string()),
        "an unaskable question must not kill an agent: {:?}",
        closed(&stub)
    );
    assert!(
        run.says("could not open the confirmation popup"),
        "{}",
        run.stderr
    );
}

#[test]
fn the_popup_is_never_closed_by_id_because_none_is_returned() {
    // Measured on 0.9.0: plugin.pane.open answers {"type":"ok"} and nothing else, so
    // there is no pane id to hand to plugin.pane.close. Calling it with a guess would
    // close somebody else's pane. The popup ends by its own process exiting.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("close"));
    run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.params_for("plugin.pane.close").is_empty(),
        "{:?}",
        stub.params_for("plugin.pane.close")
    );
}

#[test]
fn a_popup_that_never_starts_changes_nothing_and_says_why() {
    // plugin.pane.open answers ok whether or not the popup's process starts, and a
    // plugin pane cannot be found in pane.list, so the started marker is the only
    // evidence. Without it this would freeze the rebuild for the full timeout.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent());
    let started = std::time::Instant::now();
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        !closed(&stub).contains(&"p2".to_string()),
        "{:?}",
        closed(&stub)
    );
    assert!(run.says("never started"), "{}", run.stderr);
    assert!(
        stub.changing().is_empty(),
        "a question that could not be asked must change nothing: {:?}",
        stub.changing()
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "a popup that never starts must not wait out the whole timeout: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_failed_pane_close_is_reported_and_the_rebuild_continues() {
    // Once a rebuild has started, abandoning it leaves the worst of both layouts.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab().failing("pane.close", "pane_not_found"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("could not close pane p2"), "{}", run.stderr);
    assert_eq!(stub.params_for("agent.start").len(), 1);
}

#[test]
fn a_tab_that_reports_no_panes_is_left_alone_rather_than_rebuilt() {
    // Fail closed on a shape that should not happen. With no pane to survive there
    // is nothing to build from, and inventing one would mean creating a second tab
    // of the same name.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    // The active tab t1 holds a pane, so the working directory still resolves; the
    // layout's own tab, t2, is the one with nothing in it.
    let stub = Stub::start(Script {
        tabs: json!({"tabs": [
            {"tab_id": "t1", "workspace_id": "w9", "label": "something else"},
            {"tab_id": "t2", "workspace_id": "w9", "label": "agent"}]}),
        panes: json!({"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"}]}),
        ..Script::default()
    });
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("reports no panes"), "{}", run.stderr);
    assert!(stub.params_for("pane.close").is_empty());
    assert!(stub.params_for("tab.create").is_empty());
}

// ---------------------------------------------------------------------------
// The popup process itself.
// ---------------------------------------------------------------------------

thread_local! {
    /// What the last `ask_popup` run wrote to its started marker.
    static LAST_STARTED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn ask_popup(typed: &str, extra: &[(&str, &str)]) -> (String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = TempDir::new();
    let answer = dir.join("answer");
    let started = dir.join("started");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .arg("--confirm")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("AGENT_LAYOUT_QUESTION", "Close pane p2 running claude?")
        .env("AGENT_LAYOUT_ANSWER_FILE", &answer)
        .env("AGENT_LAYOUT_STARTED_FILE", &started)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (k, v) in extra {
        command.env(k, v);
    }
    let mut child = command.spawn().expect("cannot run the popup");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(typed.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    LAST_STARTED.with(|slot| {
        *slot.borrow_mut() = std::fs::read_to_string(&started).unwrap_or_default();
    });
    (
        std::fs::read_to_string(&answer).unwrap_or_default(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn the_popup_writes_close_only_for_an_explicit_yes() {
    // The line-reading fallback, which is what runs when stdin is not a terminal. The
    // suite pipes stdin, so this is the path it can drive; the raw-mode keypress path
    // needs a tty and is covered by the fallback sharing its vocabulary.
    for typed in ["y\n", "Y\n", "yes\n", "YES\n"] {
        let (answer, _) = ask_popup(typed, &[]);
        assert_eq!(answer, "close", "typed {:?}", typed);
    }
}

#[test]
fn the_popup_writes_keep_only_for_an_explicit_no() {
    for typed in ["n\n", "N\n", "no\n", "NO\n"] {
        let (answer, _) = ask_popup(typed, &[]);
        assert_eq!(answer, "keep", "typed {:?}", typed);
    }
}

#[test]
fn the_popup_writes_nothing_for_everything_else() {
    // Including a bare Enter and end of input, which is what closing the popup
    // produces. Neither of the two acting answers may be reached by accident: one
    // destroys an agent's work and the other rearranges the tab.
    for typed in ["\n", "maybe\n", "", "yes please\n", " \n", "q\n", "esc\n"] {
        let (answer, _) = ask_popup(typed, &[]);
        assert_eq!(answer, "nothing", "typed {:?}", typed);
    }
}

#[test]
fn the_popup_reports_its_own_process_id_before_asking() {
    // The waiting side cannot learn from plugin.pane.open whether a pane appeared:
    // it answers {"type":"ok"} either way, including when no UI client is attached
    // and no pane is created. This marker is the only evidence, and its contents are
    // what lets a popup the user closes be noticed instead of waited out.
    let (_, _) = ask_popup("n\n", &[]);
    let started = LAST_STARTED.with(|slot| slot.borrow().clone());
    assert!(
        !started.trim().is_empty(),
        "the popup must report that it started"
    );
    assert!(
        started.trim().parse::<u32>().is_ok(),
        "the marker must be a process id, not {:?}",
        started
    );
}

#[test]
fn the_popup_shows_the_question_it_was_given() {
    let (_, shown) = ask_popup("n\n", &[]);
    assert!(shown.contains("Close pane p2 running claude?"), "{}", shown);
}

#[test]
fn the_popup_says_which_way_it_went() {
    let (_, shown) = ask_popup("y\n", &[]);
    assert!(shown.contains("Closing it"), "{}", shown);
    let (_, shown) = ask_popup("n\n", &[]);
    assert!(shown.contains("Leaving it running"), "{}", shown);
}

#[test]
fn the_popup_needs_somewhere_to_answer_and_refuses_without_it() {
    // Fail closed rather than draw a prompt whose answer goes nowhere: the waiting
    // side would then time out and the user would think they had been heard.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agent-layout"))
        .arg("--confirm")
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .output()
        .expect("cannot run the popup");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("AGENT_LAYOUT_ANSWER_FILE"));
}

#[test]
fn the_popup_never_touches_the_socket() {
    // It is a separate process with a terminal and a file, and nothing else. Giving
    // it socket access would mean a second thing that can change the workspace.
    let stub = Stub::start(Script::default());
    let (answer, _) = ask_popup(
        "y\n",
        &[("HERDR_SOCKET_PATH", stub.socket().to_str().unwrap())],
    );
    assert_eq!(answer, "close");
    assert!(stub.requests().is_empty(), "{:?}", stub.requests());
}

#[test]
fn the_confirm_flag_is_refused_twice_like_any_other() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--confirm", "--confirm"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("given more than once"), "{}", run.stderr);
}

#[test]
fn the_plugin_id_the_popup_is_opened_under_matches_the_manifest() {
    // plugin.pane.open names the plugin, so a drift between the code's constant and
    // the manifest's id would open nothing and silently answer no.
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml")).unwrap();
    let declared = manifest
        .parse::<toml::Table>()
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(agent_layout::confirm::PLUGIN_ID, declared);
}
