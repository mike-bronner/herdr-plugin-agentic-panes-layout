# Measured Herdr behaviour

Herdr does not document most of what this plugin depends on. Everything below
was measured rather than read, and this file is the record. The scripts carry no
comments, so this is the only place these facts exist.

**Every measurement here was taken against Herdr 0.8.2**, on the dates given.
Herdr on this machine is now 0.9.0. Nothing below has been re-taken against
0.9.0, so treat each as "true of 0.8.2" rather than "true today". Do not restate
one as current without re-measuring, and do not delete one because it is old: a
stale measurement with a date is still evidence, and an undated guess is not.

## How to measure safely

Measure in an **isolated server**, never in the live one. A probe split or
rename in the live server rearranges the workspaces you are working in.

The isolation lever **for a server you start** is `XDG_CONFIG_HOME`, paired with
`--session`:

```sh
rm -rf /private/tmp/hgeo && mkdir -p /private/tmp/hgeo
XDG_CONFIG_HOME=/private/tmp/hgeo herdr --session geo server &
```

`XDG_CONFIG_HOME` moves Herdr's **entire** config root, so the socket,
`plugins.json` and `session.json` all move together.

**Pair it with `--session`.** An ambient `HERDR_SOCKET_PATH` in your shell may
already point at the live socket, and `--session` is what stops the probe server
answering there.

**A shell inside a Herdr pane is the dangerous case**, and it is the usual one
when working on this plugin. Confirmed 2026-09-09: such a shell carries
`HERDR_SOCKET_PATH`, `HERDR_PANE_ID`, `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID` and
`HERDR_BIN_PATH` pointing at the **live** server. Scrub them, or every
"isolated" command lands in the real session.

**Scrub with an allowlist.** Start from an empty environment and add back only
what the command needs:

```sh
env -i HOME="$HOME" PATH="$PATH" HERDR_SOCKET_PATH=<probe socket> <command>
```

Unsetting variables by name, with explicit `-u`, is the weaker fallback. It
fails on the variable you did not think of, and the five named above are not the
whole set. Seen 2026-09-09 in a peer session on
`herdr-plugin-project-finder`: `HERDR_PLUGIN_CONFIG_DIR` survived a round of
named unsets and made `bin/config-env` read another plugin's `.env`. An
allowlist has no such failure mode, which is why it is the recommendation rather
than one of two equal options.

On **0.9.0** the probe socket is at
`<config root>/herdr/sessions/<session>/herdr.sock`, measured 2026-09-09. The
method is unchanged; only the path is worth knowing when looking for it.

Two near misses look like isolation and are not:

- **`HERDR_SOCKET_PATH` moves only the socket, so it cannot isolate a server
  you start.** A server started that way
  restores the **live** `session.json`, runs your real workspaces in a second
  process, and writes its own state back over theirs. Measured 2026-09-08: a
  throwaway workspace created in such a server appeared in the live
  `~/.config/herdr/session.json`. The live server happened to overwrite it
  minutes later, which is luck and not a safety property.
- **`HERDR_CONFIG_PATH` isolates nothing at all.** It moves neither the socket
  nor the state, whether it names a directory or a file.

**Starting a server and pointing a client at one are different problems**, and
the first bullet is only about the first of them. Do not carry it across. For a
**client** command, one that talks to a server something else already started,
`HERDR_SOCKET_PATH` alone is the correct and sufficient lever. The CLI finds a
server by socket path and by nothing else.

Measured 2026-09-09: this repo's full test suite ran against the plain
`/opt/homebrew/bin/herdr` binary, with no shim and no `--session` anywhere, and
with only `HERDR_SOCKET_PATH` set to the isolated server's socket. Six
workspaces were created in the isolated server. The live server held at 8
workspaces, counted before and after.

**Binary selection and server selection are separate axes.** `HERDR_BIN_PATH`
decides which executable runs. The socket decides which server that executable
reaches. In the run above, `HERDR_BIN_PATH` was set to the same value as the
hardcoded default in `bin/agent-layout`, so it changed nothing, and the command
still reached the isolated server. Testing a client command in isolation
therefore needs no redirected binary, and no shim injecting `--session`.

Keep the config root **short**, under `/private/tmp` rather than a deep scratch
path. Startup otherwise dies with `local socket name length exceeds capacity of
sun_path of sockaddr_un`.

**Verify isolation three ways before probing anything**, because the failure is
silent:

1. `workspace list` must return zero workspaces. A shared `session.json`
   restores the live ones, which is the tell.
2. `plugin list` must show only what you linked.
3. The live `plugins.json` must be byte-identical afterwards, by sha256 and
   mtime.

Herdr's own documentation surfaces, for reference: `herdr --skill`,
`herdr api schema --json`, `herdr <command> --help`, and `herdr config check`,
which accepts a config key and names any key it does not know.

## Process environment

### The launchd PATH

Herdr's server runs under launchd with exactly:

```
PATH=/usr/bin:/bin:/usr/sbin:/sbin
```

Verified against the live process, 2026-09-05. `/opt/homebrew` is **not** on it,
so a bare `herdr` does not resolve from anything the server spawns. This is the
trap that killed four third-party plugins. It is why this plugin spells
`/bin/sh` and `/usr/bin/python3` by absolute path, and why it resolves the herdr
binary from `HERDR_BIN_PATH`.

### What a `[[keys.command]]` process is handed

A `type = "shell"` keybinding command is spawned **detached** and has no pane of
its own. Measured 2026-09-05, by pressing the key for real inside an isolated
`herdr --session probe`, it is handed exactly:

```
HERDR_ACTIVE_WORKSPACE_ID  HERDR_ACTIVE_TAB_ID
HERDR_ACTIVE_PANE_ID       HERDR_ACTIVE_PANE_CWD
HERDR_BIN_PATH             HERDR_SOCKET_PATH  HERDR_SESSION
```

`HERDR_PANE_ID` is **not** among them. That variable belongs to the environment
Herdr injects into a managed pane, which is a different set entirely.
`HERDR_PLUGIN_CONFIG_DIR` is not among them either, which is why
`bin/config-env` has to ask `herdr plugin config-dir` when it is reached from a
keybinding.

### What a plugin event hook is handed

A plugin event hook gets the pane set instead: `HERDR_PANE_ID`,
`HERDR_WORKSPACE_ID`, `HERDR_TAB_ID`, plus `HERDR_PLUGIN_ROOT`,
`HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_EVENT` and `HERDR_PLUGIN_EVENT_JSON`.

Running either script by hand from an ordinary shell gets neither set.

## The plugin registry and the manifest

Measured 2026-09-08.

- **The server re-reads `herdr-plugin.toml` from disk at dispatch time.** It
  never consults `plugins.json` for dispatch. A manifest edit takes effect on the
  very next event, with no re-link, no restart and no `reload-config`. That is
  why no re-register step is documented anywhere in this repo.
- **This cuts both ways.** A broken or deleted manifest silently stops all
  dispatch just as immediately, with no toast and nothing surfaced. The plugin
  simply goes quiet, and the first sign is a worktree that never gets laid out.
  This is why the test suite parses the manifest for real.
- **`plugins.json` only records which directories are plugins and whether each
  is enabled**, and it is **keyed by path**. Moving this checkout breaks the
  link, while renaming the id in the manifest does not. The cached version string
  there is cosmetic. `plugin disable` is the one thing the registry truly owns.
- **`herdr plugin config-dir <id>`** answers with the plugin's config directory.
  Asking is correct rather than assembling
  `~/.config/herdr/plugins/config/<id>` by hand: that is an internal layout, and
  `HERDR_CONFIG_PATH` can move it.

## Events

### What the worktree creation paths emit

Measured 2026-09-05, by subscribing `worktree.created`, `workspace.created` and
`workspace.focused` together and reading
`herdr plugin log list --plugin mikebronner.agentic-panes-layout`:

| Path | Events emitted |
| --- | --- |
| CLI `worktree create --focus` | `workspace.created`, `worktree.created`, `workspace.focused`, all within 2ms |
| UI `new_worktree` (prefix+ctrl+n) | `workspace.created`, `worktree.created`, 1ms apart, **no** `workspace.focused` |

`worktree.created` is the only event common to both paths, and the only one that
names what happened.

Two third-party plugins (`razajamil/herdr-plugin-workspace-manager`,
`danilolucasmd/herdr-clone-layout`) subscribe to all three, on the belief that
the in-app `new_worktree` command emits only `workspace.focused`. That is not
true of 0.8.2.

`workspace.focused` fired **58 times in a 60-entry sample** against one real
worktree creation.

### `worktree.opened`

Measured 2026-09-05 in an isolated `herdr --session` server, by subscribing a
throwaway plugin to `worktree.created` and `worktree.opened` and reading
`HERDR_PLUGIN_EVENT_JSON`:

| Action | Result |
| --- | --- |
| open, no workspace for that checkout | `worktree.opened`, `already_open=false`, workspace focused, `pane_count` 1 |
| open again, same worktree | `worktree.opened`, `already_open=true`, same `workspace_id` |

**Neither open emitted `worktree.created`.** The two events do not co-occur.

The payload carries the same `workspace` object as `worktree_created`, plus
`already_open`. Shape taken from `herdr api schema --json` and confirmed against
a live event.

### `--no-focus` on worktree creation

`herdr worktree create --no-focus` was measured to emit `worktree.created` with
`"focused": false`.

### Hooks are not serialized

Herdr does **not** serialize event hooks. A 5s hook was measured not to delay the
next API call. Two subscriptions that fire ~1ms apart therefore run concurrently.

## `pane split`

### Which side `--ratio` sizes

Measured 2026-09-08, because Herdr does not say: `herdr --skill` never mentions
`--ratio`, and `herdr api schema --json` types `PaneSplitParams.ratio` as a bare
float with no description. The old 0.5 default could not reveal it either, since
a half split looks identical either way.

```
split right --ratio 0.7  ->  original pane 66 of 94 cols (0.702), new 0.298
split down  --ratio 0.6  ->  original pane 23 of 39 rows (0.590), new 0.410
```

**`--ratio` is the share kept by the pane named in `--pane`**, and the new pane
gets `1 - ratio`. Both directions agree.

### Ordering

The split puts the **new** pane after the pane named in `--pane`, so the named
pane stays first.

### `--focus` versus `--no-focus` versus neither

Measured 2026-09-08. `pane split` does **not** move the cursor to the pane it
creates:

- with `--focus` it moves
- with `--no-focus` it does not
- with neither flag it does not either

So **`--no-focus` is the default said out loud**, and `--focus` is the flag that
does something. `pane split --help` lists both and documents neither.

Nor does `--no-focus` pin the cursor to the pane being split. It leaves the
cursor wherever it already is: splitting `w3:p1` while the cursor sat on `w3:p5`
left it on `w3:p5`.

### The answer

`pane split` answers with the pane it created. That is the only way to learn the
new pane's id, which is not derivable from the pane that was split. Documented on
0.8.2 as `.result.pane`, **measured** as `.result.pane.pane_id`.

## `pane run`

`pane run <pane> <command>` takes the command as **one argument**, not a
word-split list, which is how Herdr's own docs use it
(`herdr pane run <id> "just test"`). It types the text into the pane's shell and
presses Enter, so the shell does the parsing and a command with flags works
unquoted.

A pane's shell is **interactive** and sources the user's rc files, so it has the
user's own `PATH` rather than the launchd one. Measured 2026-09-08:
`command -v lazygit` run through `pane run` answered `/opt/homebrew/bin/lazygit`.
That is why a bare `lazygit` is a workable default even though the launchd PATH
rule applies everywhere else.

`pane run` was measured to work immediately after a split, with no settling
delay needed. This is unlike `agent start`, which has a real race.

## `pane rename` and `agent start` write independent fields

Measured 2026-09-08, with `ui.show_agent_labels_on_pane_borders = true`. Before
the rename the agent pane reported `agent="claude"` and no label at all. After
it, `agent="claude"` **and** `label="agent"`, both still set 6s later.

`pane rename` writes `label`. `agent start` writes `agent`. **Neither clears the
other**, so labelling the agent's own pane is not wasted.

What that setting then draws on the pane border is a **client** render choice,
and a headless server cannot observe it. This is measured at the data layer only.
It is a real config key rather than a guess: `herdr config check` accepts
`ui.show_agent_labels_on_pane_borders` and names any key it does not know.

## `agent start`

### Names

Herdr agent names match `[a-z][a-z0-9_-]{0,31}`.

Herdr refuses `agent start` under a name a live agent already holds, with error
code `agent_name_taken`.

### `--timeout` has a 3000ms minimum

A shorter value is rejected outright, so a small timeout is not available. The
default is 30 seconds, and `agent start` **blocks** until the agent is ready.

### A non-zero exit is not automatically a failure

Herdr's own documentation (`herdr --skill`, 0.8.2) says of `agent start`:

> If the agent is blocked during startup, the command returns `agent_not_ready`
> immediately but keeps the name available for `agent read` and
> `agent send-keys`.

So **`agent_not_ready` means the agent did start** and is sitting at a question.
On a fresh worktree that question is Claude Code's trust-this-folder prompt,
which is the normal case for this plugin. Treating every non-zero exit as fatal
turned a working layout into a red toast in `herdr plugin log list`.

`agent_pane_busy` ("is not an available shell") is a different outcome and is a
real failure. See the README's "Known limitation".

### The error payload

The code arrives as compact JSON on **stderr**. Measured on 0.8.2:

```json
{"error":{"code":"agent_pane_not_found","message":"..."},"id":"cli:agent:start"}
```

### Two further codes, measured on 0.9.0

Measured 2026-09-09 in an isolated server, and the only entries in this file
taken against **0.9.0** rather than 0.8.2. Both are fatal, and the recipe treats
them so.

- **`timeout`** — `{"code":"timeout","message":"timed out waiting for agent
  startup"}`. Emitted after the 30s default when the agent never appears. Seen by
  starting a server whose panes could not resolve the `claude` binary: `pane read`
  showed `-sh: claude: command not found`, and `agent list` stayed empty. This is
  distinct from `agent_not_ready`, where the agent *did* start.
- **`invalid_agent_name`** — `{"code":"invalid_agent_name","message":"agent name
  must start with a lowercase letter and contain only lowercase letters, digits,
  '-' or '_' (1-32 characters)"}`. This is Herdr enforcing the name grammar, and
  is what makes it safe for `--agent-name` to pass a caller's string through
  unchanged.

### `agent_not_ready` still holds on 0.9.0

Measured 2026-09-09: a real `agent start` of `claude` into a freshly split pane
returned `agent_not_ready`, and the agent was present in `agent list` afterwards.
The 0.8.2 reading is unchanged.

## `agent list` and the `name` field

**This corrects an earlier record in this repo**, which claimed `agent list`
exposed no `name` field at all. That was wrong.

`name` is **optional** on Herdr's `AgentInfo`, and is **absent rather than
null** for an agent Herdr merely *detected*. Verified against 0.8.2 on a scratch
session: after `agent start probe-alpha`, `agent list` carried
`"name": "probe-alpha"`, and a second agent started by hand in another pane
carried no `"name"` key at all.

So:

- an agent **started under a name** carries `name`
- an agent Herdr **detected** carries no `name` key: a `claude` typed into a pane
  by hand, or one brought back by `resume_agents_on_restore`

Such a detected agent holds no name and therefore cannot be collided with.

**Confirmed again on 0.9.0**, 2026-09-09: after three real `agent start` calls in
an isolated server, `agent list` carried `probe-alpha`, `probe-beta` and
`reserved-2` as `name` values.

**A dedupe against live agent names is therefore possible.**
`herdr-plugin-project-finder`'s `live_agent_names()` depends on it being
possible. See [`design.md`](design.md) for why this plugin still does not dedupe.
