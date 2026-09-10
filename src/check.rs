//! `--check`: read a config and say what it would do, touching nothing.
//!
//! This exists because a bad config is **invisible in use**. A file that fails to
//! parse falls back to the built-in layout, which still produces a plausible
//! workspace, so the only symptom is a diagnostic nobody was watching for and a
//! layout that is subtly not the one you wrote. Before this, the way to find out was
//! to create a worktree and study the result.
//!
//! It is deliberately **side-effect free**: no socket is opened, no workspace is
//! listed, no pane is made. That is the whole value, since it is meant to be safe to
//! run against a config you already suspect. It reads the config file and, for
//! project matching, asks git about the current directory. Nothing else.
//!
//! Note that a real run resolves the workspace before it reads the config, so it
//! cannot reach config parsing without a server at all. This path deliberately does
//! not share that order.

use std::path::{Path, PathBuf};

use crate::config::{self, Direction, Loaded};
use crate::project;

/// Report on the config that a real run would load. Returns the exit code.
///
/// Non-zero means the file exists and **would not be used as written**, which is the
/// condition worth failing a script on. Unknown keys are reported but do not fail,
/// because the rest of the file still applies.
pub fn run(cwd: &Path) -> i32 {
    let path = config::config_path();
    let loaded = config::load_from(&path);
    report(&path, &loaded, cwd, &mut std::io::stdout())
}

fn report(path: &Path, loaded: &Loaded, cwd: &Path, out: &mut impl std::io::Write) -> i32 {
    let mut w = |line: String| {
        let _ = writeln!(out, "{}", line);
    };

    match &loaded.source {
        Some(source) => w(format!("config: {}", source.display())),
        None if path.exists() => w(format!("config: {} (NOT USED, see below)", path.display())),
        None => w(format!("config: none at {}", path.display())),
    }
    w(String::new());

    if loaded.diagnostics.is_empty() {
        w("No problems found.".to_string());
    } else {
        w(format!(
            "{} problem{} found:",
            loaded.diagnostics.len(),
            if loaded.diagnostics.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
        for note in &loaded.diagnostics {
            w(String::new());
            w(format!("  - {}", note));
        }
    }
    w(String::new());

    let discarded = path.exists() && loaded.source.is_none();
    if discarded {
        // Only a file that will not parse gets here. A broken layout no longer takes the
        // whole file with it.
        w("The file could not be parsed, so NOTHING in it applies and the".to_string());
        w("built-in layout is used instead.".to_string());
        w(String::new());
    }

    if !loaded.unusable.is_empty() || !loaded.config.layouts.is_empty() {
        w("Layouts in this file:".to_string());
        for name in loaded.config.layouts.keys() {
            w(format!("  {}  usable", name));
        }
        for name in &loaded.unusable {
            w(format!("  {}  SKIPPED, see the problem above", name));
        }
        if !loaded.unusable.is_empty() {
            w(String::new());
            w("A layout with a problem is skipped on its own. The others still work.".to_string());
        }
        w(String::new());
    }

    let candidates = project::candidates(None, None, &cwd.to_string_lossy());
    w(format!(
        "Matching [[projects]] against, from {}:",
        cwd.display()
    ));
    if candidates.is_empty() {
        w("  (nothing, so only the default layout can apply)".to_string());
    } else {
        for c in &candidates {
            w(format!("  {}", c));
        }
    }
    w(String::new());

    let chosen = loaded.choose(&candidates);
    match (&chosen.rule_path, &loaded.source) {
        (Some(rule), _) => w(format!(
            "Layout \"{}\", matched by the [[projects]] rule for {}.",
            chosen.name, rule
        )),
        (None, Some(_)) => w(format!(
            "Layout \"{}\", the file's default; no [[projects]] rule matched.",
            chosen.name
        )),
        (None, None) => w(format!("Layout \"{}\", built in.", chosen.name)),
    }
    if let Some(note) = &chosen.diagnostic {
        w(format!("  {}", note));
    }
    w(String::new());

    w("It would build:".to_string());
    for tab in &chosen.layout.tabs {
        w(format!("  tab \"{}\"", tab.name));
        for (index, pane) in tab.panes.iter().enumerate() {
            w(format!("    {}", describe_pane(index, pane)));
        }
    }
    w(String::new());
    w("Nothing was changed. This check opens no socket and creates no pane.".to_string());

    // A skipped layout fails the check too. The file is being used, but not as written,
    // and that is exactly the condition worth failing a script on.
    i32::from(discarded || !loaded.unusable.is_empty())
}

fn describe_pane(index: usize, pane: &config::Pane) -> String {
    let mut parts = Vec::new();

    parts.push(match (index, pane.split) {
        (0, _) => "the tab's own pane".to_string(),
        (_, Some(d)) => format!(
            "split {} from pane {}{}",
            match d {
                Direction::Right => "right",
                Direction::Down => "down",
            },
            index,
            match pane.ratio {
                Some(r) => format!(", pane {} keeps {}", index, r),
                // The number is shown rather than "Herdr chooses", because the layout
                // call requires one and this is the one it gets — measurably Herdr's own
                // default. A reader checking their config wants the value that will be
                // applied, not a promise that somebody else will decide.
                None => format!(", ratio unset so pane {} keeps 0.5, Herdr's default", index),
            }
        ),
        (_, None) => "no split, which is an error".to_string(),
    });

    if let Some(kind) = &pane.agent {
        parts.push(format!("runs the {} agent", kind));
    }
    if let Some(command) = &pane.command {
        parts.push(format!("runs \"{}\"", command));
    }
    match &pane.label {
        Some(label) if label.is_empty() => parts.push("labelled with an empty string".to_string()),
        Some(label) => parts.push(format!("labelled \"{}\"", label)),
        None => parts.push("not labelled".to_string()),
    }

    format!("pane {}: {}", index + 1, parts.join(", "))
}

/// The directory `--check` matches `[[projects]]` rules against.
pub fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests run in parallel, so the directory has to be unique per call. Keying it on
    /// the process id alone had them writing over each other's fixtures.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn rendered(toml: &str) -> (String, i32) {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("agent-layout-check-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent-layout.toml");
        std::fs::write(&path, toml).unwrap();
        let loaded = config::load_from(&path);
        let mut out: Vec<u8> = Vec::new();
        let code = report(&path, &loaded, &dir, &mut out);
        let _ = std::fs::remove_dir_all(&dir);
        (String::from_utf8(out).unwrap(), code)
    }

    /// Mike's own config, verbatim, as written by hand after v0.3.0 shipped.
    ///
    /// It is the fixture because it is the real mistake: a two-pane layout expressed
    /// as one pane block carrying a split, which is the natural reading of the schema
    /// and is wrong. It also cost him the layout he actually uses, which is the part
    /// that made this worth building rather than documenting.
    const MIKES_BROKEN_CONFIG: &str = r#"default = "agentic-layout"

[[layouts.home.tabs]]
path="~"
name="shells"

[[layouts.home.tabs.panes]]
split="right"
ratio=0.5

[[layouts.agentic-layout.tabs]]
name = "agent"

[[layouts.agentic-layout.tabs.panes]]
agent = "claude"

[[layouts.agentic-layout.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"

[[layouts.agentic-layout.tabs.panes]]
split = "down"
ratio = 0.7
label = "shell"
"#;

    #[test]
    fn mikes_broken_layout_is_skipped_on_its_own() {
        // The behaviour this change is for. His `home` layout is broken and his
        // `agentic-layout` is correct, and only the broken one is dropped.
        let (out, code) = rendered(MIKES_BROKEN_CONFIG);
        assert_eq!(code, 1, "a skipped layout must fail the check:\n{}", out);
        assert!(out.contains("home  SKIPPED"), "{}", out);
        assert!(out.contains("agentic-layout  usable"), "{}", out);
    }

    #[test]
    fn mikes_working_layout_is_the_one_that_would_be_applied() {
        // Previously the whole file died and the built-in layout applied, which looked
        // like it had worked. Now the layout he actually uses is chosen.
        let (out, _) = rendered(MIKES_BROKEN_CONFIG);
        assert!(
            out.contains("Layout \"agentic-layout\""),
            "his own layout must be the one chosen:\n{}",
            out
        );
        assert!(
            !out.contains("Layout \"built-in\""),
            "the built-in layout must not be reached:\n{}",
            out
        );
        // And it really is his, not a coincidence of names.
        assert!(out.contains("pane 2 keeps 0.7"), "{}", out);
    }

    #[test]
    fn a_broken_layout_does_not_stop_the_file_being_used() {
        let (out, _) = rendered(MIKES_BROKEN_CONFIG);
        assert!(!out.contains("NOT USED"), "{}", out);
        assert!(
            out.contains("skipped on its own. The others still work"),
            "the reader has to be told the rule:\n{}",
            out
        );
    }

    #[test]
    fn every_problem_is_reported_in_one_run() {
        // Returning at the first problem meant somebody fixing three mistakes learned
        // about one per run.
        let (out, _) = rendered(
            "default = \"a\"\n\
             [[layouts.a.tabs]]\nname = \"t\"\n\
             [[layouts.a.tabs.panes]]\nsplit = \"right\"\n\
             [[layouts.b.tabs]]\nname = \"u\"\n\
             [[layouts.b.tabs.panes]]\nagent = \"claude\"\n\
             [[layouts.b.tabs.panes]]\ncommand = \"x\"\n\
             [[layouts.c.tabs]]\nname = \"\"\n\
             [[layouts.c.tabs.panes]]\nagent = \"claude\"\n",
        );
        assert!(out.contains("3 problems found"), "{}", out);
        assert!(out.contains("a  SKIPPED"), "{}", out);
        assert!(out.contains("b  SKIPPED"), "{}", out);
        assert!(out.contains("c  SKIPPED"), "{}", out);
    }

    #[test]
    fn a_chosen_layout_that_is_broken_falls_back_to_the_built_in_one() {
        // A workspace with no layout at all is worse than one with the default, so the
        // run still produces something. The message names which layout failed.
        let (out, code) = rendered(
            "default = \"broken\"\n\
             [[layouts.broken.tabs]]\nname = \"t\"\n\
             [[layouts.broken.tabs.panes]]\nsplit = \"right\"\n\
             [[layouts.fine.tabs]]\nname = \"u\"\n\
             [[layouts.fine.tabs.panes]]\nagent = \"claude\"\n",
        );
        assert_eq!(code, 1, "{}", out);
        assert!(out.contains("Layout \"built-in\""), "{}", out);
        assert!(
            out.contains("layout \"broken\" cannot be built"),
            "the fallback must name the layout that failed:\n{}",
            out
        );
    }

    #[test]
    fn a_syntax_error_still_discards_the_whole_file() {
        // The distinction that keeps this coherent. A file that will not parse has no
        // layouts to salvage, and Herdr treats its own config the same way.
        let (out, code) = rendered("default = \"broken\nthis is not toml [[[\n");
        assert_eq!(code, 1, "{}", out);
        assert!(out.contains("NOT USED"), "{}", out);
        assert!(out.contains("could not be parsed"), "{}", out);
        assert!(out.contains("NOTHING in it applies"), "{}", out);
    }

    #[test]
    fn mikes_broken_config_names_the_fix_for_the_first_pane() {
        let (out, _) = rendered(MIKES_BROKEN_CONFIG);
        assert!(out.contains("write TWO pane blocks"), "{}", out);
        assert!(out.contains("empty first one"), "{}", out);
    }

    #[test]
    fn mikes_misplaced_path_key_is_pointed_at_projects() {
        // He put `path = "~"` on a tab, meaning "use this layout in my home directory".
        // `path` is a real key, just in [[projects]], so naming its home is a fact.
        let (out, _) = rendered(MIKES_BROKEN_CONFIG);
        assert!(out.contains("unknown key"), "{}", out);
        assert!(out.contains("is a [[projects]] key"), "{}", out);
    }

    #[test]
    fn a_good_config_passes_and_describes_what_it_would_build() {
        let (out, code) = rendered(
            "default = \"one\"\n\
             [[layouts.one.tabs]]\nname = \"agent\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\nlabel = \"agent\"\n\
             [[layouts.one.tabs.panes]]\nsplit = \"right\"\nratio = 0.5\ncommand = \"lazygit\"\n",
        );
        assert_eq!(code, 0, "{}", out);
        assert!(out.contains("No problems found."), "{}", out);
        assert!(out.contains("tab \"agent\""), "{}", out);
        assert!(out.contains("runs the claude agent"), "{}", out);
        assert!(out.contains("runs \"lazygit\""), "{}", out);
        assert!(out.contains("labelled \"agent\""), "{}", out);
    }

    #[test]
    fn the_report_distinguishes_an_absent_label_from_an_empty_one() {
        // The distinction the whole release turned on, so the check has to show it.
        let (out, _) = rendered(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nlabel = \"\"\n",
        );
        assert!(out.contains("labelled with an empty string"), "{}", out);

        let (out, _) = rendered(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
        );
        assert!(out.contains("not labelled"), "{}", out);
    }

    #[test]
    fn the_report_says_which_pane_a_ratio_applies_to() {
        // The silent-bug risk in the schema: `ratio` on pane 3 sizes pane 2. Printing
        // "pane 2 keeps 0.7" is the check earning its keep, because a reader who had it
        // backwards sees it here rather than in a mis-sized workspace.
        let (out, _) = rendered(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\n\
             [[layouts.one.tabs.panes]]\nsplit = \"right\"\nratio = 0.5\n\
             [[layouts.one.tabs.panes]]\nsplit = \"down\"\nratio = 0.7\n",
        );
        assert!(out.contains("pane 2 keeps 0.7"), "{}", out);
        assert!(out.contains("pane 1 keeps 0.5"), "{}", out);
    }

    #[test]
    fn an_unset_ratio_shows_the_number_that_will_actually_be_applied() {
        // It used to say "Herdr chooses", which was true when the ratio could be left
        // off the wire. A split node requires one, so 0.5 goes out — Herdr's own default,
        // measured. Someone checking a config wants the value, and being told a choice is
        // pending would send them looking for a setting that does not exist.
        let (out, _) = rendered(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\n\
             [[layouts.one.tabs.panes]]\nsplit = \"down\"\n",
        );
        assert!(
            out.contains("ratio unset so pane 1 keeps 0.5, Herdr's default"),
            "{}",
            out
        );
    }

    #[test]
    fn an_unknown_key_alone_is_reported_but_does_not_fail_the_check() {
        // The rest of the file still applies, so this is a warning rather than a
        // failure. Failing here would train people to ignore the exit code.
        let (out, code) = rendered(
            "default = \"one\"\ncolour = \"purple\"\n\
             [[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
        );
        assert_eq!(code, 0, "{}", out);
        assert!(out.contains("unknown key"), "{}", out);
        assert!(!out.contains("whole file was discarded"), "{}", out);
    }

    #[test]
    fn a_syntax_error_fails_the_check_and_says_the_file_is_not_used() {
        let (out, code) = rendered("default = \"broken\nthis is not toml [[[\n");
        assert_eq!(code, 1, "{}", out);
        assert!(out.contains("NOT USED"), "{}", out);
    }

    #[test]
    fn the_check_says_it_changed_nothing() {
        // Its whole value is being safe to run against a config you already suspect, so
        // it says so where the reader will see it.
        let (out, _) = rendered(
            "default = \"one\"\n[[layouts.one.tabs]]\nname = \"t\"\n\
             [[layouts.one.tabs.panes]]\nagent = \"claude\"\n",
        );
        assert!(
            out.contains("opens no socket and creates no pane"),
            "{}",
            out
        );
    }
}
