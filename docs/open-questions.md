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

## Herdr version drift

Every measurement recorded in this repo was taken against Herdr **0.8.2**. Herdr
on this machine is now **0.9.0**.

Nothing has been re-taken. The measurements are dated and attributed on purpose,
so that a reader can tell evidence from assumption. Re-measuring is worth doing,
but it is a deliberate exercise with the isolated-server method rather than
something to do in passing.
