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
   logs it while a design question is settled. That question, and the condition
   that resolves it, are in [`docs/open-questions.md`](docs/open-questions.md).
2. Is the new workspace the focused one? `herdr worktree create --no-focus`
   emits `worktree.created` with `"focused": false`, and laying out an unfocused
   workspace would start an agent in whatever you were looking at instead.

If both answers are yes it hands off to `bin/agent-layout` with no arguments,
which applies the recipe to the focused workspace: rename the tab, split the
single pane in two, split the new pane again, run lazygit in the upper one,
label all three, and start the agent in the original pane.

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

## Arguments

With **no arguments**, `bin/agent-layout` does exactly what the two sections
above describe: the focused workspace, an agent name derived from that
workspace's label, and an agent started. The event hook execs it that way and the
keybinding presses it that way, so that behaviour is fixed.

Two optional flags exist for a third caller, another program running this file
as an executable:

```sh
bin/agent-layout [--workspace <id>] [--agent-name <name>]
```

`--workspace <id>` lays out the workspace with that id rather than the focused
one. "Which workspace is focused?" is the right question for the hook and the
keybinding, which both mean the workspace you are looking at, and the wrong one
for a caller that opens several workspaces unfocused and picks a focus target at
the end. Nothing else changes: the tab, the label and the working directory are
all read off the workspace named here.

`--agent-name <name>` starts the agent under that exact name, instead of one
derived from the workspace label. The name reaches Herdr **verbatim**, so it is
not lowercased, not prefixed and not truncated. A caller that reserves distinct
names across a batch of workspaces does so precisely so that
`herdr agent prompt <name>` reaches each one, and re-deriving the string here
would break the guarantee the reservation was made for. An invalid name is
Herdr's to reject, and the refusal is reported as a toast.

There is no flag to skip the agent. The first pane always holds the preferred
agent. A caller that must not block on `agent start` detaches the whole
`bin/agent-layout` call and passes `--agent-name`, which gets the panes, the
reserved name and no blocking wait together.

These are per-call arguments and deliberately **not** settings, so neither has an
`AGENT_LAYOUT_` equivalent. Those settings are one shared vocabulary, so a value
means the same thing wherever it is read. A per-call argument hiding among them
would let a stray `export` in an interactive shell silently retarget a keybinding
press.

Every argument error is fatal, and is reported before anything is read or
changed: an unknown flag, a flag missing its value, a flag given an empty value,
and the same flag given twice. A `--workspace` id that matches no workspace is
fatal too, rather than falling back to the focused one. That fallback would lay
out whatever you happened to be looking at, which is the accident the flag exists
to prevent.

The "already laid out?" guard applies either way. It reads the pane count of the
workspace being laid out, so a second run against the same target is a no-op
whichever way it was reached.

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

## Documentation

The scripts and the manifest carry no comments. Everything that would have been
one lives in `docs/`:

- [`docs/design.md`](docs/design.md) — why the code in `bin/` is shaped as it is.
  The entry paths, the arguments, the guard, the fatal versus non-fatal split,
  and the settings contract.
- [`docs/herdr-behaviour.md`](docs/herdr-behaviour.md) — Herdr behaviour this
  plugin depends on that Herdr does not document, such as which side `--ratio`
  sizes. Every entry is dated and attributed. **Read the measurement method
  there before probing anything**: measure in an isolated server, never in the
  live one, or a probe split will rearrange the workspaces you are working in.
- [`docs/open-questions.md`](docs/open-questions.md) — decisions deliberately not
  taken yet, each with the condition that resolves it. The `worktree.opened`
  subscription is the open one.

Every measurement recorded there was taken against Herdr 0.8.2. Treat each as
"true of 0.8.2" rather than "true today".
