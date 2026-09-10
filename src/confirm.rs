//! Asking the user a yes/no question in a Herdr popup pane.
//!
//! A plugin pane is a **separate process**, so its answer has to travel back to
//! the process applying the layout. The channel is a file: this side makes a path,
//! hands it to the popup through `plugin.pane.open`'s `env` map, and waits for the
//! popup to write into it. The popup is this same binary under `--confirm`, so
//! there is one language and one place the question is worded.
//!
//! Why a file rather than the socket: the popup would need somewhere to put the
//! answer on the socket too, and Herdr has no plugin-to-plugin channel. A file
//! whose path only the two processes know is the smallest thing that works, and it
//! is created inside a directory this process owns and removes.
//!
//! Three things about `plugin.pane.open`, all measured on 0.9.0, 2026-09-09:
//!
//! 1. **It answers `{"type":"ok"}` and nothing else** — no pane id. So the popup
//!    cannot be polled for by id and cannot be closed by id.
//! 2. **A plugin pane does not appear in `pane.list`.** So it cannot be found by id
//!    afterwards either.
//! 3. **The pane's command process does run**, and was observed running in a server
//!    with no UI client attached.
//!
//! Between them there is no way to learn from Herdr whether the popup is up. So the
//! popup reports itself: it writes its own process id to a second file the instant it
//! starts, before drawing anything. That marker is what proves the process exists,
//! and its pid is what lets a popup the user closed be noticed at once rather than
//! waited out.
//!
//! What the marker does **not** protect against is a popup that starts and is never
//! answered because nothing is displaying it. That case runs out the full timeout and
//! then changes nothing, which is slow but safe.
//!
//! **Two answers, and every failure is the second.**
//!
//! - `y` closes the pane running the agent and rebuilds the tab clean.
//! - `esc` changes nothing at all.
//!
//! There was a third, `n`, which kept the agent and built the layout around it. It is
//! gone because the engine cannot express it: `layout.apply` replaces a tab wholesale
//! and `pane_id` on a leaf is output-only, so a running agent cannot be carried into a
//! new tree. Mike dropped the requirement rather than the rewrite, on the grounds that
//! new panes are being made anyway.
//!
//! A dismissed popup, a popup that could not be opened, a question that timed out, and
//! an answer nobody recognises all mean **`esc`**. That reasoning is unchanged by losing
//! `n`: a popup the user closed or ignored is the same class of event as a misfire, and
//! acting on silence is the thing being prevented.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::api::Client;

pub const PLUGIN_ID: &str = "mikebronner.agentic-panes-layout";
pub const PANE_ENTRYPOINT: &str = "confirm";
pub const QUESTION_VAR: &str = "AGENT_LAYOUT_QUESTION";
pub const ANSWER_VAR: &str = "AGENT_LAYOUT_ANSWER_FILE";
pub const STARTED_VAR: &str = "AGENT_LAYOUT_STARTED_FILE";

/// The words the popup and the waiting side exchange. One definition, so the two halves
/// cannot drift into always disagreeing.
pub const CLOSE_IT: &str = "close";
pub const DO_NOTHING: &str = "nothing";

/// How long to wait for an answer before giving up and answering No. Generous,
/// because the user may have walked away mid-worktree.
const WAIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);
/// How long to wait for the popup to report that it started.
///
/// Short, because this is not the user thinking: it is the gap between asking Herdr
/// for a pane and a process running in it. `plugin.pane.open` answers `ok` whether or
/// not the process starts, so without this bound a popup that failed to launch would
/// freeze the rebuild for the full two minutes.
const STARTUP: Duration = Duration::from_secs(3);
/// How often to check the popup is still alive. Slower than the file poll because
/// each check spawns a process.
const LIVENESS: Duration = Duration::from_millis(500);

/// The two things the user can mean.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Answer {
    /// `y`: close the pane running the agent and rebuild the tab clean.
    CloseIt,
    /// `esc`, or any failure to ask or be answered: change nothing at all.
    DoNothing,
}

/// Ask `question` in a popup and wait for the answer.
///
/// Returns `(answer, note)`: the note is what the caller reports, so the reason a
/// question went unanswered is never swallowed.
pub fn ask(client: &Client, question: &str) -> (Answer, Option<String>) {
    let channel = match Channel::new() {
        Ok(channel) => channel,
        Err(e) => {
            return (
                Answer::DoNothing,
                Some(format!("cannot make a channel for the question: {}", e)),
            )
        }
    };

    let opened = client.call(
        "plugin.pane.open",
        json!({"plugin_id": PLUGIN_ID,
               "entrypoint": PANE_ENTRYPOINT,
               "placement": "popup",
               "width": "60%",
               "height": "30%",
               "focus": true,
               "env": {QUESTION_VAR: question,
                       ANSWER_VAR: channel.answer.to_string_lossy(),
                       STARTED_VAR: channel.started.to_string_lossy()}}),
    );

    // The answer carries no pane id, so an `ok` is only evidence that Herdr accepted
    // the request. Whether a pane appeared is what the started marker decides.
    if let Err(e) = opened {
        return (
            Answer::DoNothing,
            Some(format!(
                "could not open the confirmation popup, so the tab was left alone: {}",
                e
            )),
        );
    }

    decide(channel.watch())
}

/// Whether a process is still alive.
///
/// `kill -0` is a signal-free existence test. Spawning it is not free, so it runs at
/// a slower cadence than the file poll. There is no libc dependency here on purpose:
/// one crate for one syscall is a supply-chain surface this plugin does not need.
fn process_is_alive(pid: &str) -> bool {
    match std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        // If the test itself cannot run, absence is not proven.
        Err(_) => true,
    }
}

/// The two files the popup writes, in a directory this process creates and removes.
struct Channel {
    answer: PathBuf,
    started: PathBuf,
    dir: PathBuf,
}

impl Channel {
    fn new() -> std::io::Result<Channel> {
        // Unique per call, not merely per process. A layout with agents in more than one
        // tab asks more than once, and the popup deletes the file and directory it was
        // given on its way out. Keyed on the pid alone every call shared one path, so a
        // lingering first popup could delete the second question's answer file, and the
        // second would then time out having been answered.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("agent-layout-ask-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir)?;
        Ok(Channel {
            answer: dir.join("answer"),
            started: dir.join("started"),
            dir,
        })
    }

    fn read(path: &PathBuf) -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let text = text.trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Wait for the popup to start, then for it to answer.
    fn watch(&self) -> Ended {
        let pid = match self.await_start() {
            Some(pid) => pid,
            None => {
                // An answer already there means the popup ran and finished inside the
                // startup window, which is the ordinary case for a fast answer.
                return match Self::read(&self.answer) {
                    Some(text) => Ended::Answered(text),
                    None => Ended::NeverShown,
                };
            }
        };

        let deadline = Instant::now() + WAIT;
        let mut since_liveness_check = Instant::now();
        while Instant::now() < deadline {
            if let Some(text) = Self::read(&self.answer) {
                return Ended::Answered(text);
            }
            if since_liveness_check.elapsed() > LIVENESS && !process_is_alive(&pid) {
                // It died without answering, which is what closing the pane does.
                return match Self::read(&self.answer) {
                    Some(text) => Ended::Answered(text),
                    None => Ended::Dismissed,
                };
            }
            if since_liveness_check.elapsed() > LIVENESS {
                since_liveness_check = Instant::now();
            }
            std::thread::sleep(POLL);
        }
        Ended::TimedOut
    }

    fn await_start(&self) -> Option<String> {
        let deadline = Instant::now() + STARTUP;
        while Instant::now() < deadline {
            if let Some(pid) = Self::read(&self.started) {
                return Some(pid);
            }
            if Self::read(&self.answer).is_some() {
                return None;
            }
            std::thread::sleep(POLL);
        }
        None
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How the wait ended, separated from what it means.
///
/// The split exists so every ending is testable. The timeout is 120 seconds, so a
/// test that reached it by waiting would take 120 seconds; keeping the decision
/// pure means it is checked in microseconds instead, and the branch cannot rot
/// unnoticed just because it is slow to provoke.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Answered(String),
    /// The popup process died without answering, which is what closing the pane does.
    Dismissed,
    /// The popup process never reported itself, so it never started.
    ///
    /// `plugin.pane.open` answers `ok` regardless, and a plugin pane cannot be found
    /// in `pane.list`, so this marker is the only evidence either way.
    ///
    /// Only ever reached when a pane really is at risk, because the question is asked
    /// only then. A rebuild with nothing to consent to never gets here and never
    /// stalls.
    NeverShown,
    TimedOut,
}

/// What an ending means.
///
/// **Only an explicit word is acted on.** Everything else changes nothing, and says
/// why: a silent no-op after a keypress reads as a broken keybinding.
pub fn decide(ended: Ended) -> (Answer, Option<String>) {
    match ended {
        Ended::Answered(text) => match text.as_str() {
            CLOSE_IT => (Answer::CloseIt, None),
            DO_NOTHING => (
                Answer::DoNothing,
                Some("the confirmation was cancelled, so the tab was left alone".to_string()),
            ),
            other => (
                Answer::DoNothing,
                Some(format!(
                    "the confirmation answered \"{}\", which is neither choice, so the \
                     tab was left alone",
                    other
                )),
            ),
        },
        Ended::Dismissed => (
            Answer::DoNothing,
            Some("the confirmation was dismissed, so the tab was left alone".to_string()),
        ),
        Ended::NeverShown => (
            Answer::DoNothing,
            Some(format!(
                "the confirmation never started within {}s, so the tab was left alone",
                STARTUP.as_secs()
            )),
        ),
        Ended::TimedOut => (
            Answer::DoNothing,
            Some(format!(
                "the question went unanswered for {}s, so the tab was left alone",
                WAIT.as_secs()
            )),
        ),
    }
}

/// The `--confirm` side: draw the question in the popup and record the answer.
///
/// One keypress, no Enter. There is no ratatui here on purpose: a yes/no/cancel
/// needs no widgets, no layout engine and no render loop, and Herdr already draws
/// the pane frame and puts the manifest's `title` on it. Three `println!`s and one
/// key is the whole interface.
pub fn run_popup() -> Result<(), String> {
    let question =
        std::env::var(QUESTION_VAR).unwrap_or_else(|_| "Close the pane running an agent?".into());
    let answer_path = std::env::var(ANSWER_VAR)
        .map_err(|_| format!("{} is not set; nothing to answer into", ANSWER_VAR))?;

    // Report that a pane really appeared, before drawing anything. The waiting side
    // cannot learn this from `plugin.pane.open`, which answers `ok` either way, and
    // the process id is what lets a popup the user closes be noticed at once.
    if let Ok(started_path) = std::env::var(STARTED_VAR) {
        let _ = std::fs::write(started_path, std::process::id().to_string());
    }

    println!("{}", question);
    println!();
    println!("  y    close it and rebuild the tab");
    println!("  esc  change nothing");
    println!();
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let answer = read_choice();

    std::fs::write(&answer_path, answer)
        .map_err(|e| format!("cannot write the answer to {}: {}", answer_path, e))?;

    println!();
    println!(
        "{}",
        match answer {
            CLOSE_IT => "Closing it and rebuilding.",
            _ => "Changing nothing.",
        }
    );
    Ok(())
}

/// One keypress, or one line where there is no terminal to read a keypress from.
///
/// Raw mode needs a tty. The popup always has one, being a real pane; a test harness
/// piping stdin does not, and neither would a stray invocation from a script. Rather
/// than fail there, it falls back to a line — which keeps the three choices
/// answerable either way and means the fallback is exercised by the suite rather
/// than being untested code that only runs when something has gone wrong.
fn read_choice() -> &'static str {
    match read_key() {
        Some(answer) => answer,
        None => read_line(),
    }
}

fn read_key() -> Option<&'static str> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    enable_raw_mode().ok()?;
    let answer = loop {
        match read() {
            Ok(Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            })) => {
                // A key press and its release both arrive on some terminals; acting on
                // both would read one keystroke as two.
                if kind != KeyEventKind::Press {
                    continue;
                }
                match code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => break CLOSE_IT,
                    // `n` is deliberately NOT an acting key. It used to keep the agent
                    // and build around it, and somebody with that habit pressing it now
                    // must change nothing rather than destroy the pane.
                    KeyCode::Esc
                    | KeyCode::Char('n')
                    | KeyCode::Char('N')
                    | KeyCode::Char('q')
                    | KeyCode::Char('Q') => break DO_NOTHING,
                    // Ctrl-C in raw mode is a key event, not a signal, and it means
                    // the same as Esc.
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        break DO_NOTHING
                    }
                    // Any other key is ignored, as in any dialog. The three choices are
                    // on screen, and guessing at an unlisted key is how a misfire
                    // becomes a rearranged workspace.
                    _ => continue,
                }
            }
            // Anything that is not a key, such as a resize, is not an answer.
            Ok(_) => continue,
            Err(_) => break DO_NOTHING,
        }
    };
    let _ = disable_raw_mode();
    Some(answer)
}

fn read_line() -> &'static str {
    let mut typed = String::new();
    match std::io::stdin().read_line(&mut typed) {
        // End of input is not an answer.
        Ok(0) | Err(_) => DO_NOTHING,
        Ok(_) => match typed.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => CLOSE_IT,
            _ => DO_NOTHING,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_words_map_to_the_two_answers() {
        assert_eq!(decide(Ended::Answered(CLOSE_IT.into())).0, Answer::CloseIt);
        assert_eq!(
            decide(Ended::Answered(DO_NOTHING.into())).0,
            Answer::DoNothing
        );
    }

    #[test]
    fn the_word_the_third_answer_used_is_no_longer_acted_on() {
        // `keep` used to mean "keep the agent and build around it". A popup left over
        // from an older build, or anything else still sending it, must change nothing
        // rather than fall through to destroying the pane.
        assert_eq!(decide(Ended::Answered("keep".into())).0, Answer::DoNothing);
    }

    #[test]
    fn only_the_exact_word_closes_a_pane() {
        // The safety property in one place. Every one of these would otherwise be
        // able to close a pane running an agent, whose work cannot be recovered.
        for ended in [
            Ended::Answered(DO_NOTHING.into()),
            Ended::Answered("keep".into()),
            Ended::Answered("".into()),
            Ended::Answered("maybe".into()),
            Ended::Answered("CLOSE".into()),
            Ended::Answered("y".into()),
            Ended::Answered("yes".into()),
            Ended::Answered("close it".into()),
            Ended::Dismissed,
            Ended::NeverShown,
            Ended::TimedOut,
        ] {
            assert_ne!(decide(ended).0, Answer::CloseIt);
        }
    }

    #[test]
    fn every_failure_to_be_answered_changes_nothing() {
        // A dismissed or unanswered popup is the same class of event as a misfire, and
        // the only other answer destroys a pane running an agent.
        for ended in [
            Ended::Dismissed,
            Ended::NeverShown,
            Ended::TimedOut,
            Ended::Answered("maybe".into()),
            Ended::Answered("".into()),
        ] {
            assert_eq!(decide(ended).0, Answer::DoNothing);
        }
    }

    #[test]
    fn an_acted_on_answer_is_silent_and_every_other_ending_explains_itself() {
        // A silent no-op after a keypress reads as a broken keybinding.
        assert_eq!(decide(Ended::Answered(CLOSE_IT.into())).1, None);
        assert!(decide(Ended::Answered(DO_NOTHING.into()))
            .1
            .unwrap()
            .contains("cancelled"));
        assert!(decide(Ended::Answered("maybe".into()))
            .1
            .unwrap()
            .contains("neither choice"));
        assert!(decide(Ended::Dismissed).1.unwrap().contains("dismissed"));
        assert!(decide(Ended::NeverShown)
            .1
            .unwrap()
            .contains("never started"));
        assert!(decide(Ended::TimedOut).1.unwrap().contains("unanswered"));
    }

    #[test]
    fn the_two_words_are_distinct() {
        // They travel through a file between two processes, so a collision would make
        // one choice unreachable.
        assert_ne!(CLOSE_IT, DO_NOTHING);
    }
}
