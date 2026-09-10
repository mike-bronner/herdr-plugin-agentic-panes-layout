# Open questions

Decisions this repo has deliberately not taken yet, and the ones it has taken since.
Each carries the measurement behind it and the condition that resolves it.

A resolved question is kept rather than deleted when the risk it guarded against was
real. The reasoning is what stops somebody undoing it later without knowing what it
cost to settle.

---

## RESOLVED: `worktree.opened` lays out the workspace

**Status: resolved 2026-09-10, by branch 1 of the exit condition below.** Kept here
rather than deleted, because the risk it was guarding against is real and the reason it
does not apply is a measurement somebody should be able to find.

Mike's decision, in his words: opening a workspace should lay it out, because that is
what the plugin is for. Both events now act:

| Event | Role |
| --- | --- |
| `worktree.created` | **Acts.** Runs the layout. |
| `worktree.opened` | **Acts.** Runs the layout. |

### The exit condition it met

The condition set on 2026-09-05 had three branches. Branch 1 was: `worktree.opened`
logged with `already_open=false` means promote it, because nothing else lays that
workspace out. Mike's log showed three `worktree.opened` events in a day, all doing
nothing, and he resolved it directly rather than waiting for more evidence.

### The race that made a second acting subscription dangerous, and why it does not apply

The old note argued that two **acting** subscriptions would be actively wrong: the
events fire about a millisecond apart, Herdr does not serialize hooks, and two
concurrent runs would both see one free tab and **both build it**. The guard makes a
*later* run safe, not a *simultaneous* one. That reasoning still stands.

It does not apply because **the two events are disjoint per action**, measured on 0.9.0
rather than assumed:

| Action | Events emitted |
| --- | --- |
| `herdr worktree create` | `worktree.created` only |
| `herdr worktree create --focus` | `worktree.created` only |
| `herdr worktree open` on a closed worktree | `worktree.opened` only, `already_open=false` |
| `herdr worktree open` on an open one | `worktree.opened` only, `already_open=true` |

A probe plugin subscribing to both, logging each with a timestamp, saw exactly one
event per action in every case. So one action cannot produce two concurrent runs
racing to split the same pane. The old note asserted this ("open emits no
`worktree.created`"); it is now measured.

**Reopening an already-open workspace is a no-op**, confirmed rather than assumed. The
tab-name guard skips a tab whose name is already there, and the event path takes the
skip branch rather than the rebuild branch.

### One consequence worth knowing

`herdr worktree open` from the CLI reports `focused: false` in its payload, and the
workspace really is not focused afterwards. `worktree open` has no `--focus` flag.
Since the focused gate is kept on both acting paths — laying out an unfocused workspace
starts an agent in whatever the user is looking at — **a CLI `worktree open` will not
lay out**. Whether Herdr's TUI focuses on open is unmeasured, because it needs an
attached client. If opening through the TUI turns out not to focus either, this
resolution has no effect in practice and the gate is what to revisit, not the
subscription.

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

That reasoning is why a second acting subscription needed a measurement before it was
allowed, rather than an assumption. It got one: see the resolved section above, where
the two worktree events are shown to be disjoint per action.

`workspace.created` and `workspace.focused` stay gone. Nothing has measured them as
disjoint from anything, and the cost argument alone still rules them out.

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
