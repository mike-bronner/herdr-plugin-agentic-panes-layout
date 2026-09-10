//! Showing config problems in a popup pane this plugin styles itself.
//!
//! **Why not a toast.** `notification.show` is the only toast API and it cannot carry
//! this. Measured on 0.9.0, in one isolated server holding Mike's own
//! `ui.toast.delivery = "system"`:
//!
//! - `notification.show` answered `{"shown": false, "reason": "no_foreground_client"}`
//!   for every call.
//! - `plugin.pane.open` in the same server started the pane's process.
//!
//! So a config diagnostic sent as a toast was being dropped before it rendered, which
//! is why a broken config felt silent. A pane is a different mechanism and does not
//! consult the toast settings at all.
//!
//! Three more limits make a toast the wrong shape even where it does render. There is
//! no severity: Herdr hardcodes every API-originated notification to one kind, so a
//! plugin cannot style an error differently from a success. Only one toast is live at
//! a time and the next answers `Busy`. And there is a rate limit. This plugin used to
//! loop every diagnostic through a toast, so at best the first one appeared.
//!
//! A pane sidesteps all of it: our own terminal, our own colours, every issue in one
//! place, no rate limit.
//!
//! **It never gates the layout.** The panes are built first and this is opened
//! afterwards, without waiting. A cosmetic warning must not make somebody wait on a
//! dialog to get their workspace.

use std::path::PathBuf;

use serde_json::json;

use crate::api::Client;
use crate::confirm::PLUGIN_ID;

pub const PANE_ENTRYPOINT: &str = "issues";
pub const ISSUES_VAR: &str = "AGENT_LAYOUT_ISSUES_FILE";
pub const HEADING_VAR: &str = "AGENT_LAYOUT_ISSUES_HEADING";

/// Open the issues popup. Returns without waiting for it.
///
/// Failure to open is deliberately quiet here: stderr already carries every
/// diagnostic, and a message about not being able to show messages helps nobody.
pub fn show(client: &Client, heading: &str, diagnostics: &[String]) {
    if diagnostics.is_empty() {
        return;
    }
    let Some(path) = write_issues(diagnostics) else {
        return;
    };
    let _ = client.call(
        "plugin.pane.open",
        json!({"plugin_id": PLUGIN_ID,
               "entrypoint": PANE_ENTRYPOINT,
               "placement": "popup",
               "width": "80%",
               "height": "60%",
               "focus": true,
               "env": {ISSUES_VAR: path.to_string_lossy(),
                       HEADING_VAR: heading}}),
    );
}

/// Unique per call, not merely per process.
///
/// Keyed on the pid alone, every call in a process shared one directory. That was a
/// real race rather than a tidiness point: it made the test suite fail about one run
/// in four, and pids are reused over time, so a file leaked by a popup that never ran
/// could be read by a later process that inherited the number.
static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The file the popup reads. The popup removes it, since this side does not wait.
///
/// Only ever called with a non-empty slice: `show` returns before this when there is
/// nothing to report, because the common case is a good config and it must cost no
/// file, no pane and no process.
fn write_issues(diagnostics: &[String]) -> Option<PathBuf> {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("agent-layout-issues-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("issues");
    // One diagnostic per line. They are single-line by construction; the join is
    // defensive so a future multi-line one cannot silently become two issues.
    let body = diagnostics
        .iter()
        .map(|d| d.replace('\n', " "))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, body).ok()?;
    Some(path)
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// The `--issues` side: render the problems and wait for any key.
///
/// Colours are plain ANSI on purpose. Herdr's theme setting can be `terminal`, meaning
/// it follows the host palette rather than applying one of its own, so basic ANSI is
/// what blends in. Nothing here needs 256 colours or truecolor.
pub fn run_popup() -> Result<(), String> {
    let path = std::env::var(ISSUES_VAR)
        .map_err(|_| format!("{} is not set; nothing to show", ISSUES_VAR))?;
    let heading = std::env::var(HEADING_VAR).unwrap_or_else(|_| "agent layout".to_string());
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read the issues from {}: {}", path, e))?;
    // This side owns the file, because the side that wrote it did not wait.
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::remove_dir(dir);
    }

    let issues: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();

    println!(
        "{}{}agent layout: {} problem{} in your config{}",
        BOLD,
        RED,
        issues.len(),
        if issues.len() == 1 { "" } else { "s" },
        RESET
    );
    println!("{}{}{}", DIM, heading, RESET);
    println!();
    for issue in &issues {
        println!("{}•{} {}", YELLOW, RESET, issue);
        println!();
    }
    println!(
        "{}The layout was still applied. Run `agent-layout --check` to see it in full.{}",
        DIM, RESET
    );
    println!("{}Press any key to dismiss.{}", DIM, RESET);
    use std::io::Write;
    let _ = std::io::stdout().flush();

    wait_for_any_key();
    Ok(())
}

/// Any key. There is nothing to decide here, so there is nothing to choose between.
fn wait_for_any_key() {
    use crossterm::event::{read, Event, KeyEventKind};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    if enable_raw_mode().is_ok() {
        loop {
            match read() {
                // A press, not a release: some terminals send both, and a release from
                // the keystroke that opened the pane would dismiss it instantly.
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        let _ = disable_raw_mode();
        return;
    }

    // No terminal to read a key from, which is what a test harness looks like. Reading
    // a line keeps the popup answerable rather than making it hang.
    let mut typed = String::new();
    let _ = std::io::stdin().read_line(&mut typed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_diagnostic_stays_one_line() {
        // The popup renders one bullet per line, so a diagnostic containing a newline
        // would silently split into two issues and inflate the count.
        let path = write_issues(&["one\ntwo".to_string(), "three".to_string()]).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2, "{:?}", body);
        assert!(body.starts_with("one two"), "{:?}", body);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn two_calls_never_share_a_file() {
        // They used to. The directory was keyed on the process id alone, so every call
        // in a process wrote to one path, and the suite failed about one run in four
        // when two tests overlapped. Pids are also reused over time, so a leaked file
        // could be read by a later process that inherited the number.
        let first = write_issues(&["first".to_string()]).unwrap();
        let second = write_issues(&["second".to_string()]).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
        for p in [first, second] {
            let _ = std::fs::remove_file(&p);
            if let Some(d) = p.parent() {
                let _ = std::fs::remove_dir(d);
            }
        }
    }
}
