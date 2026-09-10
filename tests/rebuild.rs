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
//! **A rebuild replaces the whole tab in one `layout.apply`.** There is no pane-by-pane
//! close and no survivor: a `tab_id` destroys every pane in that tab and builds the
//! configured tree in its place. So a tab running an agent holds work that cannot be
//! recovered, that case asks first, and every non-affirmative answer leaves the tab
//! exactly as it was.

mod support;

use serde_json::json;
use support::*;

/// A one-pane layout, so a rebuild's arithmetic is easy to read.
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
    // Rebuilding is about existing tabs. A missing one is still just built, and the
    // two cases take different targets: replacing one tab, adding another.
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
        stub.applies(),
        vec![
            (Applied::Replace("t1".into()), "agent".to_string()),
            (Applied::Add("w9".into()), "notes".to_string()),
        ]
    );
}

// ---------------------------------------------------------------------------
// Rebuilding a tab with no agent in it.
// ---------------------------------------------------------------------------

#[test]
fn rebuilding_a_plain_tab_replaces_it_and_asks_nothing() {
    // No agent means no unrecoverable work, so no popup. The tab is replaced whole,
    // which is what `layout.apply` with a `tab_id` does.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.params_for("plugin.pane.open").is_empty(),
        "a plain rebuild must not ask anything"
    );
    assert_eq!(
        stub.applies(),
        vec![(Applied::Replace("t1".into()), "agent".to_string())]
    );
}

#[test]
fn a_rebuild_closes_no_pane_by_hand() {
    // The old engine closed the extra panes one at a time and kept a survivor to split
    // from. Replacing the tab destroys them all at once, so a stray `pane.close` here
    // would be a leftover of that engine acting on ids that no longer exist.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.params_for("pane.close").is_empty(),
        "{:?}",
        stub.params_for("pane.close")
    );
    assert!(
        stub.params_for("pane.split").is_empty(),
        "{:?}",
        stub.params_for("pane.split")
    );
}

#[test]
fn a_rebuilt_tab_is_relabelled_and_gets_its_agent() {
    // The panes are new ones the apply made, so the label and the agent go to the ids
    // it answered with, not to the ids that were there before.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    assert_eq!(
        stub.params_for("pane.rename"),
        vec![json!({"pane_id": "t2p1", "label": "agent"})]
    );
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "t2p1"})]
    );
}

#[test]
fn a_rebuild_does_not_rename_the_tab_it_is_rebuilding() {
    // The tab already carries the layout's name, which is how it was found, and the
    // replacement carries the name again in `tab_label`. A `tab.rename` on top of that
    // would be a wasted call against a tab id the apply has already retired.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_plain_tab());
    run(&stub, &[], Some(root.as_path()));
    assert!(stub.params_for("tab.rename").is_empty());
    assert!(stub.params_for("tab.create").is_empty());
    assert_eq!(
        stub.params_for("layout.apply")[0]["tab_label"],
        json!("agent")
    );
}

#[test]
fn a_rebuild_builds_every_pane_of_the_tab_in_the_one_call() {
    // The split lives in the tree now, so a two-pane tab is still one round trip. The
    // direction and ratio are the ones configured for the pane being created.
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

    let applied = stub.params_for("layout.apply");
    assert_eq!(applied.len(), 1, "one call builds the whole tab");
    let root_node = &applied[0]["root"];
    assert_eq!(root_node["type"], json!("split"));
    assert_eq!(root_node["direction"], json!("right"));
    assert_eq!(root_node["ratio"], json!(0.5));
    assert_eq!(root_node["first"]["type"], json!("pane"));
    assert_eq!(root_node["second"]["type"], json!("pane"));

    // And the two panes it made are the ones the agent and the command land in.
    assert_eq!(stub.params_for("agent.start")[0]["pane_id"], json!("t2p1"));
    assert_eq!(
        stub.params_for("pane.send_input")[0]["pane_id"],
        json!("t2p2")
    );
}

#[test]
fn panes_of_another_tab_are_not_destroyed_by_a_rebuild() {
    // **This pins a decision Mike made, not an incidental limit.** His words: only
    // destroy the tabs named in the layout. He took it after being shown the
    // consequence of a wider scope, which is that one keypress would destroy an
    // unrelated tab. So a future reader should not treat this as a limitation to lift:
    // a tab the layout never mentions survives every rebuild, by choice.
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
    assert_eq!(
        stub.applies(),
        vec![(Applied::Replace("t1".into()), "agent".to_string())],
        "only the tab the layout names may be replaced"
    );
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
    let stub = Stub::start(existing_tab_with_an_agent().answers("nothing"));
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
fn an_affirmative_answer_replaces_the_tab_and_rebuilds_clean() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("close"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.applies(),
        vec![(Applied::Replace("t1".into()), "agent".to_string())],
        "the tab holding the agent must be replaced"
    );
    // Rebuilt clean: the new pane gets the layout's label and a fresh agent.
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "claude", "pane_id": "t2p1"})]
    );
}

#[test]
fn an_affirmative_answer_covers_every_agent_pane_at_once() {
    // Two agents, one question, one replacement. The old engine closed them one by one
    // and could get halfway; this cannot, because the tab goes as a unit.
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
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(stub.params_for("plugin.pane.open").len(), 1);
    assert_eq!(
        stub.applies(),
        vec![(Applied::Replace("t1".into()), "agent".to_string())]
    );
}

#[test]
fn the_question_names_every_agent_pane_when_there_is_more_than_one() {
    // The user is consenting to destroy all of them, so all of them have to be in
    // the question. Naming one and destroying two would make the answer meaningless.
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
        .answers("nothing"),
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

// ---------------------------------------------------------------------------
// The second answer: change nothing.
//
// There used to be a third, which kept the agent and built the layout around it. The
// engine cannot express it: a `tab_id` replaces the tab wholesale and `pane_id` on a
// leaf is output-only, so a running agent cannot be carried into a new tree. Mike
// dropped the requirement rather than the rewrite. What is left is a single acting
// answer, and every other outcome meaning the tab is not touched.
// ---------------------------------------------------------------------------

#[test]
fn cancelling_changes_nothing_at_all() {
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
    assert!(run.says("left agent untouched"), "{}", run.stderr);
}

#[test]
fn a_cancelled_tab_is_not_reported_as_skipped_by_the_guard() {
    // Two ways to leave a tab alone, and only one of them is the user's own answer.
    // Reporting a cancelled rebuild as "already there" would name the guard as the
    // reason and hide the fact that a question was asked and declined.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("nothing"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(!run.says("already there"), "{}", run.stderr);
}

#[test]
fn the_answer_that_used_to_keep_the_agent_now_changes_nothing() {
    // `keep` meant "keep the agent and build around it". Nothing can send it any more,
    // but a popup left over from an older build could, and the only other answer
    // destroys the pane it was trying to protect. It must not fall through.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("keep"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.changing().is_empty(),
        "the retired answer must change nothing: {:?}",
        stub.changing()
    );
}

#[test]
fn a_tab_with_two_agents_is_left_whole_when_the_answer_is_not_affirmative() {
    // Mutation testing put the older form of this here: an engine that spared only the
    // pane it had picked out would close the second agent after the user said no. This
    // engine cannot half-act, and this is what proves the question covers the tab.
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
        .answers("nothing"),
    );
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        stub.changing().is_empty(),
        "no part of the tab may be acted on: {:?}",
        stub.changing()
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
    assert_eq!(
        stub.applies(),
        vec![(Applied::Add("w9".into()), "notes".to_string())],
        "the tab that was never in question still had to be built"
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
fn a_dismissed_question_changes_nothing_and_says_so() {
    // The stub writes no answer and never lists the popup pane, which is what
    // dismissing it looks like. Silence must never read as consent to destroy an
    // agent's work, and it must not hang for the full timeout either: Herdr may kill
    // the popup's process outright, so it cannot be relied on to write an answer on
    // its way out.
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().popup_dismissed());
    let started = std::time::Instant::now();
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(stub.changing().is_empty(), "{:?}", stub.changing());
    assert!(run.says("was dismissed"), "{}", run.stderr);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "a dismissed popup must not wait out the whole timeout: {:?}",
        started.elapsed()
    );
}

#[test]
fn an_answer_that_is_neither_choice_changes_nothing_and_says_so() {
    let dir = TempDir::new();
    let root = solo_config(&dir);
    let stub = Stub::start(existing_tab_with_an_agent().answers("maybe"));
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(
        stub.changing().is_empty(),
        "an unrecognised answer must change nothing at all: {:?}",
        stub.changing()
    );
    assert!(run.says("neither choice"), "{}", run.stderr);
}

#[test]
fn a_popup_that_will_not_open_changes_nothing_and_says_so() {
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
        stub.changing().is_empty(),
        "an unaskable question must not destroy a tab: {:?}",
        stub.changing()
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
fn a_tab_that_reports_no_panes_is_rebuilt_like_any_other() {
    // It used to be left alone, because the old engine needed a pane to survive and
    // split from. This one builds the tree from nothing, so an empty tab is simply the
    // easiest case: nothing is at risk in it, so it is not even a question.
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
    assert!(stub.params_for("plugin.pane.open").is_empty());
    assert_eq!(
        stub.applies(),
        vec![(Applied::Replace("t2".into()), "agent".to_string())],
        "the empty tab is the one replaced, not the active one"
    );
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
fn the_popup_writes_nothing_for_everything_else() {
    // Including a bare Enter, end of input, and the word that used to mean the third
    // answer. The one acting answer destroys a tab that may be running an agent, so it
    // must not be reachable by accident.
    for typed in [
        "\n",
        "n\n",
        "N\n",
        "no\n",
        "NO\n",
        "keep\n",
        "maybe\n",
        "",
        "yes please\n",
        " \n",
        "q\n",
        "esc\n",
    ] {
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
    assert!(shown.contains("Changing nothing"), "{}", shown);
}

#[test]
fn the_popup_offers_two_choices_and_no_more() {
    // The screen is the whole interface, so an offer it does not honour is a lie. `n`
    // used to be listed as its own choice; it is not bound to anything now, and listing
    // it would promise an outcome nothing produces.
    let (_, shown) = ask_popup("", &[]);
    assert!(shown.contains("close it and rebuild the tab"), "{}", shown);
    assert!(shown.contains("change nothing"), "{}", shown);
    assert!(
        !shown.contains("keep"),
        "the retired third choice is still on screen: {}",
        shown
    );
}

#[test]
fn the_popup_offers_no_key_that_is_not_bound() {
    // A dialog listing a key that does nothing is worse than one listing fewer, because
    // the user presses it and reads the silence as a broken plugin. `n` and `q` were
    // both bound once and are not now, so neither may be offered as a choice.
    let (_, shown) = ask_popup("", &[]);
    for line in shown.lines() {
        let offered = line.trim_start();
        for gone in ["n ", "N ", "q ", "no ", "esc, n"] {
            assert!(
                !offered.starts_with(gone),
                "the popup still offers an unbound key: {:?}",
                line
            );
        }
    }
}

#[test]
fn the_popup_tells_the_user_a_click_dismisses() {
    // Herdr forwards a click inside a plugin pane to that pane, measured on 0.9.0, so
    // the binding is real. An offer the screen does not make is a feature nobody finds.
    let (_, shown) = ask_popup("", &[]);
    assert!(shown.to_lowercase().contains("click"), "{}", shown);
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
