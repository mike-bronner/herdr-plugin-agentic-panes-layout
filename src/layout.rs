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
    /// Left alone by the tab-name guard on the event path.
    pub skipped: Vec<String>,
    pub rebuilt: Vec<String>,
    /// Left alone because the confirmation was not affirmative.
    ///
    /// Separate from `skipped` because the two have nothing in common but the outcome.
    /// Reporting a cancelled rebuild as "already there" would name the guard as the
    /// reason and hide the user's own answer.
    pub declined: Vec<String>,
}

pub struct Fatal(pub String);

impl Outcome {
    fn new() -> Outcome {
        Outcome {
            notes: Vec::new(),
            built: Vec::new(),
            skipped: Vec::new(),
            rebuilt: Vec::new(),
            declined: Vec::new(),
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

        // WHICH TAB THIS ACTS ON. **Mike's decision, in his words: only destroy the tabs
        // named in the layout.** Taken after being shown the consequence of a wider
        // scope, which is that one keypress would destroy an unrelated tab.
        //
        // So a one-tab layout touches one tab, a three-tab layout replaces those three,
        // and a tab the layout does not name survives either way. This is a chosen
        // boundary rather than a conservative default somebody settled on, and
        // `panes_of_another_tab_are_not_destroyed_by_a_rebuild` pins it.
        let target_tab = if already_there {
            match on_existing {
                OnExisting::Skip => {
                    outcome.skipped.push(tab.name.clone());
                    continue;
                }
                OnExisting::Rebuild => match may_replace(client, target, tab, &mut outcome)? {
                    Some(tab_id) => {
                        outcome.rebuilt.push(tab.name.clone());
                        api::LayoutTarget::ReplaceTab(tab_id)
                    }
                    None => {
                        outcome.declined.push(tab.name.clone());
                        continue;
                    }
                },
            }
        } else if index == 0 && !active_taken && active_tab_is_free(target) {
            // The first tab takes over the workspace's own tab, so a fresh worktree gets
            // its layout where the user is already looking. Replacing it destroys the one
            // bare pane that is there, which is why the tab has to be free to qualify.
            active_taken = true;
            outcome.built.push(tab.name.clone());
            api::LayoutTarget::ReplaceTab(&target.active_tab_id)
        } else {
            // A busy active tab is not wrecked, and every later tab is added beside the
            // ones already there.
            existing.push(tab.name.clone());
            outcome.built.push(tab.name.clone());
            api::LayoutTarget::AddToWorkspace(&target.workspace_id)
        };

        // ONE CALL builds the whole tab. It is atomic, so a rejected tree leaves nothing
        // behind and there is no half-built tab to report or clean up.
        let pane_ids = api::layout_apply(client, target_tab, &tab.name, tree_for(tab, cwd), false)
            .map_err(|e| {
                Fatal(format!(
                    "{}: could not build tab \"{}\": {}",
                    target.label, tab.name, e
                ))
            })?;

        if pane_ids.len() != tab.panes.len() {
            return Err(Fatal(format!(
                "{}: tab \"{}\" wanted {} panes and Herdr made {}",
                target.label,
                tab.name,
                tab.panes.len(),
                pane_ids.len()
            )));
        }

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
    }

    Ok(outcome)
}

/// Whether an existing tab may be replaced, and its id if so.
///
/// `layout.apply` with a `tab_id` destroys every pane in that tab, including one running
/// an agent, and there is no way to carry a pane across: `pane_id` on a leaf is
/// output-only. So a tab holding an agent is a question rather than a decision.
///
/// Returns `None` to mean leave this tab completely alone.
fn may_replace<'a>(
    client: &Client,
    target: &'a Target,
    tab: &Tab,
    outcome: &mut Outcome,
) -> Result<Option<&'a str>, Fatal> {
    let Some(info) = target.tab_named(&tab.name) else {
        return Ok(None);
    };
    let with_agents: Vec<&LivePane> = target
        .panes_of(&info.tab_id)
        .into_iter()
        .filter(|p| p.agent.is_some())
        .collect();

    // Nothing is at risk, so there is nothing to consent to. Asking here would stall the
    // common case behind a popup for no reason.
    if with_agents.is_empty() {
        return Ok(Some(&info.tab_id));
    }

    let (answer, note) = confirm::ask(client, &question(&tab.name, &with_agents));
    if let Some(note) = note {
        outcome
            .notes
            .push(format!("tab \"{}\": {}", tab.name, note));
    }
    match answer {
        Answer::CloseIt => Ok(Some(&info.tab_id)),
        // Every other outcome, including a dismissal, a timeout, a popup that could not
        // open and an answer that is none of the offered ones. A misfire must cost
        // nothing, so not one pane of this tab is touched.
        Answer::DoNothing => Ok(None),
    }
}

/// A configured tab as the recursive tree `layout.apply` takes.
///
/// The config is a flat list where each pane after the first splits the one before it.
/// That nests to the right: pane 1 against everything else, then pane 2 against
/// everything after it, and so on. Reading it back out with `layout.export` on a tab the
/// old sequential engine built returns exactly this shape, which is how the mapping was
/// confirmed rather than assumed.
///
/// **A leaf carries `cwd` and nothing else**, and both omissions are deliberate.
///
/// `command` is left out because the field execs raw argv against the **server's** PATH.
/// Under launchd that is `/usr/bin:/bin:/usr/sbin:/sbin`, so a bare `lazygit` would not
/// resolve, and a whole command line in one element is looked up as a single executable
/// name. A leaf with no command spawns the user's interactive login shell instead, which
/// is what `pane.send_input` then types into, preserving their PATH, their rc and
/// unquoted flags.
///
/// `label` is left out because an **empty** label on a leaf comes back as `null`,
/// indistinguishable from an absent one. This plugin's whole labelling contract turns on
/// telling those apart, so labels keep going through `pane.rename`.
///
/// `cwd` is set on **every** leaf rather than relying on inheritance: a leaf that omits
/// it inherits its sibling's, not the workspace's.
fn tree_for(tab: &Tab, cwd: &str) -> serde_json::Value {
    fn node(panes: &[crate::config::Pane], cwd: &str) -> serde_json::Value {
        let leaf = serde_json::json!({"type": "pane", "cwd": cwd});
        match panes.split_first() {
            None | Some((_, [])) => leaf,
            Some((_, rest)) => {
                // The split's direction and ratio belong to the pane being created, and
                // the ratio is the share the pane being split keeps. That is what the
                // config means and what the sequential engine did.
                //
                // **An absent ratio has to become a number here.** `pane.split` took a
                // nullable ratio, so the old engine could leave it out and let Herdr
                // choose; a split node cannot. Measured on 0.9.0: omitting it answers
                // `missing field "ratio"` and `null` answers `expected f32`.
                //
                // 0.5 is not a guess at Herdr's default, it IS Herdr's default: a
                // `pane.split` with no ratio exports back as `"ratio": 0.5`. So the
                // config's promise that an absent ratio is not invented still holds in
                // effect, even though the wire now carries a value.
                let next = &rest[0];
                serde_json::json!({
                    "type": "split",
                    "direction": next.split.map(|d| d.as_str()).unwrap_or("right"),
                    "ratio": next.ratio.unwrap_or(0.5),
                    "first": leaf,
                    "second": node(rest, cwd),
                })
            }
        }
    }
    node(&tab.panes, cwd)
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

fn active_tab_is_free(target: &Target) -> bool {
    let active = target.active_panes();
    active.len() == 1 && active[0].agent.is_none()
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
