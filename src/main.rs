use agent_layout::api::{self, Client};
use agent_layout::{check, config, confirm, issues, layout, project};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(Exit(message)) => {
            report(Client::from_env().ok().as_ref(), &message);
            1
        }
    };
    std::process::exit(code);
}

struct Exit(String);

pub const EVENT_JSON_VAR: &str = "HERDR_PLUGIN_EVENT_JSON";

struct Args {
    workspace: Option<String>,
    agent_name: Option<String>,
    from_event: bool,
    confirm: bool,
    rebuild: bool,
    check: bool,
    issues: bool,
    version: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        workspace: None,
        agent_name: None,
        from_event: false,
        confirm: false,
        rebuild: false,
        check: false,
        issues: false,
        version: false,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        match flag.as_str() {
            "--from-event" => {
                if args.from_event {
                    return Err("--from-event given more than once".to_string());
                }
                args.from_event = true;
            }
            "--confirm" => {
                if args.confirm {
                    return Err("--confirm given more than once".to_string());
                }
                args.confirm = true;
            }
            "--rebuild" => {
                if args.rebuild {
                    return Err("--rebuild given more than once".to_string());
                }
                args.rebuild = true;
            }
            "--check" => {
                if args.check {
                    return Err("--check given more than once".to_string());
                }
                args.check = true;
            }
            "--issues" => {
                if args.issues {
                    return Err("--issues given more than once".to_string());
                }
                args.issues = true;
            }
            "--version" | "-V" => {
                if args.version {
                    return Err("--version given more than once".to_string());
                }
                args.version = true;
            }
            "--workspace" => {
                if args.workspace.is_some() {
                    return Err("--workspace given more than once".to_string());
                }
                args.workspace = Some(value(raw.next(), "--workspace needs a workspace id")?);
            }
            "--agent-name" => {
                if args.agent_name.is_some() {
                    return Err("--agent-name given more than once".to_string());
                }
                args.agent_name = Some(value(raw.next(), "--agent-name needs an agent name")?);
            }
            other => return Err(format!("unknown argument: {}", other)),
        }
    }
    Ok(args)
}

fn value(given: Option<String>, complaint: &str) -> Result<String, String> {
    match given {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(complaint.to_string()),
    }
}

/// What this binary is, and whether it matches the manifest Herdr reads.
///
/// The crate version alone would not have caught the failure this exists for: a binary
/// two commits behind its source, where the stale artifact and the current manifest both
/// read 0.3.0, because the version only moves on a release commit. **The commit is what
/// distinguishes them within a release**, and the build time says at a glance whether it
/// predates the last edit.
///
/// The manifest version is reported too, and it is a genuinely different fact.
/// `CARGO_PKG_VERSION` is baked in at **compile** time; the manifest is read from disk at
/// **run** time, and it is what Herdr itself reads to decide what this plugin is. So one
/// is "what you are running" and the other is "what Herdr thinks you have". After a
/// release commit, a stale binary reports the old number against a manifest holding the
/// new one, and that is caught **with no git involved** — which matters because the
/// commit is exactly what goes missing when git does.
///
/// **Nothing here can fail.** Every lookup degrades to a word. A `--version` that errored
/// because it could not find its own manifest would be worse than one that says less, and
/// this is the command that has to work when everything else is broken.
fn version_line() -> String {
    let crate_version = env!("CARGO_PKG_VERSION");
    let mut lines = vec![format!(
        "agent-layout {} ({}, built {})",
        crate_version,
        env!("AGENT_LAYOUT_COMMIT"),
        env!("AGENT_LAYOUT_BUILT_AT")
    )];

    match manifest_version() {
        Ok((version, path)) => {
            lines.push(format!("manifest {} at {}", version, path.display()));
            // Said outright rather than left as two numbers to compare. "0.3.0 / 0.3.1"
            // with no explanation invites a bug report instead of a diagnosis.
            if version != crate_version {
                lines.push(format!(
                    "STALE: this binary is {} but the manifest is {}. \
                     Rebuild it with `cargo build --release`.",
                    crate_version, version
                ));
            }
        }
        Err(why) => lines.push(why),
    }

    lines.join("\n")
}

/// The `version` key from `herdr-plugin.toml`, and where it was read from.
///
/// Resolved from the executable's own location rather than only from
/// `HERDR_PLUGIN_ROOT`, because `--version` is typed by hand far more often than it is
/// invoked by Herdr, and by hand that variable is not set.
///
/// **Each failure says which one it is.** This used to return an `Option`, so four
/// different situations printed one message: "manifest unknown". That tells somebody
/// troubleshooting nothing about which of them they are in, and troubleshooting is the
/// use Mike endorsed this line for. The `not found` case also names the fix, which is
/// the property that makes the `STALE` line worth having.
///
/// Still cannot fail. Every path returns a sentence rather than an error.
fn manifest_version() -> Result<(String, std::path::PathBuf), String> {
    let root = match std::env::var_os("HERDR_PLUGIN_ROOT") {
        Some(root) if !root.is_empty() => std::path::PathBuf::from(root),
        // <root>/target/<profile>/agent-layout
        _ => std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.parent()?.parent()?.to_path_buf()))
            .ok_or_else(|| {
                "manifest not found: set HERDR_PLUGIN_ROOT to the plugin checkout to read it"
                    .to_string()
            })?,
    };

    let path = root.join("herdr-plugin.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("manifest unreadable at {}", path.display()))?;
    let parsed = text
        .parse::<toml::Table>()
        .map_err(|_| format!("manifest unparsed at {}", path.display()))?;
    let version = parsed
        .get("version")
        .and_then(|v| v.as_str())
        // The fifth case, which the sibling plugin's three strings do not cover: a
        // manifest that reads perfectly well and simply has no version in it.
        .ok_or_else(|| format!("manifest has no version key at {}", path.display()))?;

    Ok((version.to_string(), path))
}

fn event_workspace_is_focused() -> bool {
    let raw = match std::env::var(EVENT_JSON_VAR) {
        Ok(raw) if !raw.is_empty() => raw,
        _ => return false,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    parsed
        .get("data")
        .and_then(|d| d.get("workspace"))
        .and_then(|w| w.get("focused"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn run() -> Result<(), Exit> {
    let args = parse_args().map_err(Exit)?;

    // First of all, because the question it answers is "what is this?" and that must be
    // answerable when everything else is broken. It reads nothing and reaches nothing.
    if args.version {
        println!("{}", version_line());
        return Ok(());
    }

    // The popup pane runs this same binary. It talks to a terminal and a file, not
    // to the socket, so it returns before anything else is resolved.
    if args.confirm {
        return confirm::run_popup().map_err(Exit);
    }

    // The issues popup is this same binary too, and like the confirmation it talks to a
    // terminal and a file rather than to the socket.
    if args.issues {
        return issues::run_popup().map_err(Exit);
    }

    // Before the socket, deliberately. A real run resolves the workspace first, so it
    // cannot reach config parsing without a server; the whole point of --check is that
    // it can, and that it touches nothing while doing it.
    if args.check {
        std::process::exit(check::run(&check::current_dir()));
    }

    if args.from_event && !event_workspace_is_focused() {
        return Ok(());
    }

    // Only the automatic path wants the guard. `--rebuild` is the explicit way to ask
    // for the other behaviour and is what the manifest's `[[actions]]` entry passes,
    // so the manifest reads as what it does. It is also the default for a bare
    // invocation, which is what keeps the README's documented `[[keys.command]]`
    // shell binding working without an edit.
    let on_existing = if args.from_event && !args.rebuild {
        layout::OnExisting::Skip
    } else {
        layout::OnExisting::Rebuild
    };

    let client = Client::from_env().map_err(|e| Exit(format!("cannot reach Herdr: {}", e)))?;

    let workspaces =
        api::workspaces(&client).map_err(|e| Exit(format!("cannot list the workspaces: {}", e)))?;

    let target = match args.workspace.as_deref() {
        Some(wanted) => workspaces
            .iter()
            .find(|w| w.workspace_id == wanted)
            .cloned()
            .ok_or_else(|| Exit(format!("no workspace \"{}\" in the Herdr server", wanted)))?,
        None => workspaces
            .iter()
            .find(|w| w.focused)
            .cloned()
            .ok_or_else(|| {
                Exit("cannot read a focused workspace from the Herdr server".to_string())
            })?,
    };

    let all_panes = api::panes(&client, &target.workspace_id)
        .map_err(|e| Exit(format!("{}: cannot list the panes: {}", target.label, e)))?;
    let tabs = api::tabs(&client, &target.workspace_id)
        .map_err(|e| Exit(format!("{}: cannot list the tabs: {}", target.label, e)))?;

    let cwd = all_panes
        .iter()
        .filter(|p| p.tab_id == target.active_tab_id)
        .find_map(|p| p.cwd.clone())
        .ok_or_else(|| {
            Exit(format!(
                "{}: no pane reports a working directory",
                target.label
            ))
        })?;

    let loaded = config::load();
    let candidates = project::candidates(
        target.checkout_path.as_deref(),
        target.repo_root.as_deref(),
        &cwd,
    );
    let chosen = loaded.choose(&candidates);

    // Every diagnostic reaches stderr, which is what `herdr plugin log list` keeps and
    // the only record that survives when nothing renders.
    //
    // Only ONE toast, though, and not one per diagnostic. Measured on 0.9.0: a second
    // toast answers Busy, there is a rate limit, and under Mike's own
    // `ui.toast.delivery = "system"` every one of them answers shown=false anyway. The
    // old loop could therefore never have shown more than its first item. The popup
    // below carries the detail; this line is a nudge for somebody on delivery = "herdr".
    let mut problems: Vec<String> = loaded.diagnostics.clone();
    if let Some(note) = &chosen.diagnostic {
        problems.push(format!("{}: {}", target.label, note));
    }
    for note in &problems {
        eprintln!("agent-layout: {}", note);
    }
    if !problems.is_empty() {
        api::notify(
            &client,
            &format!(
                "{}: {} problem{} in your config; see the popup",
                target.label,
                problems.len(),
                if problems.len() == 1 { "" } else { "s" }
            ),
        );
    }

    let plan = layout::Target {
        workspace_id: target.workspace_id.clone(),
        label: target.label.clone(),
        active_tab_id: target.active_tab_id.clone(),
        panes: all_panes,
        tabs,
    };

    let outcome = layout::apply(
        &client,
        &plan,
        &chosen.layout,
        args.agent_name.as_deref(),
        &cwd,
        on_existing,
    )
    .map_err(|layout::Fatal(message)| Exit(message))?;

    for note in &outcome.notes {
        report(Some(&client), &format!("{}: {}", target.label, note));
    }
    report(
        Some(&client),
        &summary(&target.label, &loaded, &chosen, &outcome),
    );

    // AFTER the layout, deliberately, and without waiting for it. A config problem is a
    // cosmetic warning, and gating pane creation on a dialog would turn it into a stall.
    // By here every pane exists and the agent has started.
    issues::show(
        &client,
        &match &loaded.source {
            Some(path) => path.display().to_string(),
            None => config::config_path().display().to_string(),
        },
        &problems,
    );
    Ok(())
}

fn summary(
    workspace_label: &str,
    loaded: &config::Loaded,
    chosen: &config::Chosen,
    outcome: &layout::Outcome,
) -> String {
    let origin = match (&loaded.source, &chosen.rule_path) {
        (Some(path), Some(rule)) => format!("{} for {}", path.display(), rule),
        (Some(path), None) => format!("{}, the default", path.display()),
        (None, _) => "built in, no config file".to_string(),
    };

    let mut parts = Vec::new();
    if !outcome.built.is_empty() {
        parts.push(format!("laid out {}", outcome.built.join(", ")));
    }
    if !outcome.rebuilt.is_empty() {
        parts.push(format!("rebuilt {}", outcome.rebuilt.join(", ")));
    }
    if !outcome.skipped.is_empty() {
        parts.push(format!(
            "left {} alone, already there",
            outcome.skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        parts.push("nothing to do".to_string());
    }
    format!(
        "{}: layout \"{}\" ({}) — {}",
        workspace_label,
        chosen.name,
        origin,
        parts.join("; ")
    )
}

fn report(client: Option<&Client>, message: &str) {
    eprintln!("agent-layout: {}", message);
    if let Some(client) = client {
        api::notify(client, message);
    }
}
