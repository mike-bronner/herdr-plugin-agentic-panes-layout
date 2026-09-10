# `agent-layout.toml`

The whole configuration surface of this plugin. One flat TOML file, in Herdr's
own config root, beside Herdr's `config.toml`:

```
~/.config/herdr/agent-layout.toml
```

The file is optional. With no file at all, the built-in layout applies and the
plugin behaves as a fresh install should.

## Where the file is looked for

The config root is **derived, never hardcoded**, in this order:

1. `HERDR_CONFIG_PATH`, which Herdr documents as the override for its own
   `config.toml`. A directory is used as-is; a file's parent directory is used.
2. `HERDR_PLUGIN_CONFIG_DIR`, which Herdr injects into plugin processes as
   `<root>/plugins/config/<plugin id>`, so walking up three levels gives the
   root.
3. `$XDG_CONFIG_HOME/herdr`, or `$XDG_CONFIG_HOME/herdr-dev` when only that one
   exists.
4. `~/.config/herdr`, with the same `herdr-dev` fallback.

Two independent reasons a hardcoded `~/.config/herdr` would be wrong:
`XDG_CONFIG_HOME` relocates the whole root, and a **debug build of Herdr uses
`herdr-dev`** as the directory name. Step 2 covers the debug case for free,
because the value Herdr injects already points inside the right directory.

## Why the file sits beside Herdr's own config

Measured on Herdr 0.9.0 in an isolated `XDG_CONFIG_HOME`: an unrelated sibling
file next to `config.toml` is **completely ignored**.

```
$ ls /private/tmp/hcfg2/herdr/
agent-layout.toml   config.toml
$ herdr config check
config: ok        # exit 0, no warning
```

A section added *inside* `config.toml` is also ignored, but then
`herdr config check` reports `config: issues found` forever, which destroys that
command as a health check. That is why the flat sibling file won.

The file is named for the **concern** rather than for this plugin, so
`herdr-plugin-project-finder` can read the same file later. Those two plugins
deliberately share vocabulary and currently have to duplicate values.

## The schema

```toml
default = "three-pane"

[[projects]]
path = "/Users/you/Developer/some-repo"
layout = "solo"

[[layouts.three-pane.tabs]]
name = "agent"

[[layouts.three-pane.tabs.panes]]
agent = "claude"
label = "agent"

[[layouts.three-pane.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"
label = "lazygit"

[[layouts.three-pane.tabs.panes]]
split = "down"
ratio = 0.6
label = "shell"
```

### Top level

| Key | Meaning |
|---|---|
| `default` | The layout to apply when no `[[projects]]` rule matches. |
| `[[projects]]` | Per-project rules, in file order. |
| `[layouts.<name>]` | A named layout. |

### A pane

| Key | Optional | Meaning |
|---|---|---|
| `agent` | yes | Start an agent of this kind in the pane, any kind Herdr's `agent.start` accepts. |
| `command` | yes | One command line, typed into the pane's own shell and executed. |
| `split` | first pane: forbidden; later panes: **required** | `right` or `down`. |
| `ratio` | yes | The share the pane **being split** keeps. |
| `label` | yes | Rename the pane. **Omit the key and no rename happens at all.** |

**`label` is the reason this release exists.** v0.2.0 always wrote three pane
labels whether the user wanted them or not. Omitting the key now sends no
`pane.rename` request. `label = ""` is a different thing: it is a rename to the
empty string. The two are deliberately not interchangeable, and there is a test
holding them apart.

### Two panes means two pane blocks

The commonest mistake, and the natural misreading of the schema. To get two panes side
by side, write **two** `[[...tabs.panes]]` blocks:

```toml
[[layouts.two.tabs]]
name = "work"

# The tab's own pane. It already exists, so this block can be empty.
[[layouts.two.tabs.panes]]

# The second pane, split out of the first.
[[layouts.two.tabs.panes]]
split = "right"
ratio = 0.5
```

**One block carrying `split = "right"` is rejected**, because the first block is the
tab's own pane and nothing splits it into existence. One block describes one pane, not
one division.

### How splits are read

Each pane after the first carries `split` and optionally `ratio`, meaning:

> Split the **previous** pane in this direction, and the previous pane keeps
> `ratio`.

So the example above splits the tab's own pane right at 0.5, then splits the
pane that made at 0.6 downward. That is exactly what v0.2.0 did with its
`AGENT_LAYOUT_RATIO` then `AGENT_LAYOUT_TOOL_RATIO` pair, and both of those had
the same "share kept by the pane being split" meaning.

Splits target the previous pane and nothing else. There is no way to name an
arbitrary pane to split from, because nothing has asked for one.

**`ratio` is optional and is not invented when absent.** Herdr's `ratio`
parameter is nullable, so leaving it out lets Herdr choose. Substituting `0.5`
here would silently override that choice.

**Both ratios name the share kept by the pane being split**, so a bigger number
always means a bigger original pane. Herdr does not document this and a 0.5
default cannot show it, so it was measured: `split right --ratio 0.7` leaves the
original pane 66 of 94 columns, and the new pane gets `1 - ratio`.

### Directions

`right` and `down`, and nothing else. That is Herdr's own `SplitDirection` enum,
read from `herdr api schema --json` at protocol 22. There are no
`vertical`/`horizontal` aliases, and inventing one here would produce a config
that validates and then fails at the socket **halfway through building a tab**.
An unknown direction is therefore refused at parse time, not at apply time.

### Per-project rules

```toml
[[projects]]
path = "/Users/you/Developer/some-repo"
layout = "solo"
```

One key, `path`, plus the layout to use. Deliberately not `repo`, `repo_name` or
`repo_path`: one key that is matched against several things beats three keys the
reader has to choose between.

`path` is matched against **both** the workspace's checkout path and its repo
root. So naming a repository's root covers every worktree of it, while naming a
specific worktree targets only that one. This matters because this plugin's main
trigger is `worktree.created`, and **a worktree does not live under its repo's
path** — matching only the checkout path would make a repo-wide rule silently
miss nearly every workspace this plugin sees.

Rules are evaluated in **file order, first match wins**. There are no globs and
no prefix matching: `/repos` does not match `/repos/thing`, and `/repos/*` is
taken literally. Neither was asked for, and a prefix rule would silently capture
every sibling repository under a shared parent.

**A plain directory works too.** A rule is matched against the workspace's **own
directory** as well as its checkout path and repo root, so a home directory can have a
layout:

```toml
[[projects]]
path = "/Users/you"
layout = "two-shells"
```

Note what this does **not** do: it matches that directory exactly, not everything under
it. There is still no prefix matching.

**When two rules both match, file order decides — not specificity.** Inside a
repository the candidates include both the directory you are in and the repository
root, so a rule naming a subdirectory and a rule naming the repository can both match at
once. The earlier rule in the file wins, even if the later one looks more specific. Put
narrower rules first.

`--check` prints the candidate list, so you can see exactly what a rule has to match.

Where the candidate paths come from, in order of how reliably they are there:

- **The workspace's own working directory.** Always available, which is what makes a
  plain directory matchable at all.
- **`checkout_path` and `repo_root`**, from `herdr workspace list`'s `worktree` object.
  Present **only for spaces Herdr created through its worktree path**. A plain
  workspace on a git repo has no such object, and `is_linked_worktree: false` does not
  mean "not a worktree space".
- **`git -C <cwd> rev-parse --show-toplevel`** and `--git-common-dir`, which fill the
  gap when that object is absent.

## How problems reach you

Three ways, deliberately, because the obvious one turned out not to work.

**A popup pane, listing every problem.** Any diagnostic opens it, on the automatic
`worktree.created` path as well as an explicit invocation. A broken config should never
be invisible, and that outranks the cost of a popup you did not ask for. Any key
dismisses it, since there is nothing to decide.

It opens **after** the layout is applied, and nothing waits for it. A config problem is
a warning, and gating pane creation on a dialog would turn it into a stall.

**stderr, always.** Every diagnostic goes there whatever else renders, which is what
`herdr plugin log list --plugin mikebronner.agentic-panes-layout` keeps. It is the
record that survives.

**One toast, at most.** A nudge for anyone running `ui.toast.delivery = "herdr"`; the
popup carries the detail.

### Why a toast was not enough

Measured on Herdr 0.9.0, in one isolated server with `ui.toast.delivery = "system"`:

- `notification.show` answered `{"shown": false, "reason": "no_foreground_client"}` for
  every call.
- `plugin.pane.open` in the same server started the pane's process.

So config diagnostics sent as toasts were being dropped before they rendered. A pane is
a different mechanism and does not consult the toast settings.

Three further limits make a toast the wrong shape even where it does render. There is
**no severity** — Herdr hardcodes every API-originated notification to one kind, so a
plugin cannot make an error look like an error. **Only one toast is live at a time**,
and the next answers `Busy`. And there is a **rate limit**. This plugin used to send one
toast per diagnostic, so at best the first ever appeared.

## Checking a file before you rely on it

```sh
bin/agent-layout --check
```

It prints the file it read, every problem it found, which layout would be chosen and
why, and the tabs and panes that layout would build. It exits non-zero when the file
exists and **would not be used as written**.

**It touches nothing.** No socket is opened, no workspace is listed, no pane is made.
It is safe to run against a config you already suspect, which is the point. Note that
a real run resolves the workspace *before* it reads the config, so a real run cannot
tell you anything about a config without a server; `--check` deliberately does not
share that order.

**Why this exists at all: a bad config is invisible in use.** A file that fails to
parse falls back to the built-in layout, which still produces a plausible three-pane
workspace. So the symptoms are a diagnostic nobody was watching for and a layout that
is subtly not the one you wrote — a ratio of 0.6 instead of yours, and no pane labels.
Before `--check`, finding out meant creating a worktree and studying the result.

Two things in the output are worth knowing about:

- **It names which pane a `ratio` sizes.** A line reading `pane 2 keeps 0.7` is the
  check earning its keep, because the ratio is written on pane 3 and applies to pane 2.
  A reader who had that backwards sees it here rather than in a mis-sized workspace.
- **It distinguishes `not labelled` from `labelled with an empty string`.** Those are
  different requests and only one of them sends a rename.

`--check` matches `[[projects]]` rules against the directory you run it from. If that
directory is not a git repository it says so, and only the top-level `default` can
apply there.

## How a bad file fails

**Nothing about config is ever fatal**, and how much is lost depends on *where* the
problem is. There are three tiers.

**A file that will not parse is lost whole.** A TOML syntax error means there are no
layouts to salvage, so the built-in layout applies. Herdr treats its own config the
same way.

**A layout with a problem is skipped on its own.** The file parsed, so every other
layout is intact and keeps working. Only the broken one is dropped.

**Anything smaller is a warning.** An unknown key is ignored and the rest of the block
still applies.

| What is wrong | What is lost |
|---|---|
| No file | Nothing. The built-in layout applies, silently. This is the normal fresh install. |
| Unknown key or section | **Just that key.** Reported, and where the key is real but in the wrong table, the message names its proper home. |
| TOML syntax error | **The whole file.** Reported. Built-in layout. |
| Unreadable file, for example bad permissions | **The whole file.** Reported. Built-in layout. |
| Invalid `split` direction | **The whole file**, because it is a parse failure rather than a structural one. Reported. Built-in layout. |
| A first pane carrying a `split` | **That layout only.** Reported, with the fix. |
| A later pane with no `split` | **That layout only.** Reported, with the fix. |
| A layout with no tabs | **That layout only.** Reported, with the fix. |
| A tab with no name, or with no panes | **That layout only.** Reported, with the fix. |
| A `[[projects]]` entry with an empty `path` | **That rule only.** Reported. It could never match anything anyway. |
| `default` names a layout that does not exist | Reported. Built-in layout applies for this run. |
| `default` names a layout that was skipped | Reported, naming which layout failed. Built-in layout applies for this run. |
| No `default` and no matching rule | Reported. Built-in layout applies for this run. |

**Every problem in the file is reported in one run**, not the first one only. Fixing
three mistakes takes one `--check`, not three.

**When the layout that would have been chosen is itself the broken one**, the built-in
layout applies and the message names which layout was unusable. A workspace with no
layout at all is worse than a workspace with the default, since the point of the plugin
is that a new worktree arrives usable.

Each message names **the fix**, not only the rule. Two of them are mistakes where the
natural reading of the schema is the wrong one, so the rule alone is no help.

**Nothing in this table exits non-zero.** A syntax error in a file the user owns
must not leave a half-built workspace or a dead hook. Falling back to the
built-in layout and saying so out loud is the whole policy.

Note that the last three are structural rather than cosmetic: a pane with no
direction cannot be created at all, so it cannot be skipped the way an unknown
key can. Refusing the file before touching the workspace is what keeps a
half-built tab from happening.

## The built-in layout

With no config file, the built-in layout reproduces v0.2.0's geometry:

```
+-----------------+----------+
|                 | lazygit  |   agent    half the width
|      agent      |          |   lazygit  60% of the remaining column
|                 +----------+   shell    the other 40%
|                 | shell    |
+-----------------+----------+
```

Written as a config file, it is exactly this:

```toml
default = "three-pane"

[[layouts.three-pane.tabs]]
name = "agent"

[[layouts.three-pane.tabs.panes]]
agent = "claude"

[[layouts.three-pane.tabs.panes]]
split = "right"
ratio = 0.5
command = "lazygit"

[[layouts.three-pane.tabs.panes]]
split = "down"
ratio = 0.6
```

**The one deliberate difference from v0.2.0 is that no pane is labelled.** Labels
became opt-in, which is the change this release is for, so a default that still
imposed three of them would not have made the change at all. Copy the block
above and add `label` keys to get v0.2.0's behaviour back exactly.

The default lives as a Rust literal rather than an embedded TOML string, so it
cannot fail to parse at startup. A test asserts that the block above parses to
the same value as that literal, which is what stops this section from rotting.

## What was removed

| Gone | Why |
|---|---|
| The `.env` in the plugin config directory | The config moved to a different directory. Carrying two readers doubles the config surface for a one-user plugin, so there is **no fallback**. |
| Every `AGENT_LAYOUT_*` variable | Each was a single setting on a fixed shape. The shape itself is now configurable, which is what they were approximating. |
| `AGENT_LAYOUT_RECIPE` | Never used, and a declarative config is the thing an escape-hatch script was a workaround for. |

## What was considered and deliberately not built

**A project-local layout file read from inside a repository.** It would execute
`command` lines from a file a cloned repo could ship, and that needs a trust
model nobody has designed. Deferred rather than rejected.

**Glob patterns in `[[projects]]` paths.** Not asked for. Exact matching against
two paths already covers the repo-and-its-worktrees case that motivated the
feature.
