# Agentic Panes Layout — a Herdr plugin

Lays out every new git worktree workspace in [Herdr](https://herdr.dev), the
agent-aware terminal multiplexer, the moment it is created: the tab renamed
`agent`, and three labelled panes.

```
+-----------------+----------+
|                 | lazygit  |   agent    half the width
|      agent      |          |   lazygit  60% of the remaining column
|                 +----------+   shell    the other 40%
|                 | shell    |
+-----------------+----------+
```

Every pane carries a label, and every proportion and command is a setting.

## Install

```sh
herdr plugin install mike-bronner/herdr-plugin-agentic-panes-layout
```

To work on the plugin instead, clone it and link the working copy by absolute
path:

```sh
git clone git@github.com:mike-bronner/herdr-plugin-agentic-panes-layout.git
herdr plugin link /absolute/path/to/herdr-plugin-agentic-panes-layout
```

Requires Herdr 0.8.0 or newer. No other dependencies: `/bin/sh` and
`/usr/bin/python3` are both spelled by absolute path, because Herdr's server
runs under launchd with `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and `/opt/homebrew`
is not on it.

## What it does

`bin/on-event` is a `worktree.created` hook. It answers two questions and
nothing else:

1. Is this the event that means a worktree was made? Only `worktree.created` is
   acted on. `worktree.opened` is subscribed as well, but only so that Herdr
   logs it while a design question is settled. See the comments in
   `herdr-plugin.toml`.
2. Is the new workspace the focused one? `herdr worktree create --no-focus`
   emits `worktree.created` with `"focused": false`, and laying out an unfocused
   workspace would start an agent in whatever you were looking at instead.

If both answers are yes it hands off to `bin/agent-layout`, which applies the
recipe to the focused workspace: rename the tab, split the single pane in two,
split the new pane again, run lazygit in the upper one, label all three, and
start the agent in the original pane.

`bin/agent-layout` is safe to run again. It refuses to touch a tab that already
has more than one pane, or whose pane already hosts an agent, so a second run is
a no-op rather than a second layout.

Only the tab rename is fatal on failure, because it happens before any pane is
created and leaves the workspace clean for a retry. Once the panes exist, a
failed lazygit or a failed label is reported as a toast and the run still
succeeds: the guard above means no later run could finish the job, so reporting
a built layout as a failure would help nobody.

## Keybinding

`bin/agent-layout` can also be run on its own, against whichever workspace is
focused. That is useful for a plain repo or a worktree you opened by hand.
Bind it in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+ctrl+l"
type = "shell"
command = "/absolute/path/to/herdr-plugin-agentic-panes-layout/bin/agent-layout"
```

The absolute path is required. A `[[keys.command]]` process is handed
`HERDR_ACTIVE_*`, `HERDR_BIN_PATH`, `HERDR_SOCKET_PATH` and `HERDR_SESSION`, and
no `HERDR_PLUGIN_ROOT` to resolve a relative path against.

## Configure

Settings live in a `.env` file in the plugin config directory:

```sh
herdr plugin config-dir mikebronner.agentic-panes-layout
# /Users/you/.config/herdr/plugins/config/mikebronner.agentic-panes-layout
```

```ini
# agent to start, any kind `herdr agent start --kind` accepts (default: claude)
AGENT_LAYOUT_KIND=claude

# where the lazygit/shell column goes: right or down (default: right)
AGENT_LAYOUT_DIRECTION=right

# the share of the tab the AGENT keeps (default: 0.5)
AGENT_LAYOUT_RATIO=0.5

# what the tab is renamed to (default: agent)
AGENT_LAYOUT_TAB_NAME=agent

# command run in the tool pane (default: lazygit)
AGENT_LAYOUT_TOOL_COMMAND=lazygit

# where the bare shell goes, relative to the tool pane (default: down)
AGENT_LAYOUT_TOOL_DIRECTION=down

# the share of that column the TOOL pane keeps (default: 0.6)
AGENT_LAYOUT_TOOL_RATIO=0.6

# pane labels (defaults: agent, lazygit, shell)
AGENT_LAYOUT_AGENT_LABEL=agent
AGENT_LAYOUT_TOOL_LABEL=lazygit
AGENT_LAYOUT_SHELL_LABEL=shell

# run this script instead of the bundled recipe (default: unset)
AGENT_LAYOUT_RECIPE=
```

Both ratios name the share kept by the pane **being split**, so a bigger number
always means a bigger agent or a bigger tool pane. Herdr does not document this,
and the 0.5 default cannot show it, so it was measured: `split right --ratio 0.7`
leaves the original pane 66 of 94 columns, and the new pane gets `1 - ratio`.

`AGENT_LAYOUT_TOOL_COMMAND` is one command line, sent to the tool pane's own
shell, so flags work without quoting. It is the one setting whose default is a
bare name rather than an absolute path. The absolute-path rule above applies to
what Herdr's launchd server spawns. A pane's shell is interactive and has the
user's own `PATH`, which was measured to resolve `lazygit` to
`/opt/homebrew/bin/lazygit`. A bare name is also the only default that can work
on both declared platforms.

Every key is optional, and every default is the layout described at the top of
this file. Real environment variables win over the file, a comment needs a line
of its own, and a line the plugin cannot parse is skipped rather than failing
the hook.

Values are not validated here. Herdr's own CLI rejects an unknown agent kind, an
unknown direction and a non-numeric ratio with a named error, which the plugin
reports as a toast and records in
`herdr plugin log list --plugin mikebronner.agentic-panes-layout`.

`AGENT_LAYOUT_RECIPE` replaces the layout entirely: point it at your own
executable and `bin/on-event` runs that once the two gates pass. A path that is
not executable is an error, not a silent fall back to the bundled recipe.

## Known limitation

`agent start` sometimes fails with `agent_pane_busy` ("is not an available
shell"). The hook fires within about two milliseconds of the workspace being
created, and the new pane's shell has not always reached its interactive prompt
by then.

Herdr 0.8.2 has no probe for that specific state. There is no `pane wait-shell`,
and a pane reports no `available` field. The closest thing, `pane wait-output`,
waits for text you name, which means guessing the user's shell prompt. So the
plugin neither retries nor sleeps: the right wait is unmeasured, and a made-up
one would only trade a visible failure for an invisible delay. The failure is
reported as a toast, and pressing the keybinding afterwards finishes the layout.

`agent_not_ready` is a different outcome and is treated as success. Herdr
documents it as "the agent is blocked during startup", which means the agent did
start and is waiting at a question. On a fresh worktree that question is Claude
Code's trust-this-folder prompt.

## Tests

```sh
python3 -m unittest discover tests
```

The scripts are run for real as subprocesses, against a stub `herdr` binary and
a stub recipe planted in a temporary directory. Nothing is imported and no pane
is ever split.

One check needs a newer interpreter than the plugin does. The suite parses
`herdr-plugin.toml` for real, because Herdr re-reads that file at dispatch time
and a syntax error in it stops the plugin silently. Parsing needs `tomllib`,
which arrived in Python 3.11, and `/usr/bin/python3` is 3.9. Under 3.9 that one
check is skipped and the run prints a banner saying so, because a green suite
there is not a checked manifest. Run the suite under a 3.11 or newer
interpreter to include it.

## Measuring Herdr behaviour

Several comments in `bin/agent-layout` record behaviour that Herdr does not
document, such as which side `--ratio` sizes. Measure such things in an
**isolated server**, never in the live one, or a probe split will rearrange the
workspaces you are working in.

```sh
rm -rf /private/tmp/hgeo && mkdir -p /private/tmp/hgeo
XDG_CONFIG_HOME=/private/tmp/hgeo herdr --session geo server &
```

`XDG_CONFIG_HOME` moves Herdr's entire config root, so the socket, `plugins.json`
and `session.json` all move together. Pair it with `--session`, because an
ambient `HERDR_SOCKET_PATH` may already point at the live socket. Keep the path
short, under `/private/tmp`, or startup dies on `sun_path` length.

Two near misses look like isolation and are not. `HERDR_SOCKET_PATH` moves only
the socket, so the server restores the **live** `session.json`, runs your real
workspaces in a second process, and writes its state back over yours.
`HERDR_CONFIG_PATH` isolates nothing at all.

Check isolation three ways before probing, because the failure is silent.
`workspace list` must return nothing, `plugin list` must show only what you
linked, and the live `plugins.json` must be byte-identical afterwards by sha256
and mtime.

There is no re-register step. The server re-reads `herdr-plugin.toml` from disk
at dispatch time and never consults `plugins.json` for dispatch, so a manifest
edit takes effect on the very next event with no re-link, restart or
`reload-config`. The cached version string there is cosmetic. This cuts both
ways: a broken or deleted manifest stops all dispatch just as immediately.
