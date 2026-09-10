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

## RESOLVED: the layout engine is built on `layout.apply`

**Status: resolved 2026-09-10. Done.** One tab is now one `layout.apply` call. The
sequential `tab.create` / `tab.rename` / `pane.split` / `pane.close` engine is gone,
and so is the previous-pane-only split limitation, because a tree nests arbitrarily.

Kept rather than deleted, because one requirement was dropped to get here and a reader
who does not know that will try to put it back.

### What blocked it, and how the block was removed

Three of this plugin's four paths were already served. The fourth was not:

| Path | Served by `layout.apply`? |
| --- | --- |
| Initial layout on `worktree.created`: a fresh tab, nothing to preserve | **Yes**, in one call. This is the common case. |
| The `y` answer: destroy the tab and rebuild it clean | **Yes**, in one call. That is precisely what it does. |
| The `esc` answer: change nothing | Not applicable, no engine needed. |
| The `n` answer: keep the running agent and build the layout around it | **No**, and it cannot be. |

The reason was specific and measured: `pane_id` on a pane leaf is output-only, so an
existing pane cannot be placed into a tree, and the agent's pane is exactly what `n`
had to place.

**Mike removed the blocker by removing the requirement behind it**, on the grounds
that new panes are being made anyway. `n` is gone as a distinct answer. The
confirmation now offers two: `y` replaces the tab, and every other outcome — `esc`,
`n`, a dismissal, a timeout, a popup that would not open, an answer nobody recognises
— changes nothing at all. `n` is deliberately still bound, to `DO_NOTHING`, so that
somebody with the old habit does not destroy a pane by pressing it.

### The deciding question, and what the measurement said

**What does `layout.apply` do with a pane leaf's `command`?** It was the question that
had to be answered first, and the answer is that the field is unusable here: it execs
raw argv against the **server's** `PATH`, which under launchd is
`/usr/bin:/bin:/usr/sbin:/sbin`. A bare `lazygit` would not resolve, and a whole
command line in one element is looked up as a single executable name.

So the engine does not use it. **A leaf carries `cwd` and nothing else**, which spawns
the user's own interactive login shell, and `pane.send_input` types the command into
that — preserving their `PATH`, their rc files and unquoted flags. `agent.start` goes
to the same ids, from the same response.

`label` is left off leaves for a second measured reason: an empty label on a leaf comes
back as `null`, indistinguishable from an absent one, and this plugin's labelling
contract turns on telling those apart. Labels still go through `pane.rename`.

### Three more measurements the rewrite needed

All on 0.9.0, recorded in full in [`herdr-behaviour.md`](herdr-behaviour.md):

- **`ratio` on a split node is required and not nullable**, unlike `pane.split`'s.
  Omitting it is `invalid_request`, and `null` is `expected f32`. The config's promise
  that an absent ratio is not invented is kept in effect by sending **0.5**, which is
  measurably the number Herdr itself picks.
- **`focus: false` is a real request when a tab is added** and a no-op when replacing a
  workspace's only tab, which comes back focused either way.
- **`layout.export` is the exact inverse.** A tab the old sequential engine built
  exports as precisely the tree this engine constructs, which is how the flat config's
  nesting was confirmed rather than assumed.

### What the tab scope is, and why it is not a limitation

**Mike's decision, in his words: only destroy the tabs named in the layout.** Taken
after being shown the consequence of a wider scope, which is that one keypress would
destroy an unrelated tab. A one-tab layout touches one tab; a tab the layout does not
name survives every rebuild. `panes_of_another_tab_are_not_destroyed_by_a_rebuild`
pins it, and it is a chosen boundary rather than a conservative default somebody
settled on.

### What would bring the third answer back

`layout.apply` gaining a way to **place an existing pane** into a tree, making
`pane_id` an input rather than output. That, or a way to preserve or refuse on
agent-bearing panes, is what "keep the agent and build around it" would need. Nothing
short of it will do: wrapping, re-splitting and `sh -c` were all considered and none
of them carries a live agent across.

---

## Herdr version drift

Every measurement recorded in this repo was taken against Herdr **0.8.2**. Herdr
on this machine is now **0.9.0**.

Nothing has been re-taken. The measurements are dated and attributed on purpose,
so that a reader can tell evidence from assumption. Re-measuring is worth doing,
but it is a deliberate exercise with the isolated-server method rather than
something to do in passing.
