# Agentic Panes Layout — a Herdr plugin

Lays out a [Herdr](https://herdr.dev) workspace from a **declarative TOML file**:
tabs, panes, splits, commands, agents, and optional pane labels. Applied
automatically the moment a git worktree workspace is created, or on a keypress
for any workspace you are looking at.

With no config file, the built-in layout is this:

```
+-----------------+----------+
|                 | lazygit  |   agent    half the width
|      agent      |          |   lazygit  60% of the remaining column
|                 +----------+   shell    the other 40%
|                 | shell    |
+-----------------+----------+
```

Everything above is configurable, including the number of tabs and the number of
panes. **Pane labels are opt-in**: nothing is renamed unless you ask for it.

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

Requires Herdr 0.9.0 or newer, matching the floor
[`mikebronner.project-finder`](https://github.com/mike-bronner/herdr-plugin-project-finder)
declares. Every socket method this plugin uses in fact exists at protocol 20, which
is 0.8.2, so the floor is about keeping the two plugins consistent rather than about
a feature.

### Requirements

**A Rust toolchain, 1.74 or newer**, with `cargo` on the machine. The plugin is a
Rust binary and is compiled from source; Herdr installs no toolchains, so this one
is yours to have. Nothing else is needed at runtime.

```sh
# macOS, via Homebrew
brew install rustup && rustup-init
```

`git` is used to identify a workspace's repository, at `/usr/bin/git`, which macOS
and every Linux this targets already have.

### How the binary gets built

Two mechanisms, because each covers what the other cannot.

**Installing from GitHub** runs the manifest's `[[build]]` step: `cargo build
--release`, once, during `herdr plugin install`, after the confirmation prompt and
before Herdr registers the plugin. The compile happens where you are already
looking, and Herdr reports a failure.

**Every Herdr server start** runs the manifest's `[[startup]]` step, which is the same
build. So a linked checkout is refreshed whenever you restart Herdr, without waiting
for something to invoke the plugin. Measured as firing for a linked plugin, not
inferred.

**Everything else** is covered by `bin/agent-layout`, a small `sh` shim that
builds the binary when it is missing or when the sources are newer, then execs it.
That is what catches a source edit while a server is already running, which no
manifest hook fires for. `[[build]]` runs on neither `herdr plugin link` nor update
— verified rather than assumed, and the evidence is in
[`docs/design.md`](docs/design.md).

All three point at the same `bin/build`, so there is one definition of how the binary
gets built. Two builds racing is safe: cargo serialises them, measured.

If you linked a working copy, **build once yourself** so your first worktree
creation is not waiting on a compile:

```sh
cd /path/to/herdr-plugin-agentic-panes-layout
cargo build --release
```

The shim does **not** assume `cargo` is on `PATH`, because Herdr's server runs
under launchd with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. It searches `CARGO`,
`$CARGO_HOME/bin`, `~/.cargo/bin`, and the Homebrew rustup prefixes, and puts
cargo's own directory on `PATH` for the build so `rustc` resolves. If it can find
neither cargo nor a built binary, it says so rather than going quiet.

## What it does

`bin/on-event` is a `worktree.created` hook. It checks that the event is the one
that means a worktree was made — `worktree.opened` is subscribed as well, but only
so Herdr logs it while a design question is settled, in
[`docs/open-questions.md`](docs/open-questions.md) — and hands off to the layout
binary with `--from-event`.

The binary then refuses to act unless the new workspace is the **focused** one.
`herdr worktree create --no-focus` emits `worktree.created` with
`"focused": false`, and laying out an unfocused workspace would start an agent in
whatever you were looking at instead.

It then reads [`agent-layout.toml`](docs/configuration.md), picks a layout for the
project, and applies it: rename or create each tab, split each pane out of the one
before it, run each pane's command, label the panes that asked to be labelled, and
start each pane's agent.

### It speaks Herdr's socket API, not the CLI

Requests go straight down `HERDR_SOCKET_PATH` as newline-delimited JSON, one
connection per request. No `herdr` subprocesses. Errors come back as a structured
code rather than as text to be pattern-matched out of stderr.

### Automatic runs are safe to repeat; explicit ones rebuild

When the `worktree.created` hook fires, the re-run guard is **tab-name based**: a
tab whose name already exists in the workspace is skipped rather than rebuilt. So a
second event is a no-op, and a run that died halfway is finished by the next one.

**Asking for the layout yourself rebuilds instead**, whether through the action, the
keybinding or the script. Asking explicitly is a statement of intent, so it
re-applies the configured layout to a tab that is already there rather than skipping
it. The hook is the one that opts into the guard, so nothing you invoke by hand needs
a flag to get the rebuild.

A rebuild closes the tab's other panes. If one of them is **running an agent**, you
are asked first, in a popup, and it takes **one keypress**:

| Key | What happens |
| --- | --- |
| `y` | Closes the agent's pane and rebuilds the tab clean. |
| `n` | Keeps the agent running and rebuilds the layout *around* it, relabelled as the layout's first pane. No second agent is started in it, and no `command` is typed into it. |
| `esc` | **Changes nothing at all.** |

`esc` exists so a misfire is free. Without it the cheapest answer available still
rearranges every other pane in the tab, so an accidental keypress would cost you a
rearranged workspace.

**A popup you close, ignore, or that cannot be opened means `esc`**, not `n`. Those
are the same class of event as a misfire, so they cost nothing either, and the run
says which one happened rather than going quiet. An agent mid-turn holds work that
cannot be recovered, so silence never counts as consent.

A rebuild that touches no agent pane asks nothing at all and simply proceeds.

The first tab is the exception, because it is not created — it takes over the
workspace's existing tab by being renamed. So it is skipped when a tab of its name
exists, it takes over the active tab when that tab holds a single pane and no
agent, and when the active tab is busy it is created as a new tab instead of being
wrecked.

### What is fatal and what is not

Opening a tab is fatal on failure, because it happens before any pane of that tab
exists and leaves things clean for a retry. Splits are fatal, because the next
step needs the pane id the split returns. Once the panes exist, a failed command
or a failed label is reported and the run still succeeds: the guard means no later
run could finish the job, so reporting a built layout as a failure would help
nobody.

**A bad config file is never fatal.** It falls back to the built-in layout and
says so. See the failure table in [`docs/configuration.md`](docs/configuration.md).

## Applying the layout yourself

Useful for a plain repo, or a worktree you opened by hand, or after you have
rearranged a tab and want the layout back.

The plugin declares an **action**, `mikebronner.agentic-panes-layout.apply`, so there
are three ways to reach it and none of them needs a path:

```sh
herdr plugin action invoke mikebronner.agentic-panes-layout.apply
```

It also appears in Herdr's action menu. And this is the **recommended keybinding**,
in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+ctrl+l"
type = "plugin_action"
command = "mikebronner.agentic-panes-layout.apply"
```

Three reasons to prefer this over the shell form below, and the third is the real
one. There is no absolute path to get wrong. It keeps working when you move the
checkout. And a `type = "shell"` process is handed `HERDR_ACTIVE_*`,
`HERDR_BIN_PATH`, `HERDR_SOCKET_PATH` and `HERDR_SESSION` but **no
`HERDR_PLUGIN_ROOT`** — which is the entire reason the shell form needs an absolute
path. An action is given plugin context, so that requirement disappears.

Note that a plugin manifest **cannot** declare a keybinding for you: there is no
`keys` field in a plugin manifest at any version, and a `[[keys.command]]` block
placed in one is silently ignored. The binding above is yours to add once.

### The shell form still works

For invoking the script directly, or if you would rather not use an action:

```toml
[[keys.command]]
key = "prefix+ctrl+l"
type = "shell"
command = "/absolute/path/to/herdr-plugin-agentic-panes-layout/bin/agent-layout"
```

**The absolute path is required here**, for the `HERDR_PLUGIN_ROOT` reason above.
If you are switching from this form to the action, edit that one entry in
`~/.config/herdr/config.toml`; nothing else changes, and both forms do the same
thing.

## Configure

Settings live in **one TOML file in Herdr's config root**, beside Herdr's own
`config.toml`:

```
~/.config/herdr/agent-layout.toml
```

The file is optional. Here is the built-in layout written out, ready to edit:

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

Add a `label` to any pane to have it renamed. Omit the key and no rename happens.

Point a project at a different layout with a `[[projects]]` rule:

```toml
default = "three-pane"

[[projects]]
path = "/Users/you/Developer/some-repo"
layout = "solo"

[[layouts.solo.tabs]]
name = "agent"

[[layouts.solo.tabs.panes]]
agent = "claude"
```

`path` is matched against both the workspace's checkout path and its repo root, so
naming a repository covers every worktree of it. Rules are evaluated in file
order and the first match wins.

### A broken config is not silent

Any problem in `agent-layout.toml` opens a **popup pane listing every issue**, on the
automatic worktree path as well as when you invoke the layout yourself. Press any key
to dismiss it. It opens *after* the panes are built and nothing waits for it, so a
warning never delays your workspace.

This is a pane rather than a toast for a measured reason: with
`ui.toast.delivery = "system"`, `notification.show` answers `shown: false` and the
message is dropped before rendering, while a plugin pane opens regardless. Herdr also
gives plugin toasts no severity, allows only one at a time, and rate-limits them.
[`docs/configuration.md`](docs/configuration.md) has the measurements.

Every diagnostic also goes to stderr, which is what
`herdr plugin log list --plugin mikebronner.agentic-panes-layout` keeps.

### Check a config before you rely on it

```sh
bin/agent-layout --check
```

It prints the problems it found, the layout that would be chosen and why, and the tabs
and panes it would build. It exits non-zero when the file would not be used as written.

**It touches nothing** — no socket, no workspace, no pane. That matters because a bad
config is otherwise invisible: a file that fails to parse falls back to the built-in
layout, which still produces a plausible workspace. The only clues are a diagnostic
nobody was watching for and a layout that is quietly not the one you wrote.

Worth knowing: **one broken layout discards the whole file**, including layouts that
are correct. `--check` says so when it happens.

**The full reference is [`docs/configuration.md`](docs/configuration.md):** the
whole schema, how the config root is derived, what each failure does, and why the
file sits where it does. The config root is never hardcoded, because
`XDG_CONFIG_HOME` relocates it and a debug build of Herdr renames it.

### Upgrading from 0.2.x

The `.env` in the plugin config directory is **gone, with no fallback**, and so
are all eleven `AGENT_LAYOUT_*` variables and the `AGENT_LAYOUT_RECIPE` escape
hatch. Move your settings into `agent-layout.toml`; the table in
[`docs/configuration.md`](docs/configuration.md) says what replaced what.

If you relied on the three pane labels, add `label` keys to the block above. They
are no longer applied by default, which was the point of the release.

## Arguments

With **no arguments**, the binary lays out the focused workspace with an agent
name derived from that workspace's label. The keybinding presses it that way, so
that behaviour is fixed.

```sh
bin/agent-layout [--from-event] [--workspace <id>] [--agent-name <name>]
```

`--from-event` marks the automatic path. It reads `HERDR_PLUGIN_EVENT_JSON` and
does nothing unless that workspace is the focused one, and it opts into the
tab-name guard so an existing tab is skipped. The event hook passes it and nothing
else does.

`--confirm` is how the plugin runs *itself* inside the confirmation popup. It is
declared in the manifest's `[[panes]]` block and is not for you to type.

`--workspace <id>` lays out the workspace with that id rather than the focused
one. "Which workspace is focused?" is the right question for the hook and the
keybinding, and the wrong one for a caller that opens several workspaces unfocused
and picks a focus target at the end.

`--agent-name <name>` starts the agent under that exact name instead of a derived
one. The name reaches Herdr **verbatim**: not lowercased, not prefixed, not
truncated. A caller that reserves distinct names across a batch of workspaces does
so precisely so `herdr agent prompt <name>` reaches each one. With several
agent-bearing panes it binds to the first, and the rest derive.

There is no flag to skip the agent, and none to point at an external script.

Every argument error is fatal and is reported **before anything is read or
changed**: an unknown flag, a flag missing its value, a flag given an empty value,
and the same flag given twice. A `--workspace` id that matches no workspace is
fatal too, rather than falling back to the focused one. That fallback would lay
out whatever you happened to be looking at, which is the accident the flag exists
to prevent.

## Known limitation

`agent.start` sometimes fails with `agent_pane_busy` ("is not an available
shell"). The hook fires within about two milliseconds of the workspace being
created, and the new pane's shell has not always reached its interactive prompt by
then.

Herdr has no probe for that specific state. There is no `pane wait-shell`, and a
pane reports no `available` field. The closest thing, `pane.wait_for_output`, waits
for text you name, which means guessing your shell prompt. So the plugin neither
retries nor sleeps: the right wait is unmeasured, and a made-up one would only
trade a visible failure for an invisible delay. The failure is reported as a toast,
and pressing the keybinding afterwards finishes the layout.

`agent_not_ready` is a different outcome and is treated as success. Herdr documents
it as "the agent is blocked during startup", which means the agent did start and is
waiting at a question. On a fresh worktree that question is Claude Code's
trust-this-folder prompt.

## Tests

```sh
cargo test
```

The binary is run for real as a subprocess against a **stub socket server** in a
temporary directory. Nothing is mocked in-process and no pane is ever split. Unit
tests sit beside the code for the parts a socket cannot reach: the config schema,
project matching, and agent-name derivation.

## Documentation

The shim, the hook and the manifest carry no comments. Everything that would have
been one lives in `docs/`:

- [`docs/configuration.md`](docs/configuration.md) — `agent-layout.toml` in full:
  the schema, the config root, per-project rules, and what every kind of bad file
  does.
- [`docs/design.md`](docs/design.md) — why the code is shaped as it is. The
  entry paths, the arguments, the guard, the fatal versus non-fatal split, the
  socket transport, and the trust boundary.
- [`docs/herdr-behaviour.md`](docs/herdr-behaviour.md) — Herdr behaviour this
  plugin depends on that Herdr does not document, such as which side `ratio`
  sizes and the fact that `pane.run` is not an API method. Every entry is dated
  and attributed. **Read the measurement method there before probing anything**:
  measure in an isolated server, never the live one, or a probe split will
  rearrange the workspaces you are working in.
- [`docs/open-questions.md`](docs/open-questions.md) — decisions deliberately not
  taken yet, each with the condition that resolves it. The `worktree.opened`
  subscription is the open one.

Measurements there are dated and attributed to a Herdr version. Treat each as
"true of that version" rather than "true today".
