use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

pub const SOCKET_VAR: &str = "HERDR_SOCKET_PATH";

#[derive(Debug)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub enum CallError {
    Transport(String),
    Api(ApiError),
}

impl CallError {
    pub fn code(&self) -> Option<&str> {
        match self {
            CallError::Api(e) => Some(e.code.as_str()),
            CallError::Transport(_) => None,
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Transport(m) => write!(f, "{}", m),
            CallError::Api(e) => write!(f, "{} ({})", e.message, e.code),
        }
    }
}

pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: PathBuf) -> Client {
        Client { socket }
    }

    pub fn from_env() -> Result<Client, String> {
        match std::env::var_os(SOCKET_VAR) {
            Some(v) if !v.is_empty() => Ok(Client::new(PathBuf::from(v))),
            _ => Err(format!("{} is not set", SOCKET_VAR)),
        }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let request = json!({"id": format!("agent-layout:{}", method),
                             "method": method,
                             "params": params});

        let stream = UnixStream::connect(&self.socket).map_err(|e| {
            CallError::Transport(format!("cannot reach {}: {}", self.socket.display(), e))
        })?;
        let mut writer = &stream;
        writer
            .write_all(format!("{}\n", request).as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|e| CallError::Transport(format!("cannot send {}: {}", method, e)))?;

        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| {
            CallError::Transport(format!("cannot read the answer to {}: {}", method, e))
        })?;
        if line.trim().is_empty() {
            return Err(CallError::Transport(format!(
                "the server closed the connection without answering {}",
                method
            )));
        }

        let answer: Value = serde_json::from_str(&line).map_err(|e| {
            CallError::Transport(format!("the answer to {} is not JSON: {}", method, e))
        })?;

        if let Some(err) = answer.get("error") {
            return Err(CallError::Api(ApiError {
                code: string_at(err, "code").unwrap_or_default(),
                message: string_at(err, "message")
                    .unwrap_or_else(|| format!("{} failed with no message", method)),
            }));
        }
        match answer.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(CallError::Transport(format!(
                "the answer to {} carries neither a result nor an error",
                method
            ))),
        }
    }
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(|s| s.to_string())
}

pub fn params(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        if !value.is_null() {
            map.insert(key.to_string(), value);
        }
    }
    Value::Object(map)
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub active_tab_id: String,
    pub label: String,
    pub focused: bool,
    pub checkout_path: Option<String>,
    pub repo_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub cwd: Option<String>,
    pub agent: Option<String>,
}

pub fn workspaces(client: &Client) -> Result<Vec<Workspace>, CallError> {
    let result = client.call("workspace.list", json!({}))?;
    let listed = result
        .get("workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(listed
        .iter()
        .filter_map(|w| {
            let worktree = w.get("worktree");
            Some(Workspace {
                workspace_id: string_at(w, "workspace_id")?,
                active_tab_id: string_at(w, "active_tab_id").unwrap_or_default(),
                label: string_at(w, "label")
                    .unwrap_or_else(|| string_at(w, "workspace_id").unwrap_or_default()),
                focused: w.get("focused").and_then(Value::as_bool).unwrap_or(false),
                checkout_path: worktree.and_then(|t| string_at(t, "checkout_path")),
                repo_root: worktree.and_then(|t| string_at(t, "repo_root")),
            })
        })
        .collect())
}

pub fn panes(client: &Client, workspace_id: &str) -> Result<Vec<Pane>, CallError> {
    let result = client.call("pane.list", json!({"workspace_id": workspace_id}))?;
    let listed = result
        .get("panes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(listed
        .iter()
        .filter_map(|p| {
            Some(Pane {
                pane_id: string_at(p, "pane_id")?,
                tab_id: string_at(p, "tab_id").unwrap_or_default(),
                cwd: string_at(p, "cwd").filter(|s| !s.is_empty()),
                agent: string_at(p, "agent").filter(|s| !s.is_empty()),
            })
        })
        .collect())
}

pub fn tab_labels(client: &Client, workspace_id: &str) -> Result<Vec<String>, CallError> {
    let result = client.call("tab.list", json!({"workspace_id": workspace_id}))?;
    let listed = result
        .get("tabs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(listed
        .iter()
        .filter_map(|t| string_at(t, "label"))
        .collect())
}

pub fn tab_rename(client: &Client, tab_id: &str, label: &str) -> Result<(), CallError> {
    client
        .call("tab.rename", json!({"tab_id": tab_id, "label": label}))
        .map(|_| ())
}

pub fn tab_create(
    client: &Client,
    workspace_id: &str,
    cwd: &str,
    label: &str,
) -> Result<String, CallError> {
    let result = client.call(
        "tab.create",
        json!({"workspace_id": workspace_id, "cwd": cwd,
               "label": label, "focus": false}),
    )?;
    result
        .get("root_pane")
        .and_then(|p| string_at(p, "pane_id"))
        .ok_or_else(|| {
            CallError::Transport("tab.create answered without a root pane id".to_string())
        })
}

pub fn pane_split(
    client: &Client,
    target_pane_id: &str,
    direction: &str,
    ratio: Option<f64>,
    cwd: &str,
) -> Result<String, CallError> {
    let result = client.call(
        "pane.split",
        params(vec![
            ("target_pane_id", json!(target_pane_id)),
            ("direction", json!(direction)),
            ("ratio", ratio.map(|r| json!(r)).unwrap_or(Value::Null)),
            ("cwd", json!(cwd)),
            ("focus", json!(false)),
        ]),
    )?;
    result
        .get("pane")
        .and_then(|p| string_at(p, "pane_id"))
        .ok_or_else(|| CallError::Transport("pane.split answered without a pane id".to_string()))
}

pub fn pane_rename(client: &Client, pane_id: &str, label: &str) -> Result<(), CallError> {
    client
        .call("pane.rename", json!({"pane_id": pane_id, "label": label}))
        .map(|_| ())
}

pub fn pane_run(client: &Client, pane_id: &str, command: &str) -> Result<(), CallError> {
    client
        .call(
            "pane.send_input",
            json!({"pane_id": pane_id, "text": command, "keys": ["enter"]}),
        )
        .map(|_| ())
}

pub fn agent_start(
    client: &Client,
    name: &str,
    kind: &str,
    pane_id: &str,
) -> Result<(), CallError> {
    client
        .call(
            "agent.start",
            json!({"name": name, "kind": kind, "pane_id": pane_id}),
        )
        .map(|_| ())
}

pub fn notify(client: &Client, body: &str) {
    let _ = client.call(
        "notification.show",
        json!({"title": "agent layout", "body": body}),
    );
}
