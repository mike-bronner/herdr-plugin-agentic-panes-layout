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

Where the paths come from: `herdr workspace list` returns a `worktree` object
carrying `checkout_path` and `repo_root`, but **only for spaces Herdr created
through its worktree path**. A plain workspace on a git repo has no such object,
and `is_linked_worktree: false` does not mean "not a worktree space". So the
paths fall back to `git -C <pane cwd> rev-parse --show-toplevel` and
`--git-common-dir`.

## How a bad file fails

Herdr's own config handling is matched deliberately, because this file sits in
the same directory. Herdr uses `toml` with `serde_ignored`: an unknown key
produces a diagnostic and the rest of the file still applies, while a syntax
error discards the whole file and falls back to defaults. Nothing about config is
ever fatal.

| What is wrong | What happens |
|---|---|
| No file | The built-in layout applies. Silent — this is the normal fresh install. |
| Unknown key or section | Reported. **The rest of the file still applies.** |
| TOML syntax error | Reported. The whole file is discarded and the built-in layout applies. |
| Unreadable file, for example bad permissions | Reported. Built-in layout. |
| `default` names a layout nothing defines | Reported. Built-in layout. |
| No `default` and no matching rule | Reported. Built-in layout. |
| Invalid `split` direction | Reported as a parse failure. Built-in layout. |
| A later pane with no `split` | Reported. Built-in layout. |
| A first pane carrying a `split` | Reported. Built-in layout. |

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
