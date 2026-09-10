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

`bin/on-event` runs on **`worktree.created` and `worktree.opened`**: creating a
worktree lays it out, and so does opening one. Anything else exits at once. It hands
off to the layout binary with `--from-event`.

The binary then refuses to act unless the workspace is the **focused** one.
`herdr worktree create --no-focus` emits `worktree.created` with
`"focused": false`, and laying out an unfocused workspace would start an agent in
whatever you were looking at instead.

Both events act, which needed checking rather than assuming: two acting subscriptions
firing for one action would race to split the same pane. Measured on Herdr 0.9.0 that
they are disjoint — `worktree create` emits only `worktree.created` and `worktree open`
emits only `worktree.opened`. See
[`docs/open-questions.md`](docs/open-questions.md).

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

## Finding the plugin

Both commands below are run from the plugin's checkout, and **Herdr will tell you where
that is**:

```sh
herdr plugin list
# - mikebronner.agentic-panes-layout (Agentic Panes Layout) enabled [local:/path/to/checkout]
```

The path in brackets is the plugin root. Every `bin/agent-layout ...` in this document
means that path plus `/bin/agent-layout`. If you linked the plugin yourself you already
know it; if you installed from GitHub, this is how to find it.

## Configure

Settings live in **one TOML file in Herdr's config root**, beside Herdr's own
`config.toml`:

```
~/.config/herdr/agent-layout.toml
```

That is a different place from the plugin checkout above. The config lives with Herdr's
own settings, and the plugin lives wherever you installed it.

**The file is optional.** With no file at all you get the three-pane layout shown at the
top of this page. Write one when you want something else.

### The shape

A file holds **layouts**. A layout holds **tabs**. A tab holds **panes**. One
top-level `default` says which layout to use.

Here is the built-in layout written out, ready to edit:

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

Every key on a pane is optional:

| Key | What it does |
|---|---|
| `agent` | Starts an agent of that kind in the pane. |
| `command` | One command line, typed into the pane's own shell. Flags work unquoted. |
| `split` | `right` or `down`. Required on every pane after the first. |
| `ratio` | The share kept by the pane **being split**. Omit it and Herdr chooses. |
| `label` | Renames the pane. **Omit it and no rename happens at all.** |

### One pane block is one pane, not one split

The commonest mistake, and the natural misreading. To get **two panes** side by side,
write **two** pane blocks:

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

A single block carrying `split = "right"` is **rejected**, because the first block
describes the tab's own pane and nothing splits that into existence.

### `ratio` sizes the pane being split, not the new one

In the built-in layout above, `ratio = 0.6` sits on the third pane and applies to the
**second**. Each pane after the first splits the one before it, and the ratio is the
share the older pane keeps. `--check` prints this back to you as `pane 2 keeps 0.6`, so
you can see which pane a number actually sizes.

### Using a different layout for a project

Scoping is done with a `[[projects]]` rule, **not** with a key on a tab or a layout:

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

A rule's `path` is matched against the workspace's own directory, its checkout path,
and its repo root. So naming a repository covers every worktree of it, and naming a
plain directory such as your home directory works too. Rules are evaluated in **file
order and the first match wins**, so put narrower rules first. There is no prefix
matching and there are no globs.

### Check it before you rely on it

```sh
bin/agent-layout --check
```

Do this after every edit. It reports every problem, says which layouts are usable, and
prints the tabs and panes it would build. It touches nothing.

A bad file never breaks your workspace. A layout with a mistake is skipped on its own
and the others keep working, and a file that will not parse falls back to the built-in
layout. Either way the problems appear in a popup.

**[`docs/configuration.md`](docs/configuration.md) is the full reference**: every key,
how the config root is derived, what each kind of bad file does, and the measurements
behind those choices.

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

### Check which version is actually running

```sh
bin/agent-layout --version
```

```
agent-layout 0.3.0 (67f7c27, built 2026-09-10T17:09:29Z)
manifest 0.3.0 at /path/to/herdr-plugin.toml
```

Three facts, because no one of them is enough. The **commit** is what catches a binary
built before your last change, which a version number cannot: this repository moves the
version only on a release commit, so a stale binary and a current manifest read the same
number. The **build time** says at a glance whether it predates your last edit. The
**manifest version** is read from disk at run time and is what Herdr itself reads, so
comparing it against the compiled-in crate version catches staleness across a release
even when git is unavailable.

When those two disagree it says so outright:

```
STALE: this binary is 0.3.0 but the manifest is 0.3.1. Rebuild it with `cargo build --release`.
```

A commit reading `unknown` means the build had no git or no repository, which is normal
for a source tarball. A commit marked `-dirty` was built from uncommitted changes to
`src`, `build.rs`, `Cargo.toml` or `Cargo.lock`, so the hash alone would misrepresent what
was compiled. Edits elsewhere do not mark it, because a modified README casts no doubt on
the binary. `-unverified` means `git status` itself failed, which is not the same as
clean.

Nothing here can fail, and each failure says which one it is:

```
manifest unreadable at /path/herdr-plugin.toml
manifest unparsed at /path/herdr-plugin.toml
manifest has no version key at /path/herdr-plugin.toml
manifest not found: set HERDR_PLUGIN_ROOT to the plugin checkout to read it
```

This is the command you reach for when everything else is broken, so every lookup
degrades to a sentence rather than an error.

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

## Waiting for the shell

`agent.start` needs the target pane at an idle prompt, and the hook fires within about
two milliseconds of the workspace existing. So the pane's shell is often still starting,
and Herdr answers `agent_pane_busy` ("is not an available shell").

The plugin **waits it out**: up to 20 attempts, 250ms apart, a 5 second budget. Those
numbers are measured rather than chosen — 250ms is one interactive shell startup on the
machine this was built for, so the common case is caught on the second attempt, and 20
of them absorbs a cold start where your rc is much slower than usual.

The wait is **bounded on purpose**. A pane held by a real editor or a running command
never becomes free, so an unbounded wait would turn a clear failure into a hang. If the
budget runs out, the message says the pane is still busy and that something is running
in it, rather than blaming a slow shell, and it quotes what Herdr said. Press the layout
keybinding once the pane is free.

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
