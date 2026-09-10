pub const MAX_TRIES: u32 = 20;
pub const MAX_LEN: usize = 32;
pub const NOT_READY: &str = "agent_not_ready";
pub const TAKEN: &str = "agent_name_taken";
pub const PANE_BUSY: &str = "agent_pane_busy";

/// How long to wait between attempts at a pane that is not yet at a prompt.
///
/// **250ms is one measured shell startup**, not a round number. Mike's own
/// interactive zsh takes 230-260ms to reach a prompt, measured over seven samples
/// with his real rc, and `mise activate` dominates that. So each attempt costs about
/// one shell startup, and the common case is caught on the second try.
pub const BUSY_WAIT_MS: u64 = 250;

/// How many times to retry a busy pane before giving up.
///
/// 20 attempts is a **5 second** budget, roughly twenty measured shell startups. That
/// absorbs a cold start where the rc is much slower than steady state, which is the
/// case that produced the failure this exists to fix.
///
/// It is bounded for a reason that does not apply to the name retry. A pane occupied
/// by a real editor or a running command **never** becomes free, and no amount of
/// waiting fixes it. An unbounded wait would turn a clear failure into a hung hook.
pub const BUSY_MAX_TRIES: u32 = 20;

pub fn derive(workspace_label: &str) -> String {
    let lowered: String = workspace_label
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = lowered.trim_matches('-');
    if trimmed.is_empty() {
        return "agent".to_string();
    }

    let prefixed = if trimmed.starts_with(|c: char| c.is_ascii_lowercase()) {
        trimmed.to_string()
    } else {
        format!("a{}", trimmed)
    };

    truncate(&prefixed, MAX_LEN)
}

pub fn with_suffix(base: &str, attempt: u32) -> String {
    if attempt <= 1 {
        return base.to_string();
    }
    let suffix = format!("-{}", attempt);
    format!("{}{}", truncate(base, MAX_LEN - suffix.len()), suffix)
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_label_is_lowercased_and_hyphenated() {
        assert_eq!(derive("proj one"), "proj-one");
        assert_eq!(derive("MyRepo"), "myrepo");
    }

    #[test]
    fn a_label_that_would_start_with_a_digit_is_prefixed() {
        // Herdr rejects a name that does not start with a letter, so the `a` is not
        // decoration. The fixture is the real one from the previous suite.
        assert_eq!(derive("9 Bible/Models"), "a9-bible-models");
    }

    #[test]
    fn leading_and_trailing_separators_are_stripped() {
        // Without the strip, "/repos/" becomes "-repos-", which does not start with
        // a letter and would then be prefixed to "a-repos-".
        assert_eq!(derive("/repos/"), "repos");
        assert_eq!(derive("--x--"), "x");
    }

    #[test]
    fn a_label_with_nothing_usable_falls_back_to_agent() {
        // Fail closed on a name Herdr would reject anyway, rather than sending it
        // an empty string.
        assert_eq!(derive(""), "agent");
        assert_eq!(derive("///"), "agent");
        assert_eq!(derive("---"), "agent");
    }

    #[test]
    fn underscores_and_digits_survive_but_other_punctuation_does_not() {
        assert_eq!(derive("keep_this-9"), "keep_this-9");
        assert_eq!(derive("a.b:c"), "a-b-c");
    }

    #[test]
    fn a_derived_name_is_capped_at_thirty_two_characters() {
        assert_eq!(derive(&"a".repeat(40)).len(), MAX_LEN);
    }

    #[test]
    fn the_first_attempt_carries_no_suffix() {
        // The ordinary case must cost nothing: "proj-one-1" would be a different
        // agent name from the one v0.2.0 used.
        assert_eq!(with_suffix("proj-one", 1), "proj-one");
    }

    #[test]
    fn a_later_attempt_trims_the_base_rather_than_the_suffix() {
        // Trimming the suffix would produce a 33-character name Herdr rejects, and
        // would also let two different bases collapse onto one name.
        let base = "a".repeat(32);
        assert_eq!(with_suffix(&base, 2), format!("{}-2", "a".repeat(30)));
        assert_eq!(with_suffix(&base, 2).len(), MAX_LEN);
        assert_eq!(with_suffix(&base, 10), format!("{}-10", "a".repeat(29)));
        assert_eq!(with_suffix(&base, 10).len(), MAX_LEN);
    }

    #[test]
    fn a_short_base_is_not_padded_or_trimmed() {
        assert_eq!(with_suffix("x", 2), "x-2");
    }

    #[test]
    fn a_multibyte_label_is_truncated_on_character_boundaries() {
        // Byte slicing would panic here. Every non-ASCII character becomes one
        // hyphen, so an all-non-ASCII label strips to nothing and falls back.
        assert_eq!(derive("日本語"), "agent");
        assert_eq!(derive("café"), "caf");
    }
}
