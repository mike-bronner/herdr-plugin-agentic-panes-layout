use std::process::Command;

use agent_layout::api::{self, Client};
use agent_layout::{config, confirm, layout};

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
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        workspace: None,
        agent_name: None,
        from_event: false,
        confirm: false,
        rebuild: false,
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

    // The popup pane runs this same binary. It talks to a terminal and a file, not
    // to the socket, so it returns before anything else is resolved.
    if args.confirm {
        return confirm::run_popup().map_err(Exit);
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
    let candidates = project_candidates(&target, &cwd);
    let chosen = loaded.choose(&candidates);

    for note in &loaded.diagnostics {
        report(Some(&client), note);
    }
    if let Some(note) = &chosen.diagnostic {
        report(Some(&client), &format!("{}: {}", target.label, note));
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

fn project_candidates(workspace: &api::Workspace, cwd: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    for path in [&workspace.checkout_path, &workspace.repo_root]
        .into_iter()
        .flatten()
    {
        candidates.push(path.clone());
    }
    if let Some(toplevel) = git(cwd, &["rev-parse", "--show-toplevel"]) {
        candidates.push(toplevel);
    }
    if let Some(common) = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        let root = common
            .strip_suffix("/.git")
            .map(|s| s.to_string())
            .unwrap_or(common);
        candidates.push(root);
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn report(client: Option<&Client>, message: &str) {
    eprintln!("agent-layout: {}", message);
    if let Some(client) = client {
        api::notify(client, message);
    }
}
