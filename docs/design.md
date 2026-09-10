# Design notes

Why the code is shaped as it is. The shim, the hook and the manifest carry no
comments, so this file is the only record of their reasoning. The Rust sources
carry doc comments where a reader needs one on the spot, and defer to this file
for anything longer.

The Herdr facts these decisions rest on live in
[`herdr-behaviour.md`](herdr-behaviour.md) and are not repeated here. The
configuration file has its own reference in
[`configuration.md`](configuration.md). The open decision about the
`worktree.opened` subscription lives in
[`open-questions.md`](open-questions.md).

## What the pieces are

| File | Job |
| --- | --- |
| `bin/on-event` | The plugin body. Answers "is this the event?" and hands off. No layout logic. |
| `bin/agent-layout` | An sh shim. Finds cargo, builds the binary if it is stale, and execs it. |
| `src/main.rs` | Arguments, the event gate, target resolution, and reporting. |
| `src/api.rs` | The socket client, and one function per Herdr method used. |
| `src/config.rs` | `agent-layout.toml`: the config root, the schema, validation, project matching, and the built-in default. |
| `src/layout.rs` | Applying a layout: the tab-name guard, the splits, the labels, the agents. |
| `src/name.rs` | Deriving an agent name from a workspace label, and suffixing a taken one. |

`bin/on-event` contains **no layout logic** on purpose, and the binary contains
no event logic beyond the gate flag. The split is what lets a layout be applied
to a workspace that already exists: a plain repo, a worktree opened rather than
created, anything opened by hand.

## Why Rust, and why a shim in front of it

The plugin was two shell scripts driving the `herdr` CLI. Mike chose Rust for
tighter integration with Herdr and to stay in the same ecosystem. The two plugins
that solve this exact problem, `razajamil/herdr-plugin-workspace-manager` and
`yuk1ty/herdr-spreader`, are both Rust; that is the pattern being joined. A
survey of 127 Herdr repos put Rust at 23%, the plurality but not a majority, with
scripting languages at 58%.

**The binary is built on demand by `bin/agent-layout`, an sh shim**, which stays
at the path the README documents and the `[[keys.command]]` binding points at.
The shim rebuilds when the binary is missing or when anything in `src/`,
`Cargo.toml` or `Cargo.lock` is newer than it. Cargo takes its own lock, so two
hooks firing a millisecond apart do not race: the second waits.

The manifest **also** declares a `[[build]]` step. Both mechanisms are used,
because each covers exactly what the other cannot.

### When `[[build]]` runs, measured rather than assumed

This has been got wrong twice in this file's history, in both directions, so the
lifecycle is written down narrowly and with its evidence.

`[[build]]` fires **once, on `herdr plugin install owner/repo`**, after the
confirmation prompt and before Herdr registers the plugin. Per Herdr's plugin
documentation: "Build commands run during GitHub `plugin install` after
confirmation and before Herdr registers the plugin", and "`plugin link` does not
run build commands; local authors build their working tree themselves."

Measured 2026-09-09 in an isolated server, to check the half that is testable
locally. A throwaway plugin declaring
`[[build]] command = ["/bin/sh", "-c", "date > /private/tmp/hbld/BUILD_RAN"]` was
linked with `herdr plugin link`:

- The marker file was **never created**, so the build command did not run.
- The plugin registered anyway, and `plugin list` showed it enabled.
- `[[build]]` **was** parsed: the `plugin_linked` answer echoed the whole `build`
  array back.

So `[[build]]` is real and understood at 0.9.0, and `link` does not trigger it.

It does **not** run on link, and it does **not** run on update.

### `[[startup]]`, the second mechanism

`[[startup]]` runs once per enabled plugin at every server start, and again on live
handoff when a new server takes over. It is asynchronous and non-blocking, Herdr
records it in the normal plugin command log, and a failure does not stop the server.
It is one-shot initialisation rather than a supervised daemon, so the command does its
work and exits — which `bin/build` does.

**That it fires for a *linked* plugin was measured twice, independently, and was an
inference until then.** The distinction matters. Herdr's documentation says "each
enabled plugin" and draws no line between linked and installed, and all three of these
plugins added a `[[startup]]` entry on that reading. Taking capability from silence is
the mistake this file has already recorded twice, so the reading being correct is luck
until somebody checks.

- Measured here, 2026-09-10 on 0.9.0: a throwaway plugin declaring a startup command
  that appends to a marker was linked into an isolated server, the server restarted,
  and the marker was written **exactly once**, with `succeeded exit 0` in
  `herdr plugin log list`. Linking alone wrote nothing.
- Measured independently by the `herdr-plugin-recent-spaces` session on the same
  version, by a different route and while probing something else: same manifest shape,
  `plugin link` never `install`, `plugin list` reporting source kind `local`, stop,
  start, marker present holding the command's own output.

**What that proves, and what it does not.** It proves firing on a **server restart**
with the plugin already linked. Linking only makes the entry eligible; it is not itself
the trigger. The same session separately measured that a startup command does **not**
run on `plugin link`, on `plugin enable`, or on a disable-then-enable cycle. Both
measurements are 0.9.0 on macOS, single session.

### Three mechanisms, and what each covers

| Path | Covered by |
| --- | --- |
| `herdr plugin install owner/repo` | `[[build]]` |
| Every server start, and live handoff | `[[startup]]` |
| `herdr plugin link <path>`, before the next server start | **the shim only** |
| A source edit while a server is already running | **the shim only** |
| A plugin update | **the shim only** |

The last two are why the shim stays. Editing source in a linked checkout is the normal
development loop here, and no manifest hook fires for it.

All three point at `bin/build`, so there is **one** definition of how this binary is
built. Duplicating the cargo discovery into a second place is the thing to avoid; three
copies of that logic already exist across these plugins.

**Concurrent builds are safe, measured rather than assumed.** Startup is asynchronous,
so a `worktree.created` can fire while the startup build is still running and the shim
may decide to build too. Two `bin/build` runs were started together against a dirty
tree: **both exited 0**, both logged `Blocking waiting for file lock`, and the binary
was usable afterwards. Cargo serialises them, so the cost is a wait rather than a
corrupt artifact. The uncovered case is a build that fails at startup and is not
retried until something invokes the plugin — and the shim is exactly what handles that.

Herdr installs no toolchains and reports a build failure rather than resolving
it, and its documentation tells authors to document the tools they need. Hence the
cargo requirement in the README.

### Why both

`[[build]]` earns its place for somebody installing from GitHub: without it, their
**first worktree creation** stalls for a minute inside an event hook while cargo
compiles, with nothing but a toast to explain the pause. With it, the compile is a
visible step at install time and a failure is reported where the user is already
looking.

The shim earns its place for every other path in that table, and especially for
this repository's own normal state: linked from a working copy that is still being
edited. An edit to `src/layout.rs` takes effect on the next event, which is the
same property Herdr itself has for `herdr-plugin.toml`.

### The shim cannot assume cargo is on `PATH`

Herdr's server runs under launchd with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. On
the development machine cargo resolves to
`/opt/homebrew/opt/rustup/bin/cargo`, which is not on that list, and there is no
`~/.cargo/bin`. So the shim tries `command -v cargo` first and then a list of
absolute candidates, `CARGO`, `$CARGO_HOME/bin`, `~/.cargo/bin`, and the two
Homebrew prefixes.

When cargo is nowhere and a binary already exists, the shim runs it and says the
binary may be stale. When cargo is nowhere and there is no binary, it dies and
names the command to run by hand. **Neither case is silent**, because a plugin
that quietly does nothing is the failure mode this whole file argues against.

## Why the socket, not the CLI

The plugin speaks Herdr's socket API directly. The wire protocol is
newline-delimited JSON with no handshake: one JSON line out, one JSON line back,
one connection per request. `HERDR_SOCKET_PATH` is injected into plugin
processes, so there is nothing to discover.

What that buys, concretely:

- **Errors arrive as a structured `code`.** v0.2.0 captured stderr and matched
  `"code":"agent_not_ready"` as a substring, and needed a test proving the code
  was matched rather than merely mentioned in a message. That failure mode no
  longer exists.
- **No process per call.** A three-pane layout was nine `herdr` invocations.
- **Nothing can be word-split.** A pane label containing a space was a real
  hazard in the shell version, guarded by a second argv log. A JSON string
  cannot lose its boundaries.

### `pane.run` does not exist, at either version

`herdr pane run` is **client-side sugar**. It is not an API method at protocol 20
or 22, and Herdr's own skill documentation describes it as atomically sending
command text and Enter. It is implemented here as
`pane.send_input {pane_id, text, keys: ["enter"]}`, which was measured against a
live isolated server on 2026-09-09: the command echoed and executed.

Sending the text without the Enter would leave the command sitting unexecuted at
the prompt, which looks identical in a screenshot. There is a test pinning both
halves.

## The Herdr version floor

`min_herdr_version` is **0.8.2**, unchanged by the rewrite.

Every method this plugin sends exists at **protocol 20**, which is Herdr 0.8.2:
`workspace.list`, `pane.list`, `tab.list`, `tab.create`, `tab.rename`,
`pane.split`, `pane.rename`, `pane.send_input`, `agent.start`,
`notification.show`. `herdr tab create` was separately confirmed to exist at
v0.8.2 with identical flags.

The floor is nonetheless **0.9.0**, to match the one
`mikebronner.project-finder` declares. That is a consistency decision across two
plugins with one maintainer, taken with the measurement above in front of it, and not
a technical requirement — nothing here needs anything newer than protocol 20. The
distinction is worth keeping straight, because a reader who assumes the floor is
technical will not think to lower it if the consistency reason ever goes away.

The floor is stated in two places, `herdr-plugin.toml` and the README, and the suite
fails when they disagree.

There is **no version negotiation** in the protocol, but `ping` returns
`{version, protocol, capabilities}`, which is the clean way to discover what a
server supports if this plugin ever needs to.

## Entry paths

The shim has three ways in, and they are equals:

1. `bin/on-event` execs it on `worktree.created`, with `--from-event`.
2. A `[[keys.command]]` binding runs it by hand, with **no arguments**.
3. Another program runs it as an executable, optionally with `--workspace` and
   `--agent-name`.

### The no-arguments contract

With no arguments the binary lays out the **focused** workspace with an agent
name derived from that workspace's label. The keybinding presses it that way, so
that behaviour is fixed and the suite pins it by calling with an empty argument
list on a fixture where another workspace exists to be chosen by mistake.

## The arguments

### They are arguments, not settings

Neither `--workspace` nor `--agent-name` has a config-file equivalent. They are
per-call facts about one invocation, and a setting hiding among them would let a
stale value silently retarget a keybinding press.

### Every parse failure is fatal, and happens first

An unknown flag, a flag missing its value, a flag given an empty value, and the
same flag given twice are all fatal, and are refused **before any request is
sent**. The suite asserts no request other than the toast, which is stronger than
asserting the message: a program that died halfway through a layout it had
already half-applied would pass a message-only test.

`--workspace ""` is the fail-open this closes. An empty string reads as "no id
given" to a bare emptiness test, which would silently lay out the focused
workspace: the one workspace a caller passing `--workspace` certainly did not
mean.

A repeat is refused **even with the same value**. The rule is about the caller
not knowing what it is asking, so comparing the values would make it depend on a
coincidence.

### `--from-event`, and why the gate moved

v0.2.0 answered "is the new workspace the focused one?" inside `bin/on-event`,
with a Python one-liner over `HERDR_PLUGIN_EVENT_JSON`. No Python remains in the
repository, so the check moved into the binary behind `--from-event`, which the
hook passes and nothing else does.

The reason for the gate is unchanged: `herdr worktree create --no-focus` emits
`worktree.created` with `"focused": false`, and laying out an unfocused workspace
would start an agent in whatever the user is actually looking at.

**The flag is explicit rather than inferred from the presence of the environment
variable.** Inferring it would mean an ambient `HERDR_PLUGIN_EVENT_JSON` in some
shell silently changed what a keybinding press did, which is the same class of
mistake as reading the target from the environment.

The gate **fails closed** on every shape that is not a focused workspace:
malformed JSON, an empty string, a missing `workspace` key, a `focused` that is
not a boolean. Each exits 0 and sends nothing, because "this event was not mine"
is not a failure and a non-zero exit would have Herdr record it as one.

## Resolving the target

`workspace.list` once, then either the workspace whose id matches `--workspace`
or the one reporting `focused`. An unknown id is **fatal rather than a fallback**
to the focused workspace, because that fallback would lay out whatever the user
happens to be looking at, which is the exact accident the flag exists to prevent.
The two failures have different messages, so a caller with a stale id is not sent
looking for a focus problem.

The working directory comes from the first pane of the active tab that reports
one. No pane reporting one is fatal: every split and every created tab needs a
`cwd`, and inheriting the plugin process's own directory would put panes
somewhere arbitrary.

## The guard, which is now tab-name based

v0.2.0 guarded on the **pane count** of the active tab. More than one pane, or a
pane already hosting an agent, and the whole run was refused. That made a partly
applied layout permanently unfinishable, and it could not express a layout with
more than one tab at all.

The guard is now **per tab, keyed on the tab's name**:

- A tab whose name already exists in the workspace is **skipped**.
- Otherwise the tab is created.

The **first** tab is the exception, because it is not created — it takes over the
workspace's existing tab by being renamed. So:

| Situation | What happens |
| --- | --- |
| A tab of its name already exists | Skipped. |
| The active tab holds one pane and no agent | **Taken over**, by renaming it. |
| The active tab is busy | **Created** as a new tab. |

That third row is the one genuinely new behaviour. v0.2.0 refused the run
outright. Creating a tab instead is what stops a workspace the user has already
arranged by hand from being wrecked, while still giving them the layout they
asked for.

The consequence worth naming: **a run that died halfway is resumed by the next
one**, because the tabs it finished are skipped and the ones it did not are
built. That is why a failed `tab.create` on a later tab is still fatal — the work
is resumable, so an honest non-zero exit costs nothing.

The guard reads the tab list of the workspace **being laid out**, so it guards a
`--workspace` target as readily as a focused one.

### The guard belongs to the automatic path only

The guard is right when the trigger is `worktree.created` and wrong when a human
pressed a key. Asking for a layout explicitly is a statement of intent, and a
guard that silently ignores it is answering a question the user did not ask.

So an existing tab means two different things:

| Invoked by | Existing tab of the layout's name |
| --- | --- |
| `bin/on-event`, which passes `--from-event` | **skipped** |
| Anything else, including the keybinding | **rebuilt** |

**Only the automatic path opts in, and that direction was chosen deliberately.** The
obvious design is a `--rebuild` flag that the human-facing entry points pass, with
skipping as the default. It was rejected because the README documented a
`[[keys.command]]` entry passing **no arguments**, and a live `config.toml` matched
it: making rebuild opt-in would have left that binding silently skipping until it was
edited. So the **event hook** declares that it is the automatic path, and anything
else means a human asked. A test reads the README's documented shell binding and fails
if it ever grows a flag.

`--rebuild` exists anyway, as an explicit synonym for the default, because the
manifest's `[[actions]]` entry passes it. That way the manifest reads as what it does
rather than relying on the absence of arguments to convey intent.

## The action, and why the manifest cannot carry the keybinding

`[[actions]]` declares `mikebronner.agentic-panes-layout.apply`, whose command is the
shim with `--rebuild`. It is reachable from Herdr's action menu, from
`herdr plugin action invoke`, and from a user binding of
`type = "plugin_action"`.

**A plugin manifest cannot declare a keybinding.** There is no `keys` field in a
plugin manifest at either version, so a `[[keys.command]]` block placed inside one is
silently ignored — `razajamil/herdr-plugin-workspace-manager` ships exactly such a
dead block. The user's own `config.toml` is the only place a binding can live.

What the action buys over the `type = "shell"` form, in increasing order of weight:
there is no absolute path to get wrong, it survives the checkout moving, and — the
real one — a `type = "shell"` process is handed `HERDR_ACTIVE_*`, `HERDR_BIN_PATH`,
`HERDR_SOCKET_PATH` and `HERDR_SESSION` but **no `HERDR_PLUGIN_ROOT`**, which is the
entire reason the shell form needs an absolute path. An action is given plugin
context, so the requirement disappears.

**The action's command is the shim, not `target/release/agent-layout`.**
`NathanFlurry/herdr-plugin-jj-workspace` points its actions and panes straight at the
built binary; doing that here would skip the staleness rebuild that a linked working
copy depends on.

## Rebuilding a tab

A rebuild empties the tab down to one surviving pane and builds the layout from it.
The survivor is the tab's first pane, unless an agent changes that.

### Closing a pane running an agent needs consent

An agent mid-turn holds work that cannot be recovered, so a rebuild that would
close one **asks first**, in a popup. There is no force flag: the question is the
mechanism. It takes **one keypress**, and there are **three** answers.

| Key | Word on the wire | What happens |
| --- | --- | --- |
| `y` | `close` | The agent's pane is closed and the tab is rebuilt clean. |
| `n` | `keep` | The agent's pane becomes the survivor, keeps running, and the tab is rebuilt **around** it. |
| `esc` | `nothing` | **Not one pane of the tab is touched.** |

**`esc` is not a synonym for `n`, and that distinction is the point of having it.**
`n` still closes every non-agent pane in the tab and rebuilds the layout, so without
a third choice the cheapest available answer costs a rearranged workspace. A misfire
has to be free.

Everything that is not one of the three words means `esc`: a dismissed popup, a
popup that could not be opened, a popup that never started, a timeout, an answer
nobody recognises. A popup the user closed or ignored is the same class of event as a
misfire, so it costs the same nothing. **Acting on silence is the thing being
prevented**, and each of those paths says which one happened, because a silent no-op
after a keypress reads as a broken keybinding.

`n` does not abort the run, because that would leave the user with neither the old
layout nor the new one. It works around the survivor: the tab is split out from it,
the other panes are rebuilt, and the survivor is relabelled as the layout's first
pane. Two things are deliberately **not** done to it — no second agent is started in
it, and no `command` is typed into it, because its foreground process is an agent and
text sent there is a prompt rather than a shell command.

**A rebuild that touches no agent pane asks nothing at all.** Nothing is at risk, so
there is nothing to consent to, and a popup there would make the common case slow for
no reason. That is why `NeverShown` is only ever reachable when a pane really is in
danger: the two situations are separated before the question is asked, not after.

### Why one keypress and no ratatui

`crossterm 0.29` reads the key; there is no ratatui. A yes/no/cancel needs no
widgets, no layout engine and no render loop, and Herdr already draws the pane frame
and puts the manifest's `title` on it. Three `println!`s and one key is the whole
interface. The version is pinned to what Herdr's own `Cargo.toml` uses, which keeps
the property that every crate here is one Herdr already depends on.

Raw mode needs a tty. The popup always has one, being a real pane; a test harness
piping stdin does not. Rather than fail there, it falls back to reading a line, which
keeps the three choices answerable either way — and means the fallback is exercised
by the suite instead of being untested code that only runs once something has gone
wrong.

### Why the answer travels through a file, and why the popup reports its own pid

A plugin pane is a **separate process**, so whatever it learns from the user has to
reach the process applying the layout. This side creates a directory, hands two file
paths to the popup through `plugin.pane.open`'s `env` map, and polls. The popup is
this same binary under `--confirm`, so the question is worded in one place and there
is one language.

Three measured facts force the design, and each was checked rather than assumed:

1. `plugin.pane.open` answers `{"type":"ok"}` and **nothing else** — no pane id.
2. **A plugin pane does not appear in `pane.list`.**
3. The pane's command process **does** run, including on a server with no UI client.

So Herdr hands back no handle at all, and there is nothing to poll for. The popup
therefore reports **itself**: it writes its own process id to a `started` file before
drawing anything. The wait then ends on the first of four things — the answer file
being written, the started file failing to appear within 3 seconds, the reported
process dying, or a 120-second deadline.

The pid check earns its place because Herdr may kill the popup outright when the pane
is closed, so the popup cannot be relied on to write an answer on its way out.
Without it, a dismissed popup would hang the rebuild for two minutes. It uses
`/bin/kill -0`, a signal-free existence test, at a slower cadence than the file poll
because each check spawns a process; a libc dependency for one syscall is a
supply-chain surface this plugin does not need.

The 3-second startup bound catches a popup whose process never launched. It does
**not** catch a popup that launched and is never answered because nothing is
displaying it; that runs out the full timeout and then changes nothing, which is slow
but safe.

**A neighbouring plugin avoids this problem rather than solving it, and the shape
does not transfer.** `mikebronner.project-finder`'s picker makes its popup *the
acting process*: `bin/pick-project` runs as the pane, reads the selection
in-process, and makes its own API calls, so nothing is reported back. That works
because the picker's popup is the **entry point** — the user invokes the picker.
Here the popup is an **interruption** partway through an operation that is already
running: the config is read, the target resolved, and earlier tabs possibly already
built before the question arises. Moving the layout into the popup would mean
running it there on the refusal path too, which would put a popup on screen for
every rebuild including the ones that need no question at all.

`NathanFlurry/herdr-plugin-jj-workspace` is the closest precedent in the
ecosystem, and it pairs `[[actions]]` with an `overlay` pane rather than solving a
cross-process handoff. It also points its manifest straight at
`./target/release/<name>`; **this plugin deliberately does not**, because that
skips the shim and with it the staleness rebuild a linked checkout depends on.

It makes a **later** run safe, not a **simultaneous** one. Two concurrent copies
would both read a tab list without their own tab in it and both build. See
[`open-questions.md`](open-questions.md) for why that matters to the subscription
list.

## Fatal versus non-fatal

The split is deliberate and unchanged in substance from v0.2.0.

**Opening a tab is fatal.** A rename or a create happens before any pane of that
tab exists, so dying there leaves the tab clean for the next run.

**Splits are fatal.** A new pane's id exists only in `pane.split`'s answer, so
carrying on with nothing would aim the next split and the command at nothing.

**A command or a label is not fatal.** Once the panes exist, the guard turns
every later run into a skip for that tab. Dying there would report a layout that
was in fact built, and no rerun could ever finish it. A failed command or a failed
label is therefore reported and the run still succeeds.

One hazard is worth stating plainly, because it is **unchanged rather than
fixed**: a split that fails leaves a tab carrying the layout's name, so the guard
skips it forever after and nothing finishes the job by itself. v0.2.0 had exactly
the same property by a different route, where a half-split tab tripped the
pane-count guard. Neither version can recover it without the user closing the
tab.

**Configuration is never fatal at all.** See the table in
[`configuration.md`](configuration.md).

## Messages are toasts as well as stderr

The plugin runs detached, so stderr goes nowhere a human will read. Every message
is therefore **also** a Herdr toast, sent over `notification.show`. A silent
no-op is the one failure mode worth engineering against here.

The toast is best-effort and never masks the real message: its own result is
discarded. Read the log back with:

```sh
herdr plugin log list --plugin mikebronner.agentic-panes-layout
```

## Splits target the previous pane

Each pane after the first splits the pane before it. That reproduces v0.2.0,
where the first split divided the arriving pane and the second divided the pane
the first one made, putting the tool above the shell. Splitting the first pane
twice would stack three panes down the agent's side instead, and every ratio
would then apply to the wrong pane.

There is no way to name an arbitrary pane to split from. Nothing has asked for
one, and the shape it would need is not obvious.

### `focus: false` is insurance, not mechanism

The layout opens on the agent for a plainer reason than the parameter: the agent
starts in the pane the workspace already arrived on, and nothing moves the cursor
off it.

`focus: false` is passed anyway, so **do not strip it as a dead default**. An
accidental `true`, or a later Herdr that changes the default, would land the user
in the last pane created rather than the one holding their agent.

## Labelling is opt-in

A pane is renamed only when its config entry carries a `label` key. Omitting the
key sends no `pane.rename` request at all.

This is the originating request and the reason the release exists. v0.2.0 wrote
three labels unconditionally. `label = ""` is deliberately a **different thing**
from an absent `label`: it is a rename to the empty string, and the suite holds
the two apart, because collapsing them would quietly reintroduce a decision the
user asked to be given back.

## The agent name

Derived from the workspace label so `herdr agent prompt <label>` reaches it,
unless `--agent-name` supplied one.

The derivation lowercases, replaces every character outside `[a-z0-9_-]` with
`-`, strips leading and trailing dashes, falls back to `agent` if nothing is
left, and prefixes `a` when the result does not start with a letter.

**It truncates last, not before the prefix.** A 32-character label starting with
a digit would otherwise come out 33 characters and be rejected. It also truncates
on **character** boundaries rather than bytes, so a multibyte label cannot panic
the way a byte slice would.

### Several agent panes

A layout may declare `agent` on more than one pane. `--agent-name` binds to the
**first** agent-bearing pane and the rest derive and dedupe normally. One
supplied name across several agents is inherently underspecified, and refusing
the combination would invent a limit nobody asked for.

### Deduping a derived name

A derived name can collide: two workspaces whose labels normalise to the same
string derive the same name, and so does a run that meets an agent already live
under it. Herdr refuses the second `agent.start` with `agent_name_taken`.

So a derived name **retries**: `<base>-2`, then `<base>-3`, bounded at 20
attempts before it gives up and dies. Exhausting the bound is a real failure and
is reported as one.

**The base is trimmed, not the suffix.** `-17` has to fit inside the same
32-character limit as the name it is appended to, so the base is cut to
`32 - len(suffix)` before the suffix goes on. Trimming the suffix instead would
produce a name that is no longer distinct, which is the one thing the retry
exists to guarantee.

**It is a loop that re-reads each answer**, not a suffix computed once from the
first failure. Two concurrent processes can both read one failure and both pick
`-2`, so the second must be free to fail again and move on to `-3`.

A name from `--agent-name` **never** retries and dies loudly when it is taken.
The caller reserved that exact string so `herdr agent prompt <name>` reaches the
workspace. Quietly starting the agent under a different name would break the one
guarantee the reservation was built for, so the collision is the caller's to
resolve.

### Why the retry lives here and not in the caller

The server arbitrates `agent.start` and returns `agent_name_taken` to **every**
caller, so retrying on that error deduplicates across concurrent, unrelated
processes.

That is strictly stronger than an in-process reservation set. Such a set covers
one run of one program, cannot see an agent started by hand, and cannot see the
run happening in the next process along. The error can see all three, because the
server is the only thing that knows every live name at once.

## Classifying the `agent.start` result

`agent.start` blocks, and this process is already detached, so the default
timeout costs nothing. Blocking is what makes a startup failure reportable at
all. A `timeout_ms` is not passed, because it has a 3000ms minimum.

Three outcomes, and they are the whole set:

| Error code | Outcome |
| --- | --- |
| `agent_not_ready` | **Success.** The agent did start and is waiting at a prompt. |
| `agent_name_taken` | **Retry** under the next derived name, unless `--agent-name` supplied it. |
| anything else | **Fatal**, `agent_pane_busy` and `timeout` included. |

Fatal is the default arm rather than a list, so a code Herdr adds later is
reported instead of being silently swallowed.

`agent_not_ready` is not a failure because Herdr documents it as the agent being
blocked during startup while its name stays usable — the agent **did** start. On
a fresh worktree that is Claude Code's trust-this-folder prompt, which is the
normal case, and reporting it as a failed layout was the original bug.

`agent_pane_busy` stays fatal deliberately. The pane's shell is not yet
interactive when the hook fires, and Herdr exposes no readiness probe to wait on,
so reclassifying it would hide a layout that really did fail.

## `bin/on-event`: one gate, then a hand-off

Only `worktree.created` is acted on. The compare is what makes the
`worktree.opened` subscription log-only for free: Herdr records every event it
spawns the hook for, and the hook exits before any layout runs.

A non-zero exit is **not** used to refuse an event, because Herdr would record it
as a failed plugin command and "this event was not mine" is not a failure.

The focused-workspace check that used to live here is now `--from-event`, above.

## POSIX sh in the two shell files

The manifest declares `linux` as well as `macos`, and a minimal Linux image ships
`/bin/sh` as dash. Nothing in `bin/on-event` or `bin/agent-layout` is a bashism.

`PLUGIN_ROOT` is `HERDR_PLUGIN_ROOT` when Herdr injects it, and otherwise derived
from the script's own location, which keeps both files runnable straight out of a
checkout by hand.

`HERDR_BIN_PATH` is used only for the shim's own toasts, and only when it is set
and executable. The binary itself never needs the CLI.

## The trust boundary

The config file is user-owned, and two of its values are executed:

- `command` is typed into a pane's own shell, which is what makes a command with
  flags work.
- `agent` names an agent kind, which Herdr resolves to an executable.

Both trust the file, which is the user's own config in their own config
directory, at the same level of trust as Herdr's `config.toml` sitting beside it.
Nothing from the file is passed through a shell **by this plugin**: values travel
as JSON strings to the socket, so there is no quoting or injection surface on the
way out.

A project-local layout file read from inside a repository was considered and
deliberately not built, precisely because it would move that boundary: it would
execute `command` lines from a file a cloned repository could ship. See
[`configuration.md`](configuration.md).

## Testing

The suite runs the real binary as a subprocess against a **stub socket server**,
which is the direct analogue of the stub `herdr` binary the previous Python suite
planted. Nothing is mocked in-process: the whole job of this program is which
requests it does and does not send, and a stub server is what makes that
observable without splitting a pane.

The environment for each run is built from scratch rather than inherited, so a
stray `HERDR_*` variable in the developer's shell cannot decide a test, and `PATH`
is the one Herdr's launchd server really has.

Temporary directories live under `/private/tmp` rather than the platform temp
directory. On macOS that is a long `/var/folders/...` path, and a Unix socket path
over 103 bytes fails to bind with `sun_path` overflow — the same trap recorded in
[`herdr-behaviour.md`](herdr-behaviour.md) for Herdr's own config root.

Unit tests sit beside the code they cover for the parts a socket cannot reach:
the config schema, project matching, and name derivation.

One check the previous suite could only skip now always runs. Parsing
`herdr-plugin.toml` needed `tomllib`, absent from the macOS `/usr/bin/python3`,
so the suite printed a loud banner saying a green run did not mean a valid
manifest. The crate already depends on a TOML parser, so the check and the banner
are both simply gone.
