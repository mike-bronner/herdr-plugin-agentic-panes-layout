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
        .map(|key| format!("{}: unknown key \"{}\"; ignored", path.display(), key))
        .collect();

    if let Err(problem) = validate(&config) {
        let mut loaded = fallback(path, format!("{}: {}", path.display(), problem));
        diagnostics.append(&mut loaded.diagnostics);
        loaded.diagnostics = diagnostics;
        return loaded;
    }

    Loaded {
        config,
        source: Some(path.to_path_buf()),
        diagnostics,
    }
}

fn fallback(path: &Path, why: String) -> Loaded {
    let _ = path;
    Loaded {
        config: built_in_config(),
        source: None,
        diagnostics: vec![format!("{}; the built-in layout applies", why)],
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

pub fn validate(config: &Config) -> Result<(), String> {
    for (name, layout) in &config.layouts {
        if layout.tabs.is_empty() {
            return Err(format!("layout \"{}\" has no tabs", name));
        }
        for tab in &layout.tabs {
            if tab.name.is_empty() {
                return Err(format!("layout \"{}\" has a tab with an empty name", name));
            }
            if tab.panes.is_empty() {
                return Err(format!(
                    "layout \"{}\" tab \"{}\" has no panes",
                    name, tab.name
                ));
            }
            for (index, pane) in tab.panes.iter().enumerate() {
                if index == 0 && pane.split.is_some() {
                    return Err(format!(
                        "layout \"{}\" tab \"{}\": the first pane is the tab's own pane and cannot carry a split",
                        name, tab.name
                    ));
                }
                if index > 0 && pane.split.is_none() {
                    return Err(format!(
                        "layout \"{}\" tab \"{}\": pane {} needs a split direction, right or down",
                        name,
                        tab.name,
                        index + 1
                    ));
                }
            }
        }
    }
    for rule in &config.projects {
        if rule.path.is_empty() {
            return Err("a [[projects]] entry has an empty path".to_string());
        }
    }
    Ok(())
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
