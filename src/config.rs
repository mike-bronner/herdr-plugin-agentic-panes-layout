use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const FILE_NAME: &str = "agent-layout.toml";
pub const BUILT_IN_NAME: &str = "built-in";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Right,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Pane {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub split: Option<Direction>,
    #[serde(default)]
    pub ratio: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tab {
    pub name: String,
    #[serde(default)]
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProjectRule {
    pub path: String,
    pub layout: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub projects: Vec<ProjectRule>,
    #[serde(default)]
    pub layouts: BTreeMap<String, Layout>,
}

pub fn built_in_layout() -> Layout {
    Layout {
        tabs: vec![Tab {
            name: "agent".to_string(),
            panes: vec![
                Pane {
                    agent: Some("claude".to_string()),
                    command: None,
                    split: None,
                    ratio: None,
                    label: None,
                },
                Pane {
                    agent: None,
                    command: Some("lazygit".to_string()),
                    split: Some(Direction::Right),
                    ratio: Some(0.5),
                    label: None,
                },
                Pane {
                    agent: None,
                    command: None,
                    split: Some(Direction::Down),
                    ratio: Some(0.6),
                    label: None,
                },
            ],
        }],
    }
}

pub fn built_in_config() -> Config {
    let mut layouts = BTreeMap::new();
    layouts.insert(BUILT_IN_NAME.to_string(), built_in_layout());
    Config {
        default: Some(BUILT_IN_NAME.to_string()),
        projects: Vec::new(),
        layouts,
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub source: Option<PathBuf>,
    pub diagnostics: Vec<String>,
    /// Layouts the file defines but that cannot be built, so were dropped.
    ///
    /// Kept separately from `diagnostics` so `choose` can tell "you named a layout
    /// that is broken" from "you named a layout that does not exist". Those are
    /// different mistakes and deserve different messages.
    pub unusable: Vec<String>,
}

pub fn config_root() -> PathBuf {
    if let Some(raw) = non_empty_var("HERDR_CONFIG_PATH") {
        let path = expand_home(&raw);
        if path.is_dir() {
            return path;
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                return parent.to_path_buf();
            }
        }
    }

    if let Some(raw) = non_empty_var("HERDR_PLUGIN_CONFIG_DIR") {
        let dir = PathBuf::from(trim_slashes(&raw));
        if let Some(root) = dir.parent().and_then(Path::parent).and_then(Path::parent) {
            if !root.as_os_str().is_empty() {
                return root.to_path_buf();
            }
        }
    }

    let base = match non_empty_var("XDG_CONFIG_HOME") {
        Some(raw) => expand_home(&raw),
        None => expand_home("~/.config"),
    };
    let release = base.join("herdr");
    let debug = base.join("herdr-dev");
    if !release.is_dir() && debug.is_dir() {
        return debug;
    }
    release
}

pub fn config_path() -> PathBuf {
    config_root().join(FILE_NAME)
}

pub fn load() -> Loaded {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                config: built_in_config(),
                source: None,
                diagnostics: Vec::new(),
                unusable: Vec::new(),
            }
        }
        Err(e) => return fallback(path, format!("cannot read {}: {}", path.display(), e)),
    };

    let mut unknown = Vec::new();
    let deserializer = toml::Deserializer::new(&text);
    let parsed: Result<Config, _> =
        serde_ignored::deserialize(deserializer, |key| unknown.push(key.to_string()));

    let config = match parsed {
        Ok(config) => config,
        Err(e) => {
            return fallback(
                path,
                format!(
                    "{} is not usable: {}",
                    path.display(),
                    first_line(&e.to_string())
                ),
            )
        }
    };

    let mut diagnostics: Vec<String> = unknown
        .iter()
        .map(|key| {
            let mut note = format!("{}: unknown key \"{}\"; ignored", path.display(), key);
            if let Some(hint) = misplaced_key_hint(key) {
                note.push_str(&format!(". {}", hint));
            }
            note
        })
        .collect();

    // A broken layout disables only itself. The file parsed, so every other layout is
    // intact, and dropping the unusable ones here means nothing downstream can pick one
    // and fail halfway through building a tab.
    let mut config = config;
    let found = validate(&config);
    for problem in &found.problems {
        diagnostics.push(format!("{}: {}", path.display(), problem));
    }
    for name in &found.unusable {
        config.layouts.remove(name);
    }
    config.projects.retain(|rule| !rule.path.is_empty());

    Loaded {
        config,
        source: Some(path.to_path_buf()),
        diagnostics,
        unusable: found.unusable,
    }
}

fn fallback(path: &Path, why: String) -> Loaded {
    let _ = path;
    // A separate sentence rather than a clause, because the reasons now end in a full
    // stop and often in a whole paragraph of advice. Joining with a semicolon produced
    // "...See docs/configuration.md.; the built-in layout applies".
    let why = why.trim_end().to_string();
    let joined = if why.ends_with('.') {
        format!("{} The built-in layout applies.", why)
    } else {
        format!("{}; the built-in layout applies", why)
    };
    Loaded {
        config: built_in_config(),
        source: None,
        diagnostics: vec![joined],
        unusable: Vec::new(),
    }
}

#[derive(Debug)]
pub struct Chosen {
    pub name: String,
    pub layout: Layout,
    pub rule_path: Option<String>,
    pub diagnostic: Option<String>,
}

impl Loaded {
    pub fn choose(&self, candidates: &[String]) -> Chosen {
        let rule = self
            .config
            .projects
            .iter()
            .find(|rule| matches_project(&rule.path, candidates));
        let name = rule
            .map(|r| r.layout.clone())
            .or_else(|| self.config.default.clone());
        let rule_path = rule.map(|r| r.path.clone());

        match name {
            Some(name) => match self.config.layouts.get(&name) {
                Some(layout) => Chosen {
                    name,
                    layout: layout.clone(),
                    rule_path,
                    diagnostic: None,
                },
                // Two different mistakes, so two different messages. Naming a broken
                // layout is a problem already reported in full above; naming one that
                // does not exist is usually a typo.
                //
                // Either way the built-in layout applies. A workspace with no layout at
                // all is worse than a workspace with the default: the whole point of
                // the plugin is that a new worktree arrives usable.
                None if self.unusable.iter().any(|u| u == &name) => Chosen {
                    diagnostic: Some(format!(
                        "layout \"{}\" cannot be built, see the problem reported above; \
                         the built-in layout applies instead",
                        name
                    )),
                    name: BUILT_IN_NAME.to_string(),
                    layout: built_in_layout(),
                    rule_path,
                },
                None => Chosen {
                    diagnostic: Some(format!(
                        "no layout named \"{}\"; the built-in layout applies",
                        name
                    )),
                    name: BUILT_IN_NAME.to_string(),
                    layout: built_in_layout(),
                    rule_path,
                },
            },
            None => Chosen {
                diagnostic: Some(
                    "no default layout is named; the built-in layout applies".to_string(),
                ),
                name: BUILT_IN_NAME.to_string(),
                layout: built_in_layout(),
                rule_path,
            },
        }
    }
}

pub fn matches_project(rule_path: &str, candidates: &[String]) -> bool {
    let wanted = normalise(rule_path);
    if wanted.as_os_str().is_empty() {
        return false;
    }
    candidates.iter().any(|c| normalise(c) == wanted)
}

/// Where a key belongs, when it is a real key in the wrong table.
///
/// Only for keys the schema genuinely has somewhere else. This is not spelling
/// correction and does not guess: `path` really is a key, so saying where it lives is
/// a fact rather than a suggestion. A key the schema has nowhere gets the plain
/// unknown-key message, because anything more would be invention.
fn misplaced_key_hint(key: &str) -> Option<&'static str> {
    match key.rsplit('.').next()? {
        // A rule is matched against the workspace's own directory as well as its
        // checkout path and repo root, so a plain directory works. An earlier version of
        // this hint said non-repositories could not be matched, which was true then and
        // is not now; leaving it would have been a trap.
        "path" => Some(
            "\"path\" is a [[projects]] key, not a tab key. To make a layout apply in a \
             directory, add a [[projects]] entry naming that directory and the layout. It is \
             matched against the workspace's own directory, its checkout path, and its repo \
             root, so a plain directory works as well as a repository.",
        ),
        "layout" => Some("\"layout\" is a [[projects]] key, naming which layout that path uses."),
        "agent" | "command" | "split" | "ratio" | "label" => {
            Some("that is a pane key; it belongs in a [[...tabs.panes]] block.")
        }
        "name" => Some("\"name\" is a tab key; it belongs in a [[...tabs]] block."),
        "tabs" => Some("\"tabs\" belongs to a layout, as [[layouts.<name>.tabs]]."),
        "panes" => Some("\"panes\" belongs to a tab, as [[layouts.<name>.tabs.panes]]."),
        "default" => Some("\"default\" is a top-level key, outside every table."),
        _ => None,
    }
}

/// What checking a parsed config found.
#[derive(Debug, Default)]
pub struct Findings {
    /// Every problem, in reading order. Not the first one.
    pub problems: Vec<String>,
    /// Layouts that cannot be built, so must not be offered.
    pub unusable: Vec<String>,
}

/// Check every layout, and report **every** problem rather than the first.
///
/// Two properties are deliberate.
///
/// **A broken layout disables only itself.** The file parsed, so the other layouts
/// are intact and skipping one loses nothing. This changed after a layout somebody
/// was experimenting with silently disabled the layout they depended on. A TOML
/// syntax error is still fatal to the whole file, because a file that will not parse
/// has no layouts to salvage.
///
/// **Every problem is collected.** Returning at the first one meant somebody fixing
/// three mistakes learned about one per run, which is three worktrees or three
/// `--check` runs to discover what one could have told them.
///
/// Every message names the **fix**, not only the rule. The rule alone is no use to
/// somebody who has not read `docs/configuration.md`, and two of these are mistakes
/// where the natural reading of the schema is the wrong one.
pub fn validate(config: &Config) -> Findings {
    let mut found = Findings::default();

    for (name, layout) in &config.layouts {
        let before = found.problems.len();
        check_layout(name, layout, &mut found.problems);
        if found.problems.len() > before {
            found.unusable.push(name.clone());
        }
    }

    for rule in &config.projects {
        if rule.path.is_empty() {
            found.problems.push(
                "a [[projects]] entry has an empty path, so it can never match; give it the \
                 absolute path of a directory, a repository or a worktree, or remove the entry"
                    .to_string(),
            );
        }
    }

    found
}

fn check_layout(name: &str, layout: &Layout, problems: &mut Vec<String>) {
    if layout.tabs.is_empty() {
        problems.push(format!(
            "layout \"{}\" has no tabs; add a [[layouts.{}.tabs]] block with a name",
            name, name
        ));
        return;
    }
    for tab in &layout.tabs {
        if tab.name.is_empty() {
            problems.push(format!(
                "layout \"{}\" has a tab with an empty name; a tab's name is what the \
                 re-run guard matches on, so it has to be something",
                name
            ));
            continue;
        }
        if tab.panes.is_empty() {
            problems.push(format!(
                "layout \"{}\" tab \"{}\" has no panes; add a [[layouts.{}.tabs.panes]] \
                 block, which describes the tab's own pane",
                name, tab.name, name
            ));
            continue;
        }
        for (index, pane) in tab.panes.iter().enumerate() {
            if index == 0 && pane.split.is_some() {
                problems.push(format!(
                    "layout \"{}\" tab \"{}\": the first [[layouts.{}.tabs.panes]] block is \
                     the tab's own pane, so it cannot carry a split. To get two panes side \
                     by side, write TWO pane blocks: an empty first one for the tab's own \
                     pane, then a second carrying split = \"{}\". \
                     See docs/configuration.md.",
                    name,
                    tab.name,
                    name,
                    pane.split.map(|d| d.as_str()).unwrap_or("right")
                ));
            }
            if index > 0 && pane.split.is_none() {
                problems.push(format!(
                    "layout \"{}\" tab \"{}\": pane {} has no split, so there is nowhere to \
                     put it. Every pane after the first is split out of the one before it, \
                     so add split = \"right\" or split = \"down\" to that block. \
                     See docs/configuration.md.",
                    name,
                    tab.name,
                    index + 1
                ));
            }
        }
    }
}

fn normalise(raw: &str) -> PathBuf {
    let expanded = expand_home(raw);
    PathBuf::from(trim_slashes(&expanded.to_string_lossy()))
}

fn trim_slashes(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() && raw.starts_with('/') {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn expand_home(raw: &str) -> PathBuf {
    if raw == "~" {
        return home();
    }
    match raw.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(raw),
    }
}

fn home() -> PathBuf {
    non_empty_var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}
