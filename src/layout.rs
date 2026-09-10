use crate::api::{self, Client, Pane as LivePane};
use crate::config::{Layout, Tab};
use crate::confirm::{self, Answer};
use crate::name;

pub struct Target {
    pub workspace_id: String,
    pub label: String,
    pub active_tab_id: String,
    /// Every pane of the workspace, not only the active tab's, because a rebuild
    /// has to find the panes of a tab it did not create.
    pub panes: Vec<LivePane>,
    pub tabs: Vec<api::TabInfo>,
}

impl Target {
    fn active_panes(&self) -> Vec<&LivePane> {
        self.panes
            .iter()
            .filter(|p| p.tab_id == self.active_tab_id)
            .collect()
    }

    fn panes_of(&self, tab_id: &str) -> Vec<&LivePane> {
        self.panes.iter().filter(|p| p.tab_id == tab_id).collect()
    }

    fn tab_named(&self, name: &str) -> Option<&api::TabInfo> {
        self.tabs.iter().find(|t| t.label == name)
    }
}

/// What an existing tab of the layout's name means.
///
/// The automatic `worktree.created` path skips it, because a second event must not
/// rebuild a workspace nobody asked to rebuild. An explicit invocation rebuilds it,
/// because asking for a layout by pressing a key is a statement of intent that a
/// guard should not silently ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnExisting {
    Skip,
    Rebuild,
}

pub struct Outcome {
    pub notes: Vec<String>,
    pub built: Vec<String>,
    pub skipped: Vec<String>,
    pub rebuilt: Vec<String>,
}

pub struct Fatal(pub String);

impl Outcome {
    fn new() -> Outcome {
        Outcome {
            notes: Vec::new(),
            built: Vec::new(),
            skipped: Vec::new(),
            rebuilt: Vec::new(),
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
    on_existing: OnExisting,
) -> Result<Outcome, Fatal> {
    let mut outcome = Outcome::new();
    let mut existing: Vec<String> = target.tabs.iter().map(|t| t.label.clone()).collect();
    let mut supplied_name = supplied_name;
    let mut active_taken = false;

    for (index, tab) in layout.tabs.iter().enumerate() {
        let already_there = existing.iter().any(|label| label == &tab.name);

        let root = if already_there {
            match on_existing {
                OnExisting::Skip => {
                    outcome.skipped.push(tab.name.clone());
                    continue;
                }
                OnExisting::Rebuild => match clear_tab(client, target, tab, &mut outcome)? {
                    Some(survivor) => {
                        outcome.rebuilt.push(tab.name.clone());
                        survivor
                    }
                    None => {
                        outcome.skipped.push(tab.name.clone());
                        continue;
                    }
                },
            }
        } else {
            let opened = open_tab(client, target, tab, index == 0 && !active_taken, cwd)?;
            if matches!(opened, Root::TookOver(_)) {
                active_taken = true;
            }
            existing.push(tab.name.clone());
            outcome.built.push(tab.name.clone());
            Survivor {
                pane_id: opened.pane_id().to_string(),
                already_runs_an_agent: false,
            }
        };

        let pane_ids = split_panes(client, target, tab, &root.pane_id, cwd)?;
        run_commands(
            client,
            target,
            tab,
            &pane_ids,
            root.already_runs_an_agent,
            &mut outcome,
        );
        write_labels(client, target, tab, &pane_ids, &mut outcome);
        start_agents(
            client,
            target,
            tab,
            &pane_ids,
            root.already_runs_an_agent,
            &mut supplied_name,
            &mut outcome,
        )?;
    }

    Ok(outcome)
}

/// The pane a rebuilt tab is rebuilt from.
struct Survivor {
    pane_id: String,
    /// True when the survivor is a pane the user declined to close, so it is still
    /// running its agent and must not be given another one or have a command typed
    /// into it.
    already_runs_an_agent: bool,
}

/// Empty an existing tab down to one pane, ready to be rebuilt from.
///
/// Returns `None` when the rebuild must be abandoned, which happens only when the
/// tab has no panes to work from at all.
///
/// A pane running an agent is never closed without an affirmative answer. On a
/// refusal the agent's pane becomes the survivor: the tab is rebuilt around it and
/// it keeps running, which is what "work around it" means. Every other pane of the
/// tab is closed either way, because those hold no unrecoverable work.
fn clear_tab(
    client: &Client,
    target: &Target,
    tab: &Tab,
    outcome: &mut Outcome,
) -> Result<Option<Survivor>, Fatal> {
    let tab_id = match target.tab_named(&tab.name) {
        Some(info) => info.tab_id.clone(),
        None => return Ok(None),
    };
    let panes = target.panes_of(&tab_id);
    if panes.is_empty() {
        outcome.notes.push(format!(
            "tab \"{}\" reports no panes, so it was left alone",
            tab.name
        ));
        return Ok(None);
    }

    let with_agents: Vec<&LivePane> = panes
        .iter()
        .copied()
        .filter(|p| p.agent.is_some())
        .collect();

    // Nothing is at risk, so there is nothing to consent to. Asking here would stall
    // the common case behind a popup for no reason.
    let may_close_agents = if with_agents.is_empty() {
        true
    } else {
        let (answer, note) = confirm::ask(client, &question(&tab.name, &with_agents));
        if let Some(note) = note {
            outcome
                .notes
                .push(format!("tab \"{}\": {}", tab.name, note));
        }
        match answer {
            Answer::CloseIt => true,
            Answer::KeepIt => false,
            // The third choice, and every failure to ask or be answered. A misfire
            // must cost nothing, so not one pane of this tab is touched.
            Answer::DoNothing => return Ok(None),
        }
    };

    let survivor = if may_close_agents {
        panes[0]
    } else {
        with_agents[0]
    };

    for pane in &panes {
        if pane.pane_id == survivor.pane_id {
            continue;
        }
        if pane.agent.is_some() && !may_close_agents {
            outcome.notes.push(format!(
                "left pane {} running {} in tab \"{}\", unlabelled by the layout",
                pane.pane_id,
                pane.agent.as_deref().unwrap_or("an agent"),
                tab.name
            ));
            continue;
        }
        if let Err(e) = confirm::close_pane(client, &pane.pane_id) {
            outcome.notes.push(format!(
                "could not close pane {} while rebuilding \"{}\": {}",
                pane.pane_id, tab.name, e
            ));
        }
    }

    if !may_close_agents {
        outcome.notes.push(format!(
            "kept {} running in {} and rebuilt tab \"{}\" around it",
            survivor.agent.as_deref().unwrap_or("the agent"),
            survivor.pane_id,
            tab.name
        ));
    }

    Ok(Some(Survivor {
        pane_id: survivor.pane_id.clone(),
        already_runs_an_agent: !may_close_agents,
    }))
}

fn question(tab_name: &str, with_agents: &[&LivePane]) -> String {
    if with_agents.len() == 1 {
        format!(
            "Rebuilding tab \"{}\" would close pane {}, which is running {}.\n\
             Its work cannot be recovered. Close it?",
            tab_name,
            with_agents[0].pane_id,
            with_agents[0].agent.as_deref().unwrap_or("an agent")
        )
    } else {
        format!(
            "Rebuilding tab \"{}\" would close {} panes running agents ({}).\n\
             Their work cannot be recovered. Close them?",
            tab_name,
            with_agents.len(),
            with_agents
                .iter()
                .map(|p| p.pane_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn open_tab(
    client: &Client,
    target: &Target,
    tab: &Tab,
    may_take_over: bool,
    cwd: &str,
) -> Result<Root, Fatal> {
    if may_take_over && active_tab_is_free(target) {
        let pane_id = target.active_panes()[0].pane_id.clone();
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
    let active = target.active_panes();
    active.len() == 1 && active[0].agent.is_none()
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
    first_pane_is_busy: bool,
    outcome: &mut Outcome,
) {
    for (index, (pane, pane_id)) in tab.panes.iter().zip(pane_ids).enumerate() {
        // Typing a command into a pane whose agent the user just asked to keep
        // would send the text to the agent as a prompt.
        if index == 0 && first_pane_is_busy {
            if pane.command.is_some() {
                outcome.notes.push(format!(
                    "did not run the command in {}, which is still running an agent",
                    pane_id
                ));
            }
            continue;
        }
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
    first_pane_is_busy: bool,
    supplied_name: &mut Option<&str>,
    outcome: &mut Outcome,
) -> Result<(), Fatal> {
    for (index, (pane, pane_id)) in tab.panes.iter().zip(pane_ids).enumerate() {
        let kind = match pane.agent.as_deref() {
            Some(kind) => kind,
            None => continue,
        };
        // The user declined to close this pane, so its agent stays. Starting a
        // second one in it would be the very thing they refused.
        if index == 0 && first_pane_is_busy {
            outcome.notes.push(format!(
                "{} already runs an agent, so no {} was started in it",
                pane_id, kind
            ));
            continue;
        }
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
    /// The pane has not reached an idle prompt yet. **Transient**, so it is waited
    /// out rather than reported, up to a bound.
    PaneBusy(String),
    /// Anything else, including `timeout`. The default arm is fatal rather than a
    /// list, so a code Herdr adds later is reported instead of being silently
    /// swallowed.
    No(String),
}

fn classify(result: Result<(), api::CallError>) -> Started {
    match result {
        Ok(()) => Started::Yes,
        Err(e) if e.code() == Some(name::NOT_READY) => Started::YesAtAPrompt,
        Err(e) if e.code() == Some(name::TAKEN) => Started::NameTaken(e.to_string()),
        Err(e) if e.code() == Some(name::PANE_BUSY) => Started::PaneBusy(e.to_string()),
        Err(e) => Started::No(e.to_string()),
    }
}

/// Start an agent, waiting out a pane that has not reached its prompt yet.
///
/// **The two retries are different in kind, which is why they are separate loops.** A
/// taken name is permanent until something changes, so that retry alters the input: a
/// different name, tried at once, because waiting would not help. A busy pane is
/// usually transient, so this retry alters nothing and simply waits, because a
/// different name would not help either.
///
/// `PaneBusy` is returned only once the bound is exhausted, so the caller's name loop
/// never sees a transient one.
fn start_waiting_for_the_shell(
    client: &Client,
    agent_name: &str,
    kind: &str,
    pane_id: &str,
    outcome: &mut Outcome,
) -> Started {
    let mut waited_ms = 0;
    for attempt in 1..=name::BUSY_MAX_TRIES {
        match classify(api::agent_start(client, agent_name, kind, pane_id)) {
            Started::PaneBusy(why) => {
                if attempt == name::BUSY_MAX_TRIES {
                    return Started::PaneBusy(why);
                }
                std::thread::sleep(std::time::Duration::from_millis(name::BUSY_WAIT_MS));
                waited_ms += name::BUSY_WAIT_MS;
            }
            settled => {
                // Worth saying. A pause with no explanation reads as a hang, and this
                // is the failure the retry exists to fix.
                if waited_ms > 0 {
                    outcome.notes.push(format!(
                        "{} was not at a prompt yet, so starting {} waited {}ms",
                        pane_id, kind, waited_ms
                    ));
                }
                return settled;
            }
        }
    }
    unreachable!("the loop returns on the last attempt")
}

/// The message when a pane never reached a prompt.
///
/// Deliberately different from the other failures. A pane still busy after the whole
/// budget is not a slow shell: something is running in it, and telling the reader that
/// is more useful than repeating that the agent did not start. Herdr's own wording is
/// carried through, because a silent give-up is the invisibility this plugin has spent
/// its recent history removing.
fn busy_message(
    target: &Target,
    kind: &str,
    agent_name: &str,
    pane_id: &str,
    why: String,
) -> String {
    format!(
        "{}: panes laid out, but {} did not start as \"{}\". Pane {} was still busy after \
         {}ms, so something is running in it rather than the shell being slow. Press the \
         layout keybinding once it is free. Herdr said: {}",
        target.label,
        kind,
        agent_name,
        pane_id,
        name::BUSY_WAIT_MS * u64::from(name::BUSY_MAX_TRIES - 1),
        why
    )
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
    match start_waiting_for_the_shell(client, exact, kind, pane_id, outcome) {
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
        Started::PaneBusy(why) => Err(Fatal(busy_message(target, kind, exact, pane_id, why))),
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
        match start_waiting_for_the_shell(client, &candidate, kind, pane_id, outcome) {
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
            Started::PaneBusy(why) => {
                return Err(Fatal(busy_message(target, kind, &candidate, pane_id, why)))
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
