# Agentic Panes Layout — a Herdr plugin

Lays out every new git worktree workspace in [Herdr](https://herdr.dev), the
agent-aware terminal multiplexer, the moment it is created: the tab renamed
`agent`, Claude on the left, a bare shell on the right.

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
recipe to the focused workspace: rename the tab, split the single pane, start
the agent in the original half.

`bin/agent-layout` is safe to run again. It refuses to touch a tab that already
has more than one pane, or whose pane already hosts an agent, so a second run is
a no-op rather than a second split.

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

# where the bare shell goes: right or down (default: right)
AGENT_LAYOUT_DIRECTION=right

# split ratio, passed straight to `herdr pane split --ratio` (default: 0.5)
AGENT_LAYOUT_RATIO=0.5

# what the tab is renamed to (default: agent)
AGENT_LAYOUT_TAB_NAME=agent

# run this script instead of the bundled recipe (default: unset)
AGENT_LAYOUT_RECIPE=
```

Every key is optional, and every default is the behaviour the plugin had before
the key existed. Real environment variables win over the file, a comment needs a
line of its own, and a line the plugin cannot parse is skipped rather than
failing the hook.

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
