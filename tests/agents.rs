//! Classifying `agent.start`, and deduping a derived name.
//!
//! An error is not automatically a failure. Herdr's own docs: "If the agent is
//! blocked during startup, the command returns `agent_not_ready` immediately but
//! keeps the name available for `agent read` and `agent send-keys`." The agent
//! DID start. On a fresh worktree that is Claude Code's trust-this-folder prompt,
//! which is the normal case, and reporting it as a failed layout was the bug.
//!
//! The socket API answers with a structured `code`, so the classification reads a
//! field rather than pattern-matching stderr text the way v0.2.0 had to. The old
//! suite needed a test proving the code was matched and not merely mentioned in a
//! message; that failure mode no longer exists, and is noted here rather than
//! silently dropped.

mod support;

use serde_json::json;
use support::*;

fn names(stub: &Stub) -> Vec<String> {
    stub.params_for("agent.start")
        .iter()
        .filter_map(|p| p.get("name")?.as_str().map(|s| s.to_string()))
        .collect()
}

#[test]
fn a_started_agent_is_reported_by_the_name_it_started_under() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], None);
    assert!(run.says("claude started as \"proj-one\""), "{}", run.stderr);
}

#[test]
fn agent_not_ready_is_a_success_with_its_own_message() {
    let stub = Stub::start(Script::default().agent_error("agent_not_ready"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("waiting on a prompt"), "{}", run.stderr);
    assert!(!run.says("did not start"), "{}", run.stderr);
}

#[test]
fn a_busy_pane_is_waited_out_rather_than_reported() {
    // The failure this fixes. A real worktree creation logged "agent target pane wF:p1
    // is not an available shell (agent_pane_busy)", panes built correctly and no agent.
    // The pane's shell simply had not reached its prompt yet: measured at 230-260ms for
    // this user's zsh, dominated by `mise activate`.
    let stub = Stub::start(Script::default().busy_for(2));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        names(&stub).len(),
        3,
        "two busy answers then the real start: {:?}",
        names(&stub)
    );
    assert!(run.says("was not at a prompt yet"), "{}", run.stderr);
}

#[test]
fn waiting_for_a_pane_does_not_burn_agent_names() {
    // The two retries are different in kind. A taken name is permanent, so that retry
    // changes the name. A busy pane is transient, so this one must NOT: coming back
    // under `proj-one-3` because the shell was slow would break the reservation that
    // `herdr agent prompt <name>` depends on.
    let stub = Stub::start(Script::default().busy_for(3));
    run(&stub, &[], None);
    let tried = names(&stub);
    assert!(
        tried.iter().all(|n| n == "proj-one"),
        "the name must not advance while waiting: {:?}",
        tried
    );
}

#[test]
fn a_pane_that_is_never_free_is_fatal_and_says_what_herdr_said() {
    // Bounded on purpose. A pane held by a real editor or a running command never
    // becomes free, and an unbounded wait would turn a clear failure into a hung hook.
    // Slow by design: it spends the whole 5s budget before giving up.
    let stub = Stub::start(Script::default().agent_error("agent_pane_busy"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("did not start"), "{}", run.stderr);
    // The message has to be about the pane, not about a slow shell.
    assert!(run.says("still busy after"), "{}", run.stderr);
    assert!(run.says("something is running in it"), "{}", run.stderr);
    // And it must carry Herdr's own words. A silent give-up is the invisibility this
    // plugin has spent its recent history removing.
    assert!(run.says("agent_pane_busy"), "{}", run.stderr);
    assert!(!run.says("waiting on a prompt"), "{}", run.stderr);
}

#[test]
fn the_wait_is_bounded_rather_than_open_ended() {
    // The bound, asserted as a real elapsed time rather than only as a constant. A
    // regression to an unbounded loop would hang here instead of failing.
    let stub = Stub::start(Script::default().agent_error("agent_pane_busy"));
    let started = std::time::Instant::now();
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the wait must be bounded: {:?}",
        started.elapsed()
    );
}

#[test]
fn any_other_error_code_is_still_fatal() {
    let stub = Stub::start(Script::default().agent_error("unsupported_agent_kind"));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("did not start"), "{}", run.stderr);
}

#[test]
fn herdrs_own_error_text_reaches_the_log() {
    // The message is what tells the reader which agent kind or pane was refused.
    let stub = Stub::start(Script::default().agent_error("agent_pane_busy"));
    let run = run(&stub, &[], None);
    assert!(run.says("agent_pane_busy"), "{}", run.stderr);
}

#[test]
fn a_taken_derived_name_is_retried_with_a_suffix() {
    let stub = Stub::start(Script::default().taken(&["proj-one"]));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(names(&stub), vec!["proj-one", "proj-one-2"]);
}

#[test]
fn the_suffix_increments_until_a_free_name_is_found() {
    // Also pins that this is a REAL loop that re-reads each answer, rather than a
    // suffix computed once from the first failure. A one-shot implementation that
    // jumped straight to -3 would redden here even though it found a free name.
    let stub = Stub::start(Script::default().taken(&["proj-one", "proj-one-2"]));
    run(&stub, &[], None);
    assert_eq!(names(&stub), vec!["proj-one", "proj-one-2", "proj-one-3"]);
}

#[test]
fn a_free_name_is_not_retried_at_all() {
    let stub = Stub::start(Script::default());
    run(&stub, &[], None);
    assert_eq!(names(&stub).len(), 1);
}

#[test]
fn the_success_message_names_the_agent_that_actually_started() {
    // Reporting the base name after falling through to a suffix would send the
    // reader to `herdr agent prompt proj-one`, which reaches somebody else's agent.
    let stub = Stub::start(Script::default().taken(&["proj-one"]));
    let run = run(&stub, &[], None);
    assert!(run.says("started as \"proj-one-2\""), "{}", run.stderr);
    assert!(!run.says("started as \"proj-one\""), "{}", run.stderr);
}

#[test]
fn exhausting_the_retries_is_fatal_and_says_what_it_tried() {
    // An unbounded retry against an error that is not really about the name would
    // spin forever, so there is a cap. Exhausting it must fail loudly.
    let mut taken = vec!["proj-one".to_string()];
    taken.extend((2..=20).map(|n| format!("proj-one-{}", n)));
    let borrowed: Vec<&str> = taken.iter().map(String::as_str).collect();
    let stub = Stub::start(Script::default().taken(&borrowed));
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("every agent name from \"proj-one\" to \"proj-one-20\" is already taken"),
        "{}",
        run.stderr
    );
}

#[test]
fn the_cap_is_reached_rather_than_overshot() {
    let mut taken = vec!["proj-one".to_string()];
    taken.extend((2..=20).map(|n| format!("proj-one-{}", n)));
    let borrowed: Vec<&str> = taken.iter().map(String::as_str).collect();
    let stub = Stub::start(Script::default().taken(&borrowed));
    run(&stub, &[], None);
    assert_eq!(names(&stub).len(), 20);
}

#[test]
fn the_base_is_trimmed_so_the_suffix_fits_in_thirty_two_characters() {
    // Herdr names are at most 32 characters, and the BASE is trimmed rather than
    // the suffix. Trimming the suffix would produce a 33-character name Herdr
    // rejects, and would also let two different bases collapse onto one name.
    let base = "a".repeat(32);
    let stub = Stub::start(Script {
        workspaces: json!({"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1",
             "label": "a".repeat(40), "focused": true}]}),
        ..Script::default().taken(&[base.as_str()])
    });
    run(&stub, &[], None);
    let expected = format!("{}-2", "a".repeat(30));
    assert_eq!(expected.len(), 32);
    assert_eq!(names(&stub), vec![base, expected]);
}

#[test]
fn a_supplied_name_reaches_agent_start_verbatim() {
    // A caller reserves an exact string so `herdr agent prompt <name>` reaches
    // this workspace, so it must not be lowercased, prefixed or truncated. The
    // derivation would change every one of these, which is why they are the fixture.
    for supplied in ["Weird_Name", "9leading", &"x".repeat(40)] {
        let stub = Stub::start(Script::default());
        run(&stub, &["--agent-name", supplied], None);
        assert_eq!(names(&stub), vec![supplied.to_string()], "{}", supplied);
    }
}

#[test]
fn agent_not_ready_on_a_supplied_name_is_also_a_success() {
    // The supplied-name and derived-name paths classify the same three answers, and
    // a mutation test found this case covered on one path only. Both are pinned now.
    let stub = Stub::start(Script::default().agent_error("agent_not_ready"));
    let run = run(&stub, &["--agent-name", "picked"], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        run.says("started as \"picked\" and is waiting on a prompt"),
        "{}",
        run.stderr
    );
}

#[test]
fn a_supplied_name_that_hits_another_error_is_fatal() {
    let stub = Stub::start(Script::default().agent_error("agent_pane_busy"));
    let run = run(&stub, &["--agent-name", "picked"], None);
    assert_eq!(run.status, 1);
    assert!(run.says("did not start as \"picked\""), "{}", run.stderr);
}

#[test]
fn agent_not_ready_on_a_retried_name_is_still_a_success() {
    // The two classifications compose: the retry finds a free name, and that
    // attempt then reports the agent is waiting at a prompt.
    let stub = Stub::start(
        Script::default()
            .taken(&["proj-one"])
            .agent_error("agent_not_ready"),
    );
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(
        run.says("started as \"proj-one-2\" and is waiting on a prompt"),
        "{}",
        run.stderr
    );
}

#[test]
fn another_error_code_during_the_retry_is_still_fatal() {
    // Two codes are retried and everything else stops at once. The fixture used to be
    // agent_pane_busy, which is now one of the two, so it needed a genuinely fatal code
    // instead: with the old one this asserted 2 attempts and saw 21.
    let stub = Stub::start(
        Script::default()
            .taken(&["proj-one"])
            .agent_error("unsupported_agent_kind"),
    );
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 1);
    assert!(run.says("did not start"), "{}", run.stderr);
    assert_eq!(
        names(&stub).len(),
        2,
        "the taken name, then one fatal answer: {:?}",
        names(&stub)
    );
}

#[test]
fn a_supplied_name_that_is_taken_is_fatal_and_never_retried() {
    // The retry is for DERIVED names only. Silently starting the agent as
    // something else defeats the reservation more completely than refusing does.
    let stub = Stub::start(Script::default().taken(&["reserved-2"]));
    let run = run(&stub, &["--agent-name", "reserved-2"], None);
    assert_eq!(run.status, 1);
    assert!(
        run.says("the agent name \"reserved-2\" given with --agent-name is already taken"),
        "{}",
        run.stderr
    );
    assert_eq!(names(&stub), vec!["reserved-2"]);
}

#[test]
fn a_supplied_name_names_the_first_agent_pane_and_the_rest_derive() {
    // One supplied name across several agent panes is underspecified, so it binds
    // to the first and the others fall back to the derivation and its dedupe.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "two-agents"
[[layouts.two-agents.tabs]]
name = "agent"
[[layouts.two-agents.tabs.panes]]
agent = "claude"
[[layouts.two-agents.tabs.panes]]
split = "right"
ratio = 0.5
agent = "claude"
"#,
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &["--agent-name", "picked"], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(names(&stub), vec!["picked", "proj-one"]);
}

#[test]
fn an_agent_is_started_in_the_pane_that_declares_it() {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "agent-second"
[[layouts.agent-second.tabs]]
name = "agent"
[[layouts.agent-second.tabs.panes]]
command = "lazygit"
[[layouts.agent-second.tabs.panes]]
split = "down"
ratio = 0.5
agent = "codex"
"#,
    );
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));
    assert_eq!(
        stub.params_for("agent.start"),
        vec![json!({"name": "proj-one", "kind": "codex", "pane_id": "p2"})]
    );
}
