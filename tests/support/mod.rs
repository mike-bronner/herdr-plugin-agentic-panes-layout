//! A stub Herdr server, and a runner for the real binary against it.
//!
//! Nothing is mocked in-process. The binary under test is executed as a real
//! subprocess and talks newline-delimited JSON over a real Unix socket, exactly
//! as it will talk to Herdr. That is the only honest way to test it: the whole
//! job of this program is which requests it does and does not send, and a stub
//! server is what makes that observable without splitting a pane.
//!
//! This replaces the stub `herdr` shell script the previous Python suite used.
//! One difference is worth knowing: a request is a JSON object, so argument
//! boundaries cannot be lost the way `"$*"` lost them in the old stub. The old
//! suite needed two logs to prove a label with a space stayed one argument;
//! here the recorded `params` carries the string whole or the test fails.

#![allow(dead_code)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// A short-lived directory under /private/tmp.
///
/// Deliberately not the platform temp dir: on macOS that is a long
/// `/var/folders/...` path, and a Unix socket path over 103 bytes fails to bind
/// with `sun_path` overflow. The same trap is recorded in docs/herdr-behaviour.md
/// for Herdr's own config root.
pub struct TempDir {
    path: PathBuf,
}

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    pub fn new() -> TempDir {
        let n = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path = PathBuf::from(format!(
            "/private/tmp/agent-layout-t{}-{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot make the temporary directory");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// How the stub answers. Every field is a knob a test turns.
#[derive(Clone)]
pub struct Script {
    pub workspaces: Value,
    pub panes: Value,
    pub tabs: Value,
    /// Methods that must answer with this error code instead of a result.
    pub fail: Vec<(String, String)>,
    /// `agent.start` answers `agent_name_taken` for each of these names.
    pub taken_names: HashSet<String>,
    /// `agent.start` answers this error code for any name not in `taken_names`.
    pub agent_error: Option<String>,
    /// Fail only the Nth call to `pane.split`, 1-based.
    pub fail_split: Option<u32>,
    /// What the confirmation popup "answers".
    ///
    /// The real popup is a separate process that writes into a file whose path
    /// arrives in `plugin.pane.open`'s `env`. The stub plays the user: it writes
    /// this straight into that file, so the waiting side is exercised for real
    /// rather than stubbed out. `None` writes nothing, which is the timeout path.
    pub popup_answer: Option<String>,
    /// The popup appears and then dies without answering, which is what closing the
    /// pane does.
    pub popup_dies_unanswered: bool,
    /// layout.apply answers with this many panes rather than one per leaf.
    pub apply_panes: Option<usize>,
    /// `agent.start` answers `agent_pane_busy` for this many calls, then behaves
    /// normally. Stands in for a pane whose shell has not reached its prompt yet.
    pub busy_for: u32,
}

impl Default for Script {
    fn default() -> Script {
        Script {
            workspaces: one_workspace(),
            panes: one_bare_pane(),
            tabs: one_unnamed_tab(),
            fail: Vec::new(),
            taken_names: HashSet::new(),
            agent_error: None,
            fail_split: None,
            popup_answer: None,
            popup_dies_unanswered: false,
            apply_panes: None,
            busy_for: 0,
        }
    }
}

impl Script {
    pub fn failing(mut self, method: &str, code: &str) -> Script {
        self.fail.push((method.to_string(), code.to_string()));
        self
    }

    pub fn taken(mut self, names: &[&str]) -> Script {
        self.taken_names = names.iter().map(|n| n.to_string()).collect();
        self
    }

    pub fn agent_error(mut self, code: &str) -> Script {
        self.agent_error = Some(code.to_string());
        self
    }

    pub fn fail_split(mut self, nth: u32) -> Script {
        self.fail_split = Some(nth);
        self
    }

    /// The user answers the confirmation popup this way.
    pub fn answers(mut self, answer: &str) -> Script {
        self.popup_answer = Some(answer.to_string());
        self
    }

    /// The popup opens and is then closed without an answer.
    pub fn popup_dismissed(mut self) -> Script {
        self.popup_dies_unanswered = true;
        self
    }

    /// layout.apply answers with a tree carrying `n` panes, whatever was asked for.
    pub fn apply_returns_panes(mut self, n: usize) -> Script {
        self.apply_panes = Some(n);
        self
    }

    /// The target pane is not at a prompt for the first `n` attempts.
    pub fn busy_for(mut self, n: u32) -> Script {
        self.busy_for = n;
        self
    }
}

pub fn one_workspace() -> Value {
    json!({"type": "workspace_list", "workspaces": [
        {"workspace_id": "w9", "active_tab_id": "t1", "label": "proj one",
         "focused": true}]})
}

pub fn two_workspaces() -> Value {
    json!({"type": "workspace_list", "workspaces": [
        {"workspace_id": "w9", "active_tab_id": "t1", "label": "proj one",
         "focused": true},
        {"workspace_id": "w7", "active_tab_id": "t7", "label": "other proj",
         "focused": false}]})
}

pub fn one_bare_pane() -> Value {
    json!({"type": "pane_list", "panes": [
        {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"}]})
}

pub fn one_bare_pane_in_t7() -> Value {
    json!({"type": "pane_list", "panes": [
        {"pane_id": "p1", "tab_id": "t7", "cwd": "/tmp/other"}]})
}

pub fn one_unnamed_tab() -> Value {
    json!({"type": "tab_list", "tabs": [
        {"tab_id": "t1", "workspace_id": "w9", "label": "1", "number": 1}]})
}

pub fn tabs_labelled(labels: &[&str]) -> Value {
    let tabs: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            json!({"tab_id": format!("t{}", i + 1), "workspace_id": "w9",
                   "label": label, "number": i + 1})
        })
        .collect();
    json!({"type": "tab_list", "tabs": tabs})
}

pub struct Stub {
    socket: PathBuf,
    recorded: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    _dir: TempDir,
}

impl Stub {
    pub fn start(script: Script) -> Stub {
        let dir = TempDir::new();
        let socket = dir.join("herdr.sock");
        let listener = UnixListener::bind(&socket).expect("cannot bind the stub socket");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_log = Arc::clone(&recorded);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let splits = AtomicU32::new(0);
            let tabs = AtomicU32::new(1);
            let starts = AtomicU32::new(0);
            for stream in listener.incoming() {
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                serve(&stream, &script, &thread_log, &splits, &tabs, &starts);
            }
        });

        Stub {
            socket,
            recorded,
            stop,
            _dir: dir,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Every request the binary sent, in order, as `{method, params}`.
    pub fn requests(&self) -> Vec<Value> {
        self.recorded.lock().unwrap().clone()
    }

    /// The methods sent, in order.
    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter_map(|r| r.get("method")?.as_str().map(|s| s.to_string()))
            .collect()
    }

    /// Every request for one method, params only, in order.
    pub fn params_for(&self, method: &str) -> Vec<Value> {
        self.requests()
            .iter()
            .filter(|r| r.get("method").and_then(Value::as_str) == Some(method))
            .map(|r| r.get("params").cloned().unwrap_or(Value::Null))
            .collect()
    }

    /// Every method sent except the toast.
    ///
    /// A refusal still reports itself, exactly as v0.2.0's `die()` did, so
    /// "nothing was read" means no request other than `notification.show`.
    pub fn methods_besides_the_toast(&self) -> Vec<String> {
        self.methods()
            .into_iter()
            .filter(|m| m != "notification.show")
            .collect()
    }

    /// Every `layout.apply`, as `(what it targets, the tab label)`.
    ///
    /// The engine builds one tab per call. `Replace` means it destroyed and rebuilt that
    /// tab, which is what a take-over and a rebuild both do. `Add` means it created a new
    /// tab beside the existing ones.
    pub fn applies(&self) -> Vec<(Applied, String)> {
        self.params_for("layout.apply")
            .iter()
            .map(|p| {
                let target = match (p.get("tab_id"), p.get("workspace_id")) {
                    (Some(t), _) => Applied::Replace(t.as_str().unwrap_or("").to_string()),
                    (_, Some(w)) => Applied::Add(w.as_str().unwrap_or("").to_string()),
                    _ => Applied::Neither,
                };
                let label = p
                    .get("tab_label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                (target, label)
            })
            .collect()
    }

    /// Every request that would alter a workspace.
    ///
    /// The read-only calls and the toast are not layout steps, so a refusal is
    /// proved by this list being empty rather than by no request at all.
    pub fn changing(&self) -> Vec<String> {
        self.methods()
            .into_iter()
            .filter(|m| {
                matches!(
                    m.as_str(),
                    "tab.rename"
                        | "tab.create"
                        | "pane.split"
                        | "pane.rename"
                        | "pane.send_input"
                        | "agent.start"
                        | "pane.close"
                        | "layout.apply"
                )
            })
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
    }
}

fn serve(
    stream: &UnixStream,
    script: &Script,
    log: &Arc<Mutex<Vec<Value>>>,
    splits: &AtomicU32,
    tabs: &AtomicU32,
    starts: &AtomicU32,
) {
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let Ok(request) = serde_json::from_str::<Value>(&line) else {
        return;
    };
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = request.get("params").cloned().unwrap_or(json!({}));

    log.lock()
        .unwrap()
        .push(json!({"method": method, "params": params}));

    let id = request.get("id").cloned().unwrap_or(json!("stub"));
    let answer = answer_for(&method, &params, script, splits, tabs, starts, &id);
    let mut out = stream;
    let _ = out.write_all(format!("{}\n", answer).as_bytes());
    let _ = out.flush();
}

#[allow(clippy::too_many_arguments)]
fn answer_for(
    method: &str,
    params: &Value,
    script: &Script,
    splits: &AtomicU32,
    tabs: &AtomicU32,
    starts: &AtomicU32,
    id: &Value,
) -> Value {
    let fail = |code: &str| {
        json!({"id": id, "error": {"code": code,
               "message": format!("stub refused {}", method)}})
    };
    let ok = |result: Value| json!({"id": id, "result": result});

    if let Some((_, code)) = script.fail.iter().find(|(m, _)| m == method) {
        return fail(code);
    }

    match method {
        "workspace.list" => ok(script.workspaces.clone()),
        "pane.list" => ok(script.panes.clone()),
        "tab.list" => ok(script.tabs.clone()),
        "tab.rename" => ok(json!({"type": "tab_info", "tab": {"tab_id": "t1"}})),
        "tab.create" => {
            let n = tabs.fetch_add(1, Ordering::SeqCst) + 1;
            ok(json!({"type": "tab_created",
                      "tab": {"tab_id": format!("t{}", n)},
                      "root_pane": {"pane_id": format!("t{}p1", n)}}))
        }
        "layout.apply" => {
            // Answers the way the real server does, measured on 0.9.0: the sent tree is
            // echoed back with a fresh `pane_id` on every leaf, so a caller can map leaf
            // to pane exactly. A `tab_id` replaces that tab and the id changes; a
            // `workspace_id` adds one.
            let n = tabs.fetch_add(1, Ordering::SeqCst) + 1;
            let mut root = params.get("root").cloned().unwrap_or(json!({}));
            let mut next = 0;
            fill_pane_ids(&mut root, n, &mut next);
            if let Some(want) = script.apply_panes {
                root = json!({"type": "pane", "pane_id": "t9p1"});
                let mut chain = root.clone();
                for i in 2..=want {
                    chain = json!({"type": "split", "direction": "right", "ratio": 0.5,
                                   "first": chain,
                                   "second": {"type": "pane", "pane_id": format!("t9p{}", i)}});
                }
                root = chain;
            }
            ok(json!({"type": "layout_apply",
                      "layout": {"workspace_id": "w9",
                                 "tab_id": format!("t{}", n),
                                 "root": root}}))
        }
        "pane.split" => {
            let n = splits.fetch_add(1, Ordering::SeqCst) + 1;
            if script.fail_split == Some(n) {
                return fail("pane_split_failed");
            }
            ok(json!({"type": "pane_info",
                      "pane": {"pane_id": format!("p{}", n + 1)}}))
        }
        "pane.rename" => ok(json!({"type": "pane_info", "pane": {"pane_id": "p1"}})),
        "pane.send_input" => ok(json!({"type": "ok"})),
        "agent.start" => {
            // A pane that has not reached its prompt yet. Counted across calls so a
            // test can have it become ready partway through.
            if starts.fetch_add(1, Ordering::SeqCst) < script.busy_for {
                return fail("agent_pane_busy");
            }
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if script.taken_names.contains(name) {
                return fail("agent_name_taken");
            }
            match &script.agent_error {
                Some(code) => fail(code),
                None => ok(json!({"type": "agent_started",
                                  "agent": {"name": name}})),
            }
        }
        "pane.close" => ok(json!({"type": "ok"})),
        "plugin.pane.open" => {
            // Play both Herdr and the user. The two file paths arrive in `env`,
            // exactly as the real popup process would receive them, so the waiting
            // side is exercised for real rather than stubbed out.
            //
            // The started marker carries this test process's own id, because a live
            // pid is what "the popup is still up" looks like. Writing neither file is
            // what a server with no attached UI client produces: `ok`, and no pane.
            let env = params.get("env");
            let path_for = |key: &str| {
                env.and_then(|e| e.get(key))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            };
            if let Some(answer) = &script.popup_answer {
                if let Some(started) = path_for("AGENT_LAYOUT_STARTED_FILE") {
                    let _ = std::fs::write(started, std::process::id().to_string());
                }
                if let Some(path) = path_for("AGENT_LAYOUT_ANSWER_FILE") {
                    let _ = std::fs::write(path, answer);
                }
            } else if script.popup_dies_unanswered {
                if let Some(started) = path_for("AGENT_LAYOUT_STARTED_FILE") {
                    let _ = std::fs::write(started, dead_pid());
                }
            }
            // Measured on 0.9.0: the real answer is exactly this, with no pane id.
            ok(json!({"type": "ok"}))
        }
        "plugin.pane.close" => ok(json!({"type": "ok"})),
        "notification.show" => ok(json!({"type": "notification_show", "shown": false})),
        _ => fail("unhandled_by_stub"),
    }
}

/// A process id that is certainly not running.
///
/// Spawned and reaped, so the id is real and freed rather than made up. Reuse is
/// possible in principle and vanishingly unlikely inside one short test run.
fn dead_pid() -> String {
    let child = Command::new("/usr/bin/true")
        .spawn()
        .expect("cannot spawn /usr/bin/true");
    let pid = child.id();
    let mut child = child;
    let _ = child.wait();
    pid.to_string()
}

/// Stamp a fresh pane id onto every leaf, depth first, as the server does.
fn fill_pane_ids(node: &mut Value, tab: u32, next: &mut u32) {
    match node.get("type").and_then(Value::as_str) {
        Some("pane") => {
            *next += 1;
            if let Some(map) = node.as_object_mut() {
                map.insert("pane_id".to_string(), json!(format!("t{}p{}", tab, next)));
            }
        }
        Some("split") => {
            for side in ["first", "second"] {
                if let Some(child) = node.get_mut(side) {
                    fill_pane_ids(child, tab, next);
                }
            }
        }
        _ => {}
    }
}

/// How many pane leaves a `layout.apply` tree carries.
///
/// The split count is one less, so this is the pane arithmetic the old suite did with
/// `pane.split` call counts, read off the tree instead.
pub fn leaves(node: &Value) -> usize {
    match node.get("type").and_then(Value::as_str) {
        Some("pane") => 1,
        Some("split") => ["first", "second"]
            .iter()
            .filter_map(|side| node.get(side))
            .map(leaves)
            .sum(),
        _ => 0,
    }
}

/// What a `layout.apply` acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Replaced this tab. Every pane in it was destroyed.
    Replace(String),
    /// Added a tab to this workspace. Existing tabs untouched.
    Add(String),
    Neither,
}

pub struct Run {
    pub status: i32,
    pub stderr: String,
}

impl Run {
    pub fn says(&self, needle: &str) -> bool {
        self.stderr.contains(needle)
    }
}

/// Run the real binary against `stub`, with an environment built from scratch.
///
/// Never inherited: a stray `HERDR_*` in the developer's shell must not decide a
/// test. `PATH` is the one Herdr's launchd server really has, so a binary that
/// only works because of a richer PATH fails here instead of in production.
pub fn run(stub: &Stub, args: &[&str], config_root: Option<&Path>) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .args(args)
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", stub.socket().parent().unwrap())
        .env("HERDR_SOCKET_PATH", stub.socket());
    if let Some(root) = config_root {
        command.env("HERDR_CONFIG_PATH", root);
    } else {
        command.env("XDG_CONFIG_HOME", stub.socket().parent().unwrap());
    }
    finish(command.output().expect("cannot run the binary"))
}

/// Run with extra environment variables, for the event-gate tests.
pub fn run_with_env(
    stub: &Stub,
    args: &[&str],
    config_root: Option<&Path>,
    extra: &[(&str, &str)],
) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-layout"));
    command
        .args(args)
        .env_clear()
        .env("PATH", LAUNCHD_PATH)
        .env("HOME", stub.socket().parent().unwrap())
        .env("HERDR_SOCKET_PATH", stub.socket());
    if let Some(root) = config_root {
        command.env("HERDR_CONFIG_PATH", root);
    } else {
        command.env("XDG_CONFIG_HOME", stub.socket().parent().unwrap());
    }
    for (key, value) in extra {
        command.env(key, value);
    }
    finish(command.output().expect("cannot run the binary"))
}

fn finish(output: Output) -> Run {
    Run {
        status: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

/// Write `agent-layout.toml` into a fresh config root and return that root.
pub fn config_root_with(dir: &TempDir, toml: &str) -> PathBuf {
    let root = dir.join("herdr");
    std::fs::create_dir_all(&root).expect("cannot make the config root");
    std::fs::write(root.join("agent-layout.toml"), toml).expect("cannot write the config");
    root
}
