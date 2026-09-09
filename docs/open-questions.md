# Open questions

Decisions this repo has deliberately not taken yet. Each carries the measurement
behind it and the condition that resolves it.

---

## The `worktree.opened` subscription is temporary instrumentation

**Status: unresolved. The exit condition below has not been reached.**

`herdr-plugin.toml` subscribes two events, and they are **not peers**:

| Event | Role |
| --- | --- |
| `worktree.created` | **Acts.** Runs the layout. |
| `worktree.opened` | **Log-only instrumentation.** Deliberately does nothing. |

It is log-only for free. Gate 1 in `bin/on-event` compares against
`worktree.created`, so Herdr records the event and the hook exits before any
layout runs.

**Do not promote it to gate 1 "while you are here."** Promotion is a decision to
take with the plugin log in hand, not a tidy-up alongside another change.

### Why it was added

Added 2026-09-05 to answer one question: what does prefix+ctrl+o
(`open_worktree`) emit?

It exists because of a **known anomaly**. One worktree
(`bible-models/update-for-migrations`, 2026-09-05 10:30:41) appeared with **no
events logged at all**, while the plugin had been armed since 10:19:21 and was
demonstrably logging events either side of it.

The leading hypothesis is that it was never a creation. An `open_worktree` on an
existing checkout emits `worktree.opened`, which was not subscribed at the time,
and unsubscribed is indistinguishable from silent.

The answer is not only diagnostic. Opening a worktree that has no workspace yet
also produces a workspace with one bare pane, which wants the same layout. So
this measurement can turn into a feature.

### What has been measured so far

See [`herdr-behaviour.md`](herdr-behaviour.md) for the raw results. In summary,
measured 2026-09-05 in an isolated server:

- opening a worktree with no workspace emitted `worktree.opened` with
  `already_open=false`, focused, `pane_count` 1
- opening the same worktree again emitted `worktree.opened` with
  `already_open=true` and the same `workspace_id`
- **neither open emitted `worktree.created`**

Because the two events do not co-occur, the double-fire hazard described below
does not apply between them. What is unsettled is the **decision**, not the
mechanics: `already_open=true` must not re-lay-out a workspace, and only real use
shows how often each case occurs.

### Read the log back with

```sh
herdr plugin log list --plugin mikebronner.agentic-panes-layout
```

### Exit condition

This does not stay as it is. Once prefix+ctrl+o has been pressed on a real
worktree and the log has been read, resolve it one of three ways:

1. **`worktree.opened` logged with `already_open=false` → promote it.** Add it to
   gate 1 in `bin/on-event` so it lays out that bare workspace. Nothing else lays
   it out today, because open emits no `worktree.created`. Gate 2 (focused) and
   the recipe's own pane-count guard cover the rest, and that guard is what makes
   an `already_open=true` press a safe no-op.
2. **`worktree.opened` logged only with `already_open=true` → delete the
   subscription.** Those workspaces already exist and are already laid out.
3. **`worktree.opened` never logged → delete the subscription.** The 10:30:41
   anomaly stays unexplained, and the `open_worktree` hypothesis is dead.

Cases 1 and 2 can both appear in the log, because `already_open` reports the
state at the moment of the press. **Case 1 decides it**: it is the only case that
leaves a bare workspace unstyled.

`tests/test_on_event.py` pins the subscription list exactly, so resolving this is
a one-line edit there alongside the manifest change.

---

## Why only one event acts

This is settled, and is recorded here because it is the reasoning that makes the
instrumentation above safe.

`worktree.created` is the only event common to both worktree creation paths, and
the only one that names what happened. See
[`herdr-behaviour.md`](herdr-behaviour.md) for the emission table.

`workspace.created` and `workspace.focused` are **gone, not kept as insurance**.
Two reasons:

**Cost.** `workspace.focused` fired 58 times in a 60-entry sample against one
real worktree creation. That is 58 subprocess spawns a session to reach an early
exit.

**Correctness.** A second **acting** subscription would be actively wrong, not
merely wasteful. Those events fire ~1ms apart, Herdr does not serialize hooks,
and the recipe's "already laid out?" guard reads pane count over the API. Two
concurrent copies would both see one pane and **both would split it**. The guard
makes a *later* run safe, not a *simultaneous* one.

That is why the second subscription is log-only rather than acting. A
subscription that only ever reaches the early exit in gate 1 cannot join that
race, whatever it turns out to fire alongside.

---

## Should the layout engine be rewritten on `layout.apply`?

**Open, and worth doing for most of the engine. Not rejected.**

`layout.apply` builds an entire tab from one recursive tree in a single call. This
plugin instead issues N sequential `pane.split` calls, one `pane.rename` per label,
and one `pane.send_input` per command. It would also remove the previous-pane-only
split limitation entirely, because a tree can nest arbitrarily.

It was measured on 0.9.0, 2026-09-09. The findings are recorded in full in
[`herdr-behaviour.md`](herdr-behaviour.md); in short, with a `tab_id` it replaces the
tab and the tab id changes, it kills a pane's agent unconditionally, and `pane_id` on
a pane leaf is output-only.

### One path of four is blocked, and it is not the common one

Walking the paths this plugin actually has:

| Path | Served by `layout.apply`? |
| --- | --- |
| Initial layout on `worktree.created`: a fresh tab, nothing to preserve | **Yes**, in one call. This is the common case. |
| The `y` answer: destroy the tab and rebuild it clean | **Yes**, in one call. That is precisely what it does. |
| The `esc` answer: change nothing | Not applicable, no engine needed. |
| The `n` answer: keep the running agent and build the layout around it | **No.** |

Only the last one is blocked, and the reason is specific: `pane_id` on a pane leaf is
output-only, so an existing pane cannot be placed into a tree, and the agent's pane is
exactly what has to be placed.

**Destructiveness is explicitly not a blocker.** Replacing a tab wholesale is the
desired behaviour for three of the four paths. It is only a problem for the one that
must preserve something.

Two costs previously recorded here were overstated and are corrected:

- **A changed `tab_id` is harmless.** Verified in the code rather than assumed: the
  guard matches on tab **name** (`tab_named` compares `label`), and `tab_id` is used
  only to select a tab's panes from a list read fresh at the start of each run. No id
  is cached across runs.
- **The 180 existing tests being written against the incremental engine is a cost of
  changing, not an argument against changing.**

### The deciding question, which nobody has measured

**What does `layout.apply` do with a pane leaf's `command`?**

This plugin documents `command` as one line typed into the pane's **already-running
interactive shell**, which is why unquoted flags work and why the user's shell rc
setup applies — `lazygit` resolves because the pane's shell has the user's `PATH`. A
`layout.apply` pane leaf takes an **argv array**. Those are not equivalent.

Whoever picks this up should measure this first, because it decides whether the
mismatch is real or imaginary:

1. Does `layout.apply` **exec the argv directly**, or hand it to a shell?
2. If a shell, is it an **interactive login** shell that sources the user's rc files,
   or a bare non-interactive one?

If it execs argv directly, or spawns a non-interactive shell, then a bare `lazygit`
would stop resolving and `command = "git log --oneline -20"` would need splitting.
Wrapping in `sh -c` does not rescue it: that is a fresh non-interactive shell which has
never sourced the user's rc files. If instead it runs the argv inside an interactive
shell, the mismatch is cosmetic and a one-element argv carries the line unchanged.

`layout.export` is worth using either way: it returns the same tree shape the current
engine builds, so it is a cheap check that a constructed tree is the shape the server
understands.

### What would unblock the fourth path

`layout.apply` gaining a way to **place an existing pane** into a tree, making
`pane_id` an input rather than output. That is the load-bearing one: it would turn
"keep the agent and build around it" from a special case into a normal one, and a
single call would then serve every path. A way to preserve or refuse on agent-bearing
panes would do the same job by a different route.

Until then, a rewrite would keep an incremental fallback for that one branch, which
is a real design question rather than a blocker.

---

## Herdr version drift

Every measurement recorded in this repo was taken against Herdr **0.8.2**. Herdr
on this machine is now **0.9.0**.

Nothing has been re-taken. The measurements are dated and attributed on purpose,
so that a reader can tell evidence from assumption. Re-measuring is worth doing,
but it is a deliberate exercise with the isolated-server method rather than
something to do in passing.
