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

## `--version`, and why three facts rather than one

A running binary was found **two commits behind its source**: built at 08:36 while the
last commit was 09:59, with three source files newer than the artifact. The manifest
Herdr held was current, including `startup` and both panes, so the busy-pane retry and
the `worktree.opened` layout were registered and not actually running. Diagnosing it took
three commands and an inference.

A crate version alone would not have caught it. Under this repository's convention the
version moves **only on a release commit**, so the stale binary and the current manifest
both read 0.3.0. Three facts are reported because each catches something the others
cannot:

| Fact | Catches |
| --- | --- |
| Crate version, compiled in | Which release the binary belongs to |
| Git commit, compiled in | A binary behind its source **within** a release |
| Build time, compiled in | Whether the artifact predates the last edit |
| Manifest version, read at run time | Staleness **across** a release, with no git involved |

The manifest line was not in the original design and is a real addition rather than a
duplicate. `CARGO_PKG_VERSION` is baked in at **compile** time; the manifest is read from
disk at **run** time, and it is the file Herdr itself reads to decide what this plugin is.
So one says what you are running and the other says what Herdr thinks you have. After the
next release commit, a stale binary reports the old number against a manifest holding the
new one — caught **without git**, which matters because the commit is exactly what goes
missing when git does.

When the two disagree the output says `STALE` and names the fix. Printing two numbers and
leaving the reader to compare them invites a bug report rather than a diagnosis.

### Nothing in it can fail, and each failure names itself

Every lookup degrades to a sentence rather than an error. This is the command reached for
when everything else is broken, so one that errored because it could not find its own
manifest would be worse than one that says less.

The manifest paths were once a single `Option`, so four different situations printed one
message: "manifest unknown". That tells a troubleshooter nothing about which they are in,
and troubleshooting is the use this line was endorsed for. They are distinguished now:

| Situation | What it prints |
| --- | --- |
| The plugin root cannot be resolved | `manifest not found: set HERDR_PLUGIN_ROOT to the plugin checkout to read it` |
| The file cannot be read | `manifest unreadable at <path>` |
| The file is not TOML | `manifest unparsed at <path>` |
| The file has no `version` key | `manifest has no version key at <path>` |

The first names its own fix, which is the property that makes the `STALE` line worth
having. The first three match `mikebronner.recent-spaces`, which was asked for the same
format; the fourth is ours, because that plugin's three do not cover a manifest that
reads perfectly and simply has no version in it.

The build script holds the same rule. Git may be absent, and the plugin root may not be a
repository at all: `herdr plugin install` clones, but a source tarball would not. Herdr
reports build failures and installs no toolchains, so failing a build for a diagnostic
string would be a bad trade. **Verified by building a copy of this crate outside any
repository**: it compiled and reported `unknown`.

### Two decisions worth recording

**A dirty tree is marked.** Not asked for, and included because it is the case a bare
hash silently misrepresents: a binary built from uncommitted changes reports a commit
whose source is not what was compiled. That is the same class of failure as the stale
binary this exists to catch, so hiding it would undercut the point. A `status` that
itself fails reads `-unverified` rather than being claimed as clean.

**The watch list and the dirty pathspec are one list.** `BUILD_INPUTS` holds `src`,
`build.rs`, `Cargo.toml` and `Cargo.lock`, and both halves read it: the paths cargo
watches for a rebuild, and the pathspec `git status` is asked about.

They used to differ, watching `src` while asking about the whole tree, which matched
neither meaning: a modified README could mark the binary dirty with nothing rebuilding to
notice, and an edit to `Cargo.toml` could leave it saying clean.

**The meaning chosen is "what was compiled", not "what `git status` says".** A hash claims
this binary came from that commit, and only these files can make that claim false. A
modified README casts no doubt on the artifact, so reporting it would be noise in a field
whose whole job is signal.

The git head is watched as well. With no `cargo:rerun-if-changed` at all, the script
reruns on a package change and would miss a commit made with no file edits, embedding a
stale commit — a poor joke given what this is for. With only the git paths, a source edit
would not rerun it and the build time would predate the binary beside it.

**`rerun-if-changed` is keyed on mtimes**, so a bare `touch Cargo.toml` reruns the script
and refreshes the build time even though nothing resolved changed. Measured 2026-09-10
against a control: two consecutive builds held the embedded time still, and a `touch`
moved it.

This corrects an earlier note here, which said the opposite on the strength of cargo's
**package** fingerprint being computed from the parsed manifest rather than its bytes.
That mechanism is real, and it is a different one. It stopped being the mechanism in
force the moment `Cargo.toml` joined `BUILD_INPUTS`, and the note outlived the change
that retired it. The failure that creates is worse than a wrong comment: somebody touches
`Cargo.toml`, watches the marker move, and finds a document telling them it cannot.

**`.git/refs` is watched as a directory, not as the one file `HEAD` points at.** The
specific-file version has a hole: `git gc` packs the refs and deletes
`.git/refs/heads/<branch>`. Two things then go wrong, and the second is the expensive
one. A directive naming a **missing** path reruns the script on every single build —
measured 2026-09-10, correcting a second claim here that cargo ignores such a directive.
And dropping the watch to avoid that is not available either, because a later commit
writes the loose ref back and nothing would be watching for it, leaving a hash the binary
was not built from.

A directory watch closes both. `.git/refs` survives `gc`, its subdirectories staying
behind empty, so no directive ever names a missing path; six consecutive builds after a
`git pack-refs --all` held the build time still, and a loose ref written afterwards moved
it. The cost is one extra rerun per `git fetch`, which writes remote refs underneath.
This script runs two short git commands, so that is the cheap side of the trade against a
stale hash.

`.git/packed-refs` is deliberately not watched. A ref that **moves** is always written
loose; only `gc` and `pack-refs` write that file, and neither changes what `HEAD`
resolves to.

**Both paths are resolved with `git rev-parse --git-path`, not built by joining onto a
literal `.git`, and that is for the linked-worktree case specifically.** The next reader
sees a subprocess call where a path join would do, so the reason has to be written down.

**In a linked worktree `.git` is a file**, and the real git directory is elsewhere.
Measured 2026-09-10 in a worktree of this repository: `.git` is a 90-byte file, so a bare
`Path::new(".git").exists()` says yes and both joined paths then fail to exist. The
existence filter above would leave **nothing at all watched**, and a commit would rerun
nothing — a silently stale hash, in the checkout layout this plugin exists to serve.
Worse than the every-build rerun the filter was added to prevent, because that one is
visible and this one is not.

`--git-path` returns `.git/HEAD` and `.git/refs` unchanged in a normal checkout. In a
worktree it returns the per-worktree `HEAD` and the **shared** `refs` directory, which is
where a worktree's own branch ref lives — so one directory watch covers a commit made in
either place. Measured end to end: three builds in a worktree held the stamp still, and a
commit made inside it moved both the stamp and the embedded hash. Outside a repository
`git rev-parse` exits 128, so a source tarball watches nothing rather than naming files
that are not there.

Cargo recurses into a watched directory — a content edit and a file addition two levels
down each moved the stamp, against a control that held still. Measured on cargo 1.97.0,
which is the version in use here.

`.git/index` is deliberately not watched. Staging a file moves it from unstaged to staged
in `git status --porcelain` and leaves the output non-empty either way, so the marker
cannot flip on a bare `git add`. Verified rather than assumed.

## The Herdr version floor

`min_herdr_version` is **0.8.2**, unchanged by the rewrite.

Every method this plugin sends exists at **protocol 20**, which is Herdr 0.8.2:
`workspace.list`, `pane.list`, `tab.list`, `layout.apply`, `pane.rename`,
`pane.send_input`, `agent.start`, `plugin.pane.open`, `notification.show`.
`layout.apply` was checked at protocol 20 specifically, because the engine now
depends on it entirely.

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

The **first** tab is the exception, because it is not added — it takes over the
workspace's existing tab. So:

| Situation | What happens |
| --- | --- |
| A tab of its name already exists | Skipped. |
| The active tab holds one pane and no agent | **Taken over**, by replacing it. |
| The active tab is busy | **Added** as a new tab. |

Taking over means replacing: `layout.apply` with a `tab_id` destroys the tab and
builds a new one carrying the same label. The one bare pane that was there does not
survive, which is exactly why the tab has to be **free** to qualify. A busy active tab
is never wrecked to save a tab creation.

That third row is the one genuinely new behaviour. v0.2.0 refused the run
outright. Creating a tab instead is what stops a workspace the user has already
arranged by hand from being wrecked, while still giving them the layout they
asked for.

The consequence worth naming: **a run that died halfway is resumed by the next
one**, because the tabs it finished are skipped and the ones it did not are
built. That is why a failed `layout.apply` on a later tab is still fatal — the work
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

A rebuild **replaces the whole tab**, in one `layout.apply` carrying its `tab_id`.
There is no pane-by-pane close and no surviving pane: every pane in that tab is
destroyed and the configured tree is built in its place, under the same label.

The scope of that destruction is **the tabs the layout names, and no others**. That
is Mike's decision, in his words — *only destroy the tabs named in the layout* —
taken after being shown the consequence of a wider scope, which is that one keypress
would destroy an unrelated tab. It is a chosen boundary rather than a conservative
default, and `panes_of_another_tab_are_not_destroyed_by_a_rebuild` pins it.

### Destroying a pane running an agent needs consent

An agent mid-turn holds work that cannot be recovered, so a rebuild that would
destroy one **asks first**, in a popup. There is no force flag: the question is the
mechanism. There are **two** answers.

| What the user does | Word on the wire | What happens |
| --- | --- | --- |
| `y` | `close` | The tab is replaced, agents and all, and rebuilt clean. |
| `esc`, `Ctrl-C`, or a click anywhere in the pane | `nothing` | **Not one pane of the tab is touched.** |

There used to be a third. `n` kept the running agent and rebuilt the layout **around**
it, and it is gone because this engine cannot express it: a `tab_id` replaces the tab
wholesale, and `pane_id` on a leaf is output-only, so a live agent cannot be carried
into a new tree. Mike dropped the requirement rather than the rewrite, on the grounds
that new panes are being made anyway.

**`n` and `q` are bound to nothing.** They were briefly kept as cancel aliases, on the
theory that an old `n` habit would otherwise destroy the pane it used to protect. Mike
says `n` was never a habit of his, so they defended against nothing. An unbound key is
ignored and the dialog stays open, which is **not** cancelling: the destructive answer
is one keystroke away, so a key nobody bound must not resolve the question either way.

### A click dismisses, and can never confirm

Measured on 0.9.0, in an isolated server with a real UI client driven through a pty. A
click **inside** a plugin pane is forwarded to that pane as an SGR sequence, rebased to
pane-local coordinates: a click at screen (60, 20) arrived as `<0;14;4M`. A click
**outside** it was not forwarded at all. Right-click is forwarded too.

So the whole pane is the dismiss target. There is nothing to aim at in this dialog, no
buttons and no regions, so "click to get out" can only mean any click anywhere.

**No mouse event can reach the destructive answer.** A stray click destroying an
agent's work is the failure this dialog exists to prevent, so `close` stays on an
explicit `y`. Motion and scrolling are not answers either: mouse capture reports
movement as well as clicks, and acting on it would close the dialog the instant the
pointer crossed the pane, before it had been read.

Raw mode and mouse capture are restored by a `Drop` guard rather than a call after the
loop, so an early return, an error or a panic cannot leave the pane in raw mode with
mouse reporting on, printing escape sequences at whoever uses it next.

Everything that is not the one acting word means `esc`: a dismissed popup, a popup
that could not be opened, a popup that never started, a timeout, an answer nobody
recognises. A popup the user closed or ignored is the same class of event as a
misfire, so it costs the same nothing. **Acting on silence is the thing being
prevented**, and each of those paths says which one happened, because a silent no-op
after a keypress reads as a broken keybinding.

Declining does not abort the run. The tab that was in question is left exactly as it
was, and the layout's **other** tabs are still built — those were never at risk, and
abandoning them would make one `esc` cost the whole run. The report distinguishes the
two ways a tab goes untouched, because "left alone, already there" names the guard and
"left untouched, not confirmed" names the user's own answer.

**A rebuild that touches no agent pane asks nothing at all.** Nothing is at risk, so
there is nothing to consent to, and a popup there would make the common case slow for
no reason. That is why `NeverShown` is only ever reachable when a pane really is in
danger: the two situations are separated before the question is asked, not after.

### Why one keypress and no ratatui

`crossterm 0.29` reads the key and the click; there is no ratatui. A yes/no needs no
widgets, no layout engine and no render loop, and Herdr already draws the pane frame
and puts the manifest's `title` on it. A few `println!`s and one event is the whole
interface. The
version is pinned to what Herdr's own `Cargo.toml` uses, which keeps the property that
every crate here is one Herdr already depends on.

Raw mode needs a tty. The popup always has one, being a real pane; a test harness
piping stdin does not. Rather than fail there, it falls back to reading a line, which
keeps both choices answerable either way — and means the fallback is exercised by the
suite instead of being untested code that only runs once something has gone wrong.

### Why the answer travels through a file, and why the popup reports its own pid

A plugin pane is a **separate process**, so whatever it learns from the user has to
reach the process applying the layout. This side creates a directory, hands two file
paths to the popup through `plugin.pane.open`'s `env` map, and polls. The popup is
this same binary under `--confirm`, so the question is worded in one place and there
is one language.

Three measured facts force the design, and each was checked rather than assumed:

1. `plugin.pane.open` answers `{"type":"ok"}` on success and **carries no pane id**.
   (It can fail: `ui_busy` when a popup is already open, for one. What it never does
   is hand back a handle.)
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

The split is deliberate, and the line falls in the same place it always has: before
the panes exist, dying is safe; after they exist, it is not.

**Building a tab is fatal.** The `layout.apply` that builds it is **atomic** — a
rejected tree leaves no tab behind — so dying there leaves the workspace exactly as
the run found it, and the next run builds the tab from scratch.

**A command or a label is not fatal.** Once the panes exist, the guard turns
every later run into a skip for that tab. Dying there would report a layout that
was in fact built, and no rerun could ever finish it. A failed command or a failed
label is therefore reported and the run still succeeds.

**One long-standing hazard is now gone, and it is worth naming as gone.** Under the
sequential engine a failed `pane.split` left a tab carrying the layout's name and
half its panes: the guard skipped it forever after, and nothing but the user closing
the tab could finish the job. v0.2.0 had the same property by a different route.
Neither could recover. A tab now arrives whole or not at all, so there is no
half-built state for the guard to freeze.

The response is checked as well as the call. If `layout.apply` answers with a pane
count that is not the number of leaves sent, the run is fatal and says both numbers
— applying commands, labels and agents positionally against a tree of a different
shape would put them in the wrong panes.

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

## The flat config nests to the right

The config is a flat list where each pane after the first splits the one before it.
As a tree that nests to the right: pane 1 against everything else, then pane 2
against everything after it, and so on. The split's `direction` and `ratio` belong to
the pane **being created**, and the ratio is the share the pane being split keeps.

That reproduces v0.2.0, where the first split divided the arriving pane and the
second divided the pane the first one made, putting the tool above the shell.
Splitting the first pane twice would stack three panes down the agent's side
instead, and every ratio would then apply to the wrong pane.

**The mapping was confirmed rather than assumed.** `layout.export` is the exact
inverse of `layout.apply`, and a tab the old sequential engine had built exports as
precisely this shape.

A tree could now nest arbitrarily, so the previous-pane-only limitation is a property
of the **config format** rather than of the engine. Nothing has asked for more, and
the syntax an arbitrary split target would need is not obvious.

**An absent `ratio` becomes 0.5 on the wire.** A split node's `ratio` is required and
not nullable, unlike `pane.split`'s, so leaving the choice to Herdr is no longer
expressible. 0.5 is not a guess at Herdr's default, it **is** Herdr's default,
measured by splitting with no ratio and exporting the tab straight back. The config's
promise that an absent ratio is not overridden therefore still holds in effect.

### `focus: false` is a real request, not a dead default

The layout opens on the agent for a plainer reason than the parameter: the agent
starts in the first pane of the first tab, and nothing moves the cursor off it.

**Do not strip `focus: false` as a redundant default.** Measured on 0.9.0: a tab
added with `focus: true` really does move the user to it, and `focus: false` really
does leave them where they were. A multi-tab layout sending `true` would end with the
user staring at whichever tab it happened to build last.

A **replacement** inherits whatever the tab it replaced had, which is Herdr's choice
rather than this plugin's. Replacing the focused tab leaves the replacement focused,
because there is nothing else it could be. Replacing a tab that was **not** focused
moves nothing — confirmed end to end against a live server, where a hand-made tab held
the focus through a rebuild of two others.

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

Four outcomes, and they are the whole set:

| Error code | Outcome |
| --- | --- |
| `agent_not_ready` | **Success.** The agent did start and is waiting at a prompt. |
| `agent_name_taken` | **Retry** under the next derived name, unless `--agent-name` supplied it. |
| `agent_pane_busy` | **Wait and retry** the same name, up to a bound. |
| anything else | **Fatal**, `timeout` included. |

Fatal is the default arm rather than a list, so a code Herdr adds later is
reported instead of being silently swallowed.

`agent_not_ready` is not a failure because Herdr documents it as the agent being
blocked during startup while its name stays usable — the agent **did** start. On
a fresh worktree that is Claude Code's trust-this-folder prompt, which is the
normal case, and reporting it as a failed layout was the original bug.

### Waiting out a busy pane

`agent_pane_busy` used to be fatal, on the reasoning that Herdr exposes no readiness
probe so any wait would be invented. A real worktree creation then produced exactly the
failure that reasoning permitted: panes built correctly, no agent, and
`agent target pane wF:p1 is not an available shell (agent_pane_busy)` in the log.
Herdr wants the target pane at an idle interactive prompt with nothing in the
foreground, and the hook fires within about 2ms of the workspace existing.

So it is now waited out, and **the numbers come from a measurement rather than being
round**:

- **250ms between attempts** is one measured shell startup. This user's interactive
  zsh takes 230-260ms to reach a prompt, over seven samples with their real rc, and
  `mise activate` dominates it. One attempt per shell startup catches the common case
  on the second try.
- **20 attempts, a 5 second budget**, is about twenty shell startups. That absorbs a
  cold start where the rc is far slower than steady state, which is the case that
  produced the failure.

**The two retries are different in kind, and are separate loops for that reason.**

| | `agent_name_taken` | `agent_pane_busy` |
| --- | --- | --- |
| Nature | Permanent until something changes | Usually transient |
| What the retry changes | The **name** | **Nothing**; it waits |
| Delay between attempts | None; waiting would not help | 250ms; a different name would not help |

That distinction is load-bearing. Advancing the name while waiting for a shell would
come back under `proj-one-3` because the shell was slow, breaking the reservation that
`herdr agent prompt <name>` depends on.

**Bounded, for a reason the name retry does not share.** A pane held by a real editor
or a running command never becomes free, and no amount of waiting fixes it. An
unbounded wait would turn a clear failure into a hung hook. So the exhausted message
says something different from the others: that the pane is still busy after the whole
budget, so something is running in it rather than the shell being slow — and it carries
Herdr's own wording, because a silent give-up is the invisibility this plugin has spent
its recent history removing.

## `bin/on-event`: one gate, then a hand-off

**Both worktree events act.** `worktree.created` and `worktree.opened` each run the
layout, because opening a workspace should lay it out. Anything else exits at once.

A second **acting** subscription was previously argued to be dangerous, and the
argument was sound: two events firing about a millisecond apart, with Herdr not
serializing hooks, would have two concurrent runs both see one free tab and both build
it. The guard makes a later run safe, not a simultaneous one.

It is safe because **the two events are disjoint per action**, measured on 0.9.0 with a
probe plugin subscribing to both: `worktree create` emits only `worktree.created`, with
and without `--focus`, and `worktree open` emits only `worktree.opened`. One action
never produces both. The full table is in
[`open-questions.md`](open-questions.md).

A non-zero exit is **not** used to refuse an event, because Herdr would record it as a
failed plugin command and "this event was not mine" is not a failure.

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
