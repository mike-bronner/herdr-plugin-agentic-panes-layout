use crate::api::{self, Client, Pane as LivePane};
use crate::config::{Layout, Tab};
use crate::name;

pub struct Target {
    pub workspace_id: String,
    pub label: String,
    pub active_tab_id: String,
    pub active_panes: Vec<LivePane>,
}

pub struct Outcome {
    pub notes: Vec<String>,
    pub built: Vec<String>,
    pub skipped: Vec<String>,
}

pub struct Fatal(pub String);

impl Outcome {
    fn new() -> Outcome {
        Outcome {
            notes: Vec::new(),
            built: Vec::new(),
            skipped: Vec::new(),
        }
    }
}

enum Root {
    TookOver(String),
    Created(String),
}

impl Root {
    fn pane_id(&self) -> &str {
        match self {
            Root::TookOver(id) | Root::Created(id) => id,
        }
    }
}

pub fn apply(
    client: &Client,
    target: &Target,
    layout: &Layout,
    supplied_name: Option<&str>,
    cwd: &str,
) -> Result<Outcome, Fatal> {
    let mut outcome = Outcome::new();
    let mut existing = api::tab_labels(client, &target.workspace_id)
        .map_err(|e| Fatal(format!("{}: cannot list the tabs: {}", target.label, e)))?;
    let mut supplied_name = supplied_name;
    let mut active_taken = false;

    for (index, tab) in layout.tabs.iter().enumerate() {
        if existing.iter().any(|label| label == &tab.name) {
            outcome.skipped.push(tab.name.clone());
            continue;
        }

        let root = open_tab(client, target, tab, index == 0 && !active_taken, cwd)?;
        if matches!(root, Root::TookOver(_)) {
            active_taken = true;
        }
        existing.push(tab.name.clone());

        let pane_ids = split_panes(client, target, tab, root.pane_id(), cwd)?;
        run_commands(client, target, tab, &pane_ids, &mut outcome);
        write_labels(client, target, tab, &pane_ids, &mut outcome);
        start_agents(
            client,
            target,
            tab,
            &pane_ids,
            &mut supplied_name,
            &mut outcome,
        )?;
        outcome.built.push(tab.name.clone());
    }

    Ok(outcome)
}

fn open_tab(
    client: &Client,
    target: &Target,
    tab: &Tab,
    may_take_over: bool,
    cwd: &str,
) -> Result<Root, Fatal> {
    if may_take_over && active_tab_is_free(target) {
        let pane_id = target.active_panes[0].pane_id.clone();
        api::tab_rename(client, &target.active_tab_id, &tab.name).map_err(|e| {
            Fatal(format!(
                "{}: could not rename the tab to \"{}\": {}",
                target.label, tab.name, e
            ))
        })?;
        return Ok(Root::TookOver(pane_id));
    }

    api::tab_create(client, &target.workspace_id, cwd, &tab.name)
        .map(Root::Created)
        .map_err(|e| {
            Fatal(format!(
                "{}: could not create the tab \"{}\": {}",
                target.label, tab.name, e
            ))
        })
}

fn active_tab_is_free(target: &Target) -> bool {
    target.active_panes.len() == 1 && target.active_panes[0].agent.is_none()
}

fn split_panes(
    client: &Client,
    target: &Target,
    tab: &Tab,
    root_pane_id: &str,
    cwd: &str,
) -> Result<Vec<String>, Fatal> {
    let mut pane_ids = vec![root_pane_id.to_string()];
    for pane in tab.panes.iter().skip(1) {
        let direction = pane.split.ok_or_else(|| {
            Fatal(format!(
                "{}: tab \"{}\" carries a pane with no split direction",
                target.label, tab.name
            ))
        })?;
        let previous = pane_ids
            .last()
            .expect("the root pane is always present")
            .clone();
        let made = api::pane_split(client, &previous, direction.as_str(), pane.ratio, cwd)
            .map_err(|e| {
                Fatal(format!(
                    "{}: could not split {} in tab \"{}\" ({}, ratio {}): {}",
                    target.label,
                    previous,
                    tab.name,
                    direction.as_str(),
                    describe_ratio(pane.ratio),
                    e
                ))
            })?;
        pane_ids.push(made);
    }
    Ok(pane_ids)
}

fn describe_ratio(ratio: Option<f64>) -> String {
    match ratio {
        Some(r) => r.to_string(),
        None => "unset".to_string(),
    }
}

fn run_commands(
    client: &Client,
    target: &Target,
    tab: &Tab,
    pane_ids: &[String],
    outcome: &mut Outcome,
) {
    for (pane, pane_id) in tab.panes.iter().zip(pane_ids) {
        if let Some(command) = pane.command.as_deref() {
            if let Err(e) = api::pane_run(client, pane_id, command) {
                outcome.notes.push(format!(
                    "{}: panes are laid out, but \"{}\" would not run in {}: {}",
                    target.label, command, pane_id, e
                ));
            }
        }
    }
}

fn write_labels(
    client: &Client,
    target: &Target,
    tab: &Tab,
    pane_ids: &[String],
    outcome: &mut Outcome,
) {
    for (pane, pane_id) in tab.panes.iter().zip(pane_ids) {
        if let Some(label) = pane.label.as_deref() {
            if let Err(e) = api::pane_rename(client, pane_id, label) {
                outcome.notes.push(format!(
                    "{}: could not label pane {} \"{}\": {}",
                    target.label, pane_id, label, e
                ));
            }
        }
    }
}

fn start_agents(
    client: &Client,
    target: &Target,
    tab: &Tab,
    pane_ids: &[String],
    supplied_name: &mut Option<&str>,
    outcome: &mut Outcome,
) -> Result<(), Fatal> {
    for (pane, pane_id) in tab.panes.iter().zip(pane_ids) {
        let kind = match pane.agent.as_deref() {
            Some(kind) => kind,
            None => continue,
        };
        match supplied_name.take() {
            Some(exact) => start_exact(client, target, kind, pane_id, exact, outcome)?,
            None => start_derived(client, target, kind, pane_id, outcome)?,
        }
    }
    Ok(())
}

/// What one `agent.start` answer means.
///
/// The classification lives here, once, rather than in each caller. It used to be
/// written out in both the supplied-name and derived-name paths, and a mutation
/// test found the consequence: `agent_not_ready` was covered on one path and not
/// the other, because the duplicate arm had no test of its own.
enum Started {
    /// The agent is running, under this name.
    Yes,
    /// The agent started and is waiting at a prompt. Herdr documents
    /// `agent_not_ready` as blocked during startup with its name still usable, so
    /// the agent DID start. On a fresh worktree this is Claude Code's
    /// trust-this-folder prompt, which is the normal case.
    YesAtAPrompt,
    /// The name is taken. Only a derived name may retry.
    NameTaken(String),
    /// Anything else, including `agent_pane_busy` and `timeout`. The default arm
    /// is fatal rather than a list, so a code Herdr adds later is reported instead
    /// of being silently swallowed.
    No(String),
}

fn classify(result: Result<(), api::CallError>) -> Started {
    match result {
        Ok(()) => Started::Yes,
        Err(e) if e.code() == Some(name::NOT_READY) => Started::YesAtAPrompt,
        Err(e) if e.code() == Some(name::TAKEN) => Started::NameTaken(e.to_string()),
        Err(e) => Started::No(e.to_string()),
    }
}

fn started_note(kind: &str, agent_name: &str) -> String {
    format!("{} started as \"{}\"", kind, agent_name)
}

fn prompt_note(kind: &str, agent_name: &str) -> String {
    format!(
        "{} started as \"{}\" and is waiting on a prompt",
        kind, agent_name
    )
}

fn start_exact(
    client: &Client,
    target: &Target,
    kind: &str,
    pane_id: &str,
    exact: &str,
    outcome: &mut Outcome,
) -> Result<(), Fatal> {
    match classify(api::agent_start(client, exact, kind, pane_id)) {
        Started::Yes => {
            outcome.notes.push(started_note(kind, exact));
            Ok(())
        }
        Started::YesAtAPrompt => {
            outcome.notes.push(prompt_note(kind, exact));
            Ok(())
        }
        Started::NameTaken(why) => Err(Fatal(format!(
            "{}: the agent name \"{}\" given with --agent-name is already taken: {}",
            target.label, exact, why
        ))),
        Started::No(why) => Err(Fatal(format!(
            "{}: panes laid out, but {} did not start as \"{}\": {}",
            target.label, kind, exact, why
        ))),
    }
}

fn start_derived(
    client: &Client,
    target: &Target,
    kind: &str,
    pane_id: &str,
    outcome: &mut Outcome,
) -> Result<(), Fatal> {
    let base = name::derive(&target.label);
    for attempt in 1..=name::MAX_TRIES {
        let candidate = name::with_suffix(&base, attempt);
        match classify(api::agent_start(client, &candidate, kind, pane_id)) {
            Started::Yes => {
                outcome.notes.push(started_note(kind, &candidate));
                return Ok(());
            }
            Started::YesAtAPrompt => {
                outcome.notes.push(prompt_note(kind, &candidate));
                return Ok(());
            }
            Started::NameTaken(why) => {
                if attempt == name::MAX_TRIES {
                    return Err(Fatal(format!(
                        "{}: every agent name from \"{}\" to \"{}\" is already taken: {}",
                        target.label, base, candidate, why
                    )));
                }
            }
            Started::No(why) => {
                return Err(Fatal(format!(
                    "{}: panes laid out, but {} did not start as \"{}\": {}",
                    target.label, kind, candidate, why
                )))
            }
        }
    }
    unreachable!("the loop returns on the last attempt")
}
