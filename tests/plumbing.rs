//! The manifest, the sh shim, the event hook, and drift between code and docs.
//!
//! Herdr re-reads herdr-plugin.toml from disk when it dispatches an event rather
//! than trusting the copy it cached in plugins.json, measured on 0.8.2. An edit
//! takes effect on the very next event. The other half of that is what these tests
//! are for: a syntax error in the manifest stops every dispatch for this plugin,
//! with no toast, no error and nothing surfaced. The plugin simply goes quiet, and
//! the first sign is a worktree that never gets laid out.
//!
//! Unlike the previous Python suite, parsing the manifest needs no special
//! interpreter here: the crate already depends on a TOML parser, so the check that
//! used to be skipped on macOS Python 3.9 now always runs. The loud skip banner
//! that suite printed is gone because the reason for it is gone.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {}", path.display(), e))
}

fn manifest() -> toml::Table {
    read("herdr-plugin.toml")
        .parse::<toml::Table>()
        .expect("herdr-plugin.toml is not valid TOML")
}

#[test]
fn the_manifest_parses() {
    manifest();
}

#[test]
fn a_typo_in_the_manifest_is_really_caught() {
    // A canary on the test above, which would pass for two very different reasons:
    // the manifest is valid, or nothing is really parsing it. The fixture is the
    // REAL manifest plus one unterminated string, which is what a typo looks like.
    let typo = format!("{}\nname = \"unterminated\n", read("herdr-plugin.toml"));
    assert!(typo.parse::<toml::Table>().is_err());
}

#[test]
fn the_manifest_subscribes_exactly_the_events_that_were_argued_for() {
    // Both halves matter. Losing worktree.created stops the layout entirely.
    // Gaining an unplanned subscription is worse than useless: the events fire
    // about a millisecond apart, Herdr does not serialize hooks, and two
    // concurrent acting copies would both see one free tab and both build it.
    // Gate 1 in bin/on-event absorbs that, which is what makes worktree.opened
    // safe to subscribe as log-only instrumentation.
    //
    // Pinned to the exact list on purpose, never a membership check: the whole
    // value of this test is catching a subscription nobody argued for.
    let events: Vec<String> = manifest()["events"]
        .as_array()
        .expect("[[events]] must be an array")
        .iter()
        .map(|e| e["on"].as_str().expect("each event needs `on`").to_string())
        .collect();
    assert_eq!(events, vec!["worktree.created", "worktree.opened"]);
}

#[test]
fn every_event_dispatches_the_hook_and_nothing_else() {
    for event in manifest()["events"].as_array().unwrap() {
        let command: Vec<String> = event["command"]
            .as_array()
            .expect("each event needs `command`")
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        assert_eq!(command, vec!["sh", "bin/on-event"]);
    }
}

#[test]
fn the_herdr_floor_matches_the_sibling_plugin() {
    // 0.9.0 matches `mikebronner.project-finder`, which declares the same. It is a
    // consistency decision across Mike's two plugins rather than a technical
    // requirement: every method this plugin sends exists at protocol 20, which is
    // 0.8.2, and that was measured rather than assumed.
    assert_eq!(
        manifest()["min_herdr_version"].as_str(),
        Some("0.9.0"),
        "the floor moved; it tracks the sibling plugin, not the protocol"
    );
}

#[test]
fn the_manifest_declares_the_action_the_keybinding_names() {
    // A plugin manifest cannot declare keybindings: RawPluginManifest has no `keys`
    // field at either version, so a [[keys.command]] block inside one is silently
    // ignored. What it can declare is an action, which the user's own binding then
    // names as `type = "plugin_action"`.
    //
    // The action's command is the shim, not target/release, so a linked checkout still
    // rebuilds when its sources change.
    let parsed = manifest();
    let actions = parsed["actions"]
        .as_array()
        .expect("[[actions]] must be an array");
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action["id"].as_str(), Some("apply"));
    let command: Vec<String> = action["command"]
        .as_array()
        .expect("an action needs a command")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(command, vec!["sh", "bin/agent-layout", "--rebuild"]);
}

#[test]
fn the_readme_documents_the_plugin_action_binding_that_the_manifest_provides() {
    // The binding a reader copies is `<plugin id>.<action id>`, so it drifts the
    // moment either half moves. Two of them disagreeing is worse than one alone: the
    // reader's keybinding silently does nothing.
    let parsed = manifest();
    let expected = format!(
        "{}.{}",
        parsed["id"].as_str().unwrap(),
        parsed["actions"].as_array().unwrap()[0]["id"]
            .as_str()
            .unwrap()
    );
    let readme = read("README.md");
    assert!(
        readme.contains(&expected),
        "the README does not name the action {}",
        expected
    );
    assert!(
        readme.contains("type = \"plugin_action\""),
        "the README must document the plugin_action form"
    );
}

#[test]
fn the_readme_keeps_the_shell_binding_as_a_fallback_with_its_reason() {
    // The shell form still works and some people will want it, but its absolute-path
    // requirement is not arbitrary: a type = "shell" process is handed no
    // HERDR_PLUGIN_ROOT to resolve a relative path against. Documenting the rule
    // without the reason invites somebody to "simplify" it back to a relative path.
    let readme = read("README.md");
    assert!(readme.contains("type = \"shell\""));
    assert!(
        readme.contains("HERDR_PLUGIN_ROOT"),
        "the README must say why the shell form needs an absolute path"
    );
}

#[test]
fn the_readme_states_the_same_herdr_floor_as_the_manifest() {
    // min_herdr_version is a promise made to somebody who has not installed this
    // yet, and the README repeats it in prose, so it drifts the moment one of the
    // two moves. Two numbers that disagree are worse than either alone: one of
    // them is telling a reader their Herdr is new enough when it is not.
    let floor = manifest()["min_herdr_version"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        read("README.md").contains(&format!("Requires Herdr {} or newer", floor)),
        "the README does not state the floor {}",
        floor
    );
}

#[test]
fn the_manifest_version_matches_the_crate_version() {
    assert_eq!(
        manifest()["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "herdr-plugin.toml and Cargo.toml disagree about the version"
    );
}

#[test]
fn the_plugin_id_is_unchanged() {
    // A rename would orphan nothing now that the config lives outside the
    // plugin config directory, but the README and the picker's default both name
    // this id, so it is pinned rather than assumed.
    assert_eq!(
        manifest()["id"].as_str(),
        Some("mikebronner.agentic-panes-layout")
    );
}

#[test]
fn the_manifest_declares_a_build_step_that_runs_the_build_script() {
    // [[build]] runs once during `herdr plugin install owner/repo`, before Herdr
    // registers the plugin. Without it, somebody installing from GitHub has their
    // FIRST worktree creation stall for a minute inside an event hook while cargo
    // compiles, with only a toast to explain the pause.
    //
    // It does NOT run on `plugin link` and does NOT run on update, both verified;
    // see docs/design.md. That is why bin/agent-layout still builds on demand, and
    // why this test does not license removing it.
    let parsed = manifest();
    let build = parsed["build"]
        .as_array()
        .expect("[[build]] must be an array");
    assert_eq!(build.len(), 1, "one build step, not several");
    let command: Vec<String> = build[0]["command"]
        .as_array()
        .expect("a build step needs a command")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(command, vec!["sh", "bin/build"]);
}

#[test]
fn the_build_script_exists_and_is_the_only_place_cargo_is_searched_for() {
    // The manifest's build step and the shim share one implementation. Two copies of
    // the cargo search would drift, and the rustup-directory fix would land in one.
    let build = read("bin/build");
    assert!(build.contains("/opt/homebrew/opt/rustup/bin/cargo"));
    assert!(
        build.contains("CARGO_DIR"),
        "the rustup PATH fix must live here"
    );
    let shim = read("bin/agent-layout");
    assert!(
        shim.contains("bin/build"),
        "the shim must delegate to bin/build"
    );
    assert!(
        !shim.contains("rustup/bin/cargo"),
        "the cargo search must not be duplicated into the shim"
    );
}

#[test]
fn the_manifest_declares_the_confirmation_popup() {
    // plugin.pane.open names an entrypoint from [[panes]], so a missing or renamed
    // entry means the confirmation never opens — and an unaskable question answers
    // no, which would silently stop rebuilds from ever closing an agent pane.
    let parsed = manifest();
    let panes = parsed["panes"]
        .as_array()
        .expect("[[panes]] must be an array");
    assert_eq!(panes.len(), 1, "one popup, not several");
    let pane = &panes[0];
    assert_eq!(pane["id"].as_str(), Some("confirm"));
    assert_eq!(pane["placement"].as_str(), Some("popup"));
    // Herdr accepts width and height only when placement is popup.
    assert!(pane.get("width").is_some(), "a popup needs a width");
    assert!(pane.get("height").is_some(), "a popup needs a height");
    let command: Vec<String> = pane["command"]
        .as_array()
        .expect("the popup needs a command")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(command, vec!["sh", "bin/agent-layout", "--confirm"]);
}

#[test]
fn the_popup_entrypoint_the_code_opens_is_the_one_the_manifest_declares() {
    let declared = manifest()["panes"].as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(agent_layout::confirm::PANE_ENTRYPOINT, declared);
}

// ---------------------------------------------------------------------------
// The shim and the hook.
// ---------------------------------------------------------------------------

#[test]
fn the_shim_is_executable() {
    // bin/on-event execs it and the keybinding runs it directly, so a lost
    // executable bit breaks both entry paths.
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(root().join("bin/agent-layout"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o100, 0, "bin/agent-layout is not executable");
}

#[test]
fn the_build_does_not_assume_cargo_is_on_the_path() {
    // Herdr's server runs under launchd with PATH=/usr/bin:/bin:/usr/sbin:/sbin,
    // and cargo is not on it. A build that only ran `cargo` would work in a
    // developer's shell and fail on every real event.
    let build = read("bin/build");
    assert!(
        build.contains("/opt/homebrew/opt/rustup/bin/cargo"),
        "the build must search absolute cargo locations"
    );
    assert!(
        build.contains(".cargo/bin/cargo"),
        "the build must search the rustup default location"
    );
}

#[test]
fn a_missing_toolchain_is_reported_rather_than_failing_silently() {
    // The README now states cargo as a requirement, so the message that fires when
    // it is absent has to name it and say what to do.
    let build = read("bin/build");
    assert!(build.contains("cargo not found"), "{}", build);
    assert!(build.contains("cargo build --release"), "{}", build);
}

#[test]
fn the_shim_puts_cargos_own_directory_on_the_path_it_builds_with() {
    // A real bug, found by a cold-start run under the launchd PATH. Locating cargo
    // by absolute path is not enough: `cargo` is a rustup shim that execs `rustc`
    // from its own directory, so a build under PATH=/usr/bin:/bin:/usr/sbin:/sbin
    // died with "could not execute process `rustc -vV` (never executed)".
    //
    // Tested behaviourally rather than by grep: a fake cargo records the PATH it was
    // handed, so this reddens if the export is dropped even when the line still
    // looks right.
    let dir = TempDir::new();
    let plugin_root = dir.join("plugin");
    let toolchain = dir.join("toolchain");
    std::fs::create_dir_all(plugin_root.join("src")).unwrap();
    std::fs::create_dir_all(&toolchain).unwrap();
    std::fs::write(plugin_root.join("Cargo.toml"), "").unwrap();
    std::fs::write(plugin_root.join("Cargo.lock"), "").unwrap();
    std::fs::write(plugin_root.join("src/main.rs"), "").unwrap();
    std::fs::create_dir_all(plugin_root.join("bin")).unwrap();
    std::fs::copy(
        root().join("bin/agent-layout"),
        plugin_root.join("agent-layout"),
    )
    .unwrap();
    // The shim delegates the build, so the real build script has to be there too.
    std::fs::copy(root().join("bin/build"), plugin_root.join("bin/build")).unwrap();

    let recorded = dir.join("seen-path");
    let fake_cargo = toolchain.join("cargo");
    std::fs::write(
        &fake_cargo,
        format!(
            "#!/bin/sh\nprintf '%s' \"$PATH\" > {}\n\
             mkdir -p {}/target/release\n\
             printf '#!/bin/sh\\nexit 0\\n' > {}/target/release/agent-layout\n\
             chmod 755 {}/target/release/agent-layout\n",
            recorded.display(),
            plugin_root.display(),
            plugin_root.display(),
            plugin_root.display()
        ),
    )
    .unwrap();
    set_executable(&fake_cargo);

    let status = std::process::Command::new(plugin_root.join("agent-layout"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", dir.path())
        .env("CARGO", &fake_cargo)
        .env("HERDR_PLUGIN_ROOT", &plugin_root)
        .status()
        .expect("cannot run the shim");
    assert!(status.success(), "the shim did not build and exec");

    let seen = std::fs::read_to_string(&recorded).expect("the fake cargo never ran");
    assert!(
        seen.split(':').any(|p| p == toolchain.to_str().unwrap()),
        "cargo's own directory is not on the build PATH: {}",
        seen
    );
}

fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A throwaway directory under /private/tmp, for the shim test above.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> TempDir {
        let path = PathBuf::from(format!(
            "/private/tmp/agent-layout-shim-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn the_hook_acts_on_one_event_and_passes_the_gate_flag() {
    // worktree.opened is subscribed so Herdr logs it while a design question is
    // settled, and must not reach the layout. The gate flag is what carries the
    // focused-workspace check, so dropping it would lay out an unfocused
    // workspace and start an agent in whatever the user is looking at.
    let hook = read("bin/on-event");
    assert!(hook.contains(r#"[ "${HERDR_PLUGIN_EVENT:-}" = "worktree.created" ] || exit 0"#));
    assert!(hook.contains("--from-event"));
}

#[test]
fn no_python_remains_anywhere_in_the_repository() {
    // The suite and both scripts were Python or shelled out to it. The whole point
    // of the rewrite is that the plugin is one Rust binary behind one sh shim, so a
    // stray .py file or a python3 call is a regression rather than a leftover.
    let mut offenders = Vec::new();
    walk(&root(), &mut |path| {
        let display = path.strip_prefix(root()).unwrap().display().to_string();
        if display.starts_with("target/") || display.starts_with(".git/") {
            return;
        }
        if display.ends_with(".py") || display.contains("__pycache__") {
            offenders.push(display.clone());
            return;
        }
        if matches!(
            display.as_str(),
            "bin/agent-layout" | "bin/on-event" | "herdr-plugin.toml"
        ) {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if text.contains("python") {
                offenders.push(format!("{} calls python", display));
            }
        }
    });
    assert!(offenders.is_empty(), "{:?}", offenders);
}

fn walk(dir: &Path, seen: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if name == "target" || name == ".git" {
                continue;
            }
            walk(&path, seen);
        } else {
            seen(&path);
        }
    }
}

// ---------------------------------------------------------------------------
// Documentation drift.
// ---------------------------------------------------------------------------

#[test]
fn the_readme_documents_the_config_file_by_name() {
    let readme = read("README.md");
    for needle in [
        "agent-layout.toml",
        "[[projects]]",
        "socket",
        "cargo build --release",
        "docs/configuration.md",
    ] {
        assert!(
            readme.contains(needle),
            "the README never mentions {}",
            needle
        );
    }
}

#[test]
fn no_document_still_tells_the_reader_to_set_a_removed_variable() {
    // The plugin's config directory file is dropped with no fallback, and every
    // AGENT_LAYOUT_* variable with it. A doc that still tells the reader to set one
    // sends them to edit something nothing reads, which is worse than saying
    // nothing.
    //
    // What is checked is an ASSIGNMENT, not a mention. Naming the removed variables
    // is exactly what an upgrade note has to do, and a blunt "never appears" rule
    // would forbid the one sentence a 0.2.x user most needs to read.
    for name in [
        "README.md",
        "docs/design.md",
        "docs/herdr-behaviour.md",
        "docs/open-questions.md",
        "docs/configuration.md",
    ] {
        let text = read(name);
        let offenders: Vec<&str> = text
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed.starts_with("AGENT_LAYOUT_") && trimmed.contains('=')
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "{} still tells the reader to set a removed variable: {:?}",
            name,
            offenders
        );
    }
}

#[test]
fn the_assignment_check_really_finds_an_assignment() {
    // A canary on the test above. Without it, a broken filter would pass on every
    // file whether or not the stale instructions were really gone.
    let planted = "some prose\nAGENT_LAYOUT_KIND=claude\nmore prose\n";
    let found: Vec<&str> = planted
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("AGENT_LAYOUT_") && trimmed.contains('=')
        })
        .collect();
    assert_eq!(found, vec!["AGENT_LAYOUT_KIND=claude"]);
}

#[test]
fn the_documentation_that_replaced_the_comments_exists() {
    // Deleting a comment is only safe because its content moved. A docs file going
    // missing must redden here rather than at the next reader.
    for name in [
        "docs/design.md",
        "docs/herdr-behaviour.md",
        "docs/open-questions.md",
        "docs/configuration.md",
    ] {
        let path = root().join(name);
        assert!(path.is_file(), "{} is missing", name);
        assert!(
            std::fs::metadata(&path).unwrap().len() > 1000,
            "{} is too short to hold what it replaced",
            name
        );
    }
}

#[test]
fn the_shim_the_hook_and_the_manifest_carry_no_comments() {
    // Not style policing. The rationale lives in docs/, and a comment creeping back
    // into a script is the first step to the two copies disagreeing. The Rust
    // sources are exempt: doc comments are documentation, and the previous suite
    // made the same exemption for its own docstrings.
    for name in ["bin/agent-layout", "bin/on-event", "herdr-plugin.toml"] {
        let offenders: Vec<(usize, String)> = read(name)
            .lines()
            .enumerate()
            .map(|(i, line)| (i + 1, line.to_string()))
            .filter(|(n, line)| {
                line.trim_start().starts_with('#') && !(*n == 1 && line.starts_with("#!"))
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "{} carries comments; move the content into docs/: {:?}",
            name,
            offenders
        );
    }
}

#[test]
fn the_comment_check_really_finds_a_comment() {
    // A canary on the test above, which would pass just as happily if the filter
    // were broken. It also pins the two exemptions: a line-1 shebang is not a
    // comment, and an indented comment is.
    let planted = "#!/bin/sh\ntrue\n    # an indented comment\n# a bare one\n";
    let found: Vec<usize> = planted
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(n, line)| {
            line.trim_start().starts_with('#') && !(*n == 1 && line.starts_with("#!"))
        })
        .map(|(n, _)| n)
        .collect();
    assert_eq!(found, vec![3, 4]);
}

#[test]
fn the_gitignore_keeps_the_build_directory_out_of_the_repository() {
    let ignore = read(".gitignore");
    assert!(ignore.lines().any(|l| l.trim() == "/target"));
}
