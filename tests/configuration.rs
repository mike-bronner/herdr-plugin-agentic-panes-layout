//! agent-layout.toml: where it is found, how it fails, and which layout it picks.
//!
//! The file sits beside Herdr's own config.toml and is named for the concern
//! rather than for this plugin, so herdr-plugin-project-finder can read the same
//! file later. Measured on Herdr 0.9.0 in an isolated XDG_CONFIG_HOME: an
//! unrelated sibling file next to config.toml is completely ignored, and
//! `herdr config check` still answers "config: ok" with exit 0.
//!
//! Herdr's own asymmetry is matched deliberately: an unknown key produces a
//! diagnostic and the rest of the file still applies, while a file that cannot be
//! parsed at all is discarded whole and the built-in default applies. Nothing
//! about config is ever fatal, because a syntax error in a file the user owns must
//! not leave a half-built workspace or a dead hook.

mod support;

use agent_layout::config::{self, Direction};
use serde_json::json;
use support::*;

#[test]
fn no_config_file_yields_the_built_in_layout_and_says_so() {
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], None);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("built in, no config file"), "{}", run.stderr);
}

#[test]
fn a_syntax_error_falls_back_to_the_built_in_layout_and_reports_it() {
    // Never fatal. The alternative is a dead hook on a typo in a file Herdr's own
    // config handling would have forgiven.
    let dir = TempDir::new();
    let root = config_root_with(&dir, "default = \"broken\nthis is not toml [[[\n");
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("is not usable"), "{}", run.stderr);
    assert!(run.says("the built-in layout applies"), "{}", run.stderr);
    // And the layout still happened.
    assert_eq!(stub.params_for("pane.split").len(), 2);
}

#[test]
fn an_unknown_key_warns_and_the_rest_of_the_file_still_applies() {
    // The asymmetry Herdr itself implements with serde_ignored: a typo costs the
    // typo and nothing else.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        r#"
default = "one"
colour = "purple"

[[layouts.one.tabs]]
name = "work"
[[layouts.one.tabs.panes]]
command = "gitui"
nonsense = 1
"#,
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("unknown key \"colour\""), "{}", run.stderr);
    assert!(run.says("nonsense"), "{}", run.stderr);
    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t1", "label": "work"})]
    );
    assert_eq!(
        stub.params_for("pane.send_input")[0]["text"],
        json!("gitui")
    );
}

#[test]
fn a_layout_name_nothing_defines_falls_back_and_reports_it() {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"absent\"\n[[layouts.present.tabs]]\nname = \"x\"\n\
         [[layouts.present.tabs.panes]]\ncommand = \"true\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("no layout named \"absent\""), "{}", run.stderr);
    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t1", "label": "agent"})]
    );
}

#[test]
fn a_file_with_no_default_named_falls_back_and_reports_it() {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "[[layouts.present.tabs]]\nname = \"x\"\n\
         [[layouts.present.tabs.panes]]\ncommand = \"true\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("no default layout is named"), "{}", run.stderr);
}

#[test]
fn an_invalid_split_direction_is_refused_at_parse_time() {
    // Herdr accepts right and down and nothing else, measured from the schema's
    // SplitDirection enum at protocol 22. Accepting "vertical" here would produce
    // a config that validates and then fails at the socket, halfway through a tab.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"x\"\n\
         [[layouts.one.tabs.panes]]\ncommand = \"true\"\n\
         [[layouts.one.tabs.panes]]\nsplit = \"vertical\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert!(run.says("is not usable"), "{}", run.stderr);
    // The built-in layout applied instead, so the workspace is not half-built.
    assert_eq!(stub.params_for("pane.split").len(), 2);
}

#[test]
fn a_later_pane_with_no_split_is_refused_with_a_message_naming_it() {
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"x\"\n\
         [[layouts.one.tabs.panes]]\ncommand = \"a\"\n\
         [[layouts.one.tabs.panes]]\ncommand = \"b\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert_eq!(run.status, 0, "{}", run.stderr);
    // The message has to carry the FIX, not only the rule. Somebody who has never read
    // docs/configuration.md must be able to act on it.
    assert!(run.says("pane 2 has no split"), "{}", run.stderr);
    assert!(
        run.says("add split = \"right\" or split = \"down\""),
        "the message must say what to add: {}",
        run.stderr
    );
}

#[test]
fn a_first_pane_carrying_a_split_is_refused() {
    // The first pane is the tab's own pane and is not split into existence, so a
    // split on it is a misunderstanding worth naming rather than ignoring.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"x\"\n\
         [[layouts.one.tabs.panes]]\nsplit = \"right\"\n",
    );
    let stub = Stub::start(Script::default());
    let run = run(&stub, &[], Some(root.as_path()));
    assert!(
        run.says("is the tab's own pane, so it cannot carry a split"),
        "{}",
        run.stderr
    );
    // The fix is two pane blocks, which is not something a reader would guess. This is
    // the exact mistake Mike made in his own config after v0.3.0 shipped.
    assert!(
        run.says("write TWO pane blocks"),
        "the message must name the fix: {}",
        run.stderr
    );
    assert!(run.says("empty first one"), "{}", run.stderr);
}

#[test]
fn a_split_with_no_ratio_omits_the_ratio_rather_than_inventing_one() {
    // Herdr's ratio param is nullable, so leaving it out lets Herdr pick its own
    // default. Substituting 0.5 here would silently override that choice.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "default = \"one\"\n[[layouts.one.tabs]]\nname = \"x\"\n\
         [[layouts.one.tabs.panes]]\ncommand = \"a\"\n\
         [[layouts.one.tabs.panes]]\nsplit = \"down\"\n",
    );
    let stub = Stub::start(Script::default());
    run(&stub, &[], Some(root.as_path()));
    let split = &stub.params_for("pane.split")[0];
    assert_eq!(split["direction"], json!("down"));
    assert!(split.get("ratio").is_none(), "{:?}", split);
}

#[test]
fn the_config_root_is_derived_from_the_plugin_config_dir() {
    // <root>/plugins/config/<plugin id> is what Herdr injects, so walking up three
    // levels follows a relocated config root instead of guessing at one. This is
    // the path that also follows a debug build's herdr-dev directory, because the
    // injected value already points inside it.
    let dir = TempDir::new();
    let root = dir.join("herdr");
    let injected = root
        .join("plugins")
        .join("config")
        .join("mikebronner.agentic-panes-layout");
    std::fs::create_dir_all(&injected).unwrap();
    std::fs::write(
        root.join("agent-layout.toml"),
        "default = \"from-derived-root\"\n\
         [[layouts.from-derived-root.tabs]]\nname = \"derived\"\n\
         [[layouts.from-derived-root.tabs.panes]]\ncommand = \"true\"\n",
    )
    .unwrap();

    let stub = Stub::start(Script::default());
    let run = run_with_env(
        &stub,
        &[],
        None,
        &[("HERDR_PLUGIN_CONFIG_DIR", injected.to_str().unwrap())],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        stub.params_for("tab.rename"),
        vec![json!({"tab_id": "t1", "label": "derived"})]
    );
}

#[test]
fn no_path_in_the_code_hardcodes_the_users_config_directory() {
    // XDG_CONFIG_HOME relocates Herdr's config root and a debug build renames the
    // directory, so a hardcoded ~/.config/herdr is wrong for two independent
    // reasons. Grepping the sources is what keeps it from creeping back.
    for name in [
        "src/config.rs",
        "src/main.rs",
        "src/api.rs",
        "src/layout.rs",
    ] {
        let text = std::fs::read_to_string(name).expect(name);
        assert!(
            !text.contains(".config/herdr"),
            "{} hardcodes the config directory",
            name
        );
    }
}

// ---------------------------------------------------------------------------
// Per-project rules, unit-tested against the matcher directly.
//
// Rules are evaluated in file order and the first match wins. A rule's `path` is
// matched against BOTH the workspace's checkout path and its repo root, so naming
// a repository's root covers every worktree of it while naming one worktree
// targets only that one. This matters because the plugin's main trigger is
// worktree.created, and worktrees do not live under their repo's path.
// ---------------------------------------------------------------------------

fn candidates(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

#[test]
fn a_rule_matches_the_checkout_path() {
    assert!(config::matches_project(
        "/repos/thing",
        &candidates(&["/repos/thing"])
    ));
}

#[test]
fn a_rule_naming_the_repo_root_covers_a_worktree_of_it() {
    // The load-bearing case. The worktree lives at a path with no relation to the
    // repo root, so matching only the checkout path would make a repo-wide rule
    // silently miss every worktree — which is nearly every workspace this plugin
    // sees.
    assert!(config::matches_project(
        "/repos/thing",
        &candidates(&["/worktrees/thing/feature", "/repos/thing"])
    ));
}

#[test]
fn a_rule_naming_one_worktree_does_not_match_a_sibling() {
    assert!(!config::matches_project(
        "/worktrees/thing/other",
        &candidates(&["/worktrees/thing/feature", "/repos/thing"])
    ));
}

#[test]
fn a_trailing_slash_does_not_change_a_match() {
    assert!(config::matches_project(
        "/repos/thing/",
        &candidates(&["/repos/thing"])
    ));
    assert!(config::matches_project(
        "/repos/thing",
        &candidates(&["/repos/thing/"])
    ));
}

#[test]
fn a_rule_is_not_a_prefix_match() {
    // No globs and no prefixes: Mike did not ask for them, and a prefix rule would
    // silently capture every sibling repository under a shared parent.
    assert!(!config::matches_project(
        "/repos",
        &candidates(&["/repos/thing"])
    ));
    assert!(!config::matches_project(
        "/repos/thing",
        &candidates(&["/repos/thing-two"])
    ));
}

#[test]
fn a_glob_is_taken_literally_rather_than_expanded() {
    assert!(!config::matches_project(
        "/repos/*",
        &candidates(&["/repos/thing"])
    ));
}

#[test]
fn an_empty_rule_path_matches_nothing() {
    // Fail closed. An empty path is a mistake, and treating it as "matches
    // everything" would apply one project's layout to every workspace.
    assert!(!config::matches_project("", &candidates(&["/repos/thing"])));
    assert!(!config::matches_project("", &candidates(&[""])));
}

#[test]
fn the_first_matching_rule_wins() {
    let text = r#"
default = "fallback"

[[projects]]
path = "/repos/thing"
layout = "first"

[[projects]]
path = "/repos/thing"
layout = "second"

[[layouts.first.tabs]]
name = "first-tab"
[[layouts.first.tabs.panes]]
command = "true"

[[layouts.second.tabs]]
name = "second-tab"
[[layouts.second.tabs.panes]]
command = "true"

[[layouts.fallback.tabs]]
name = "fallback-tab"
[[layouts.fallback.tabs.panes]]
command = "true"
"#;
    let dir = TempDir::new();
    let root = config_root_with(&dir, text);
    let loaded = config::load_from(&root.join("agent-layout.toml"));
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let chosen = loaded.choose(&candidates(&["/repos/thing"]));
    assert_eq!(chosen.name, "first");
}

#[test]
fn the_default_applies_when_no_rule_matches() {
    let text = r#"
default = "fallback"

[[projects]]
path = "/repos/other"
layout = "first"

[[layouts.first.tabs]]
name = "first-tab"
[[layouts.first.tabs.panes]]
command = "true"

[[layouts.fallback.tabs]]
name = "fallback-tab"
[[layouts.fallback.tabs.panes]]
command = "true"
"#;
    let dir = TempDir::new();
    let root = config_root_with(&dir, text);
    let loaded = config::load_from(&root.join("agent-layout.toml"));
    let chosen = loaded.choose(&candidates(&["/repos/thing"]));
    assert_eq!(chosen.name, "fallback");
    assert!(chosen.rule_path.is_none());
}

#[test]
fn a_matched_rule_is_named_in_the_report() {
    let text = r#"
default = "fallback"

[[projects]]
path = "/repos/thing"
layout = "first"

[[layouts.first.tabs]]
name = "first-tab"
[[layouts.first.tabs.panes]]
command = "true"

[[layouts.fallback.tabs]]
name = "fallback-tab"
[[layouts.fallback.tabs.panes]]
command = "true"
"#;
    let dir = TempDir::new();
    let root = config_root_with(&dir, text);
    let loaded = config::load_from(&root.join("agent-layout.toml"));
    let chosen = loaded.choose(&candidates(&["/repos/thing"]));
    assert_eq!(chosen.rule_path.as_deref(), Some("/repos/thing"));
}

#[test]
fn a_rule_carries_only_a_path_and_a_layout() {
    // Mike amended the design mid-way: no repo, repo_name or repo_path keys, just
    // `path`. A key added later would show up here as an unknown-key diagnostic.
    let dir = TempDir::new();
    let root = config_root_with(
        &dir,
        "[[projects]]\npath = \"/a\"\nlayout = \"x\"\nrepo_name = \"a\"\n",
    );
    let loaded = config::load_from(&root.join("agent-layout.toml"));
    assert!(
        loaded
            .diagnostics
            .iter()
            .any(|d| d.contains("unknown key") && d.contains("repo_name")),
        "{:?}",
        loaded.diagnostics
    );
}

#[test]
fn the_built_in_layout_is_the_v0_2_0_geometry_without_the_labels() {
    // The Rust literal is the single source of the default, so its shape is pinned
    // here rather than only through the requests it produces.
    let layout = config::built_in_layout();
    assert_eq!(layout.tabs.len(), 1);
    let tab = &layout.tabs[0];
    assert_eq!(tab.name, "agent");
    assert_eq!(tab.panes.len(), 3);

    assert_eq!(tab.panes[0].agent.as_deref(), Some("claude"));
    assert!(tab.panes[0].split.is_none());

    assert_eq!(tab.panes[1].split, Some(Direction::Right));
    assert_eq!(tab.panes[1].ratio, Some(0.5));
    assert_eq!(tab.panes[1].command.as_deref(), Some("lazygit"));

    assert_eq!(tab.panes[2].split, Some(Direction::Down));
    assert_eq!(tab.panes[2].ratio, Some(0.6));

    for pane in &tab.panes {
        assert!(pane.label.is_none(), "the default must impose no label");
    }
}

#[test]
fn the_built_in_config_passes_its_own_validation() {
    // A default that could not be written as a config file would be a default
    // nobody could copy into one.
    let found = config::validate(&config::built_in_config());
    assert!(
        found.problems.is_empty(),
        "the built-in config must be valid: {:?}",
        found.problems
    );
    assert!(found.unusable.is_empty());
}

#[test]
fn the_documented_example_parses_and_matches_the_built_in_geometry() {
    // The README shows the built-in layout written out as TOML, so a reader can
    // copy it and start editing. If that block ever stops meaning what the Rust
    // literal means, the README is lying.
    let text = r#"
default = "three-pane"

[[layouts.three-pane.tabs]]
name = "agent"

[[layouts.three-pane.tabs.panes]]
agent = "claude"

[[layouts.three-pane.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"

[[layouts.three-pane.tabs.panes]]
split = "down"
ratio = 0.6
"#;
    let dir = TempDir::new();
    let root = config_root_with(&dir, text);
    let loaded = config::load_from(&root.join("agent-layout.toml"));
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let chosen = loaded.choose(&[]);
    assert_eq!(chosen.layout, config::built_in_layout());
}
