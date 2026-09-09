# Design notes

Why the code in `bin/` is shaped as it is. The scripts carry no comments, so
this file is the only record of their reasoning.

The Herdr facts these decisions rest on live in
[`herdr-behaviour.md`](herdr-behaviour.md), and are not repeated here. The open
decision about the `worktree.opened` subscription lives in
[`open-questions.md`](open-questions.md).

## The three files

| File | Job |
| --- | --- |
| `bin/on-event` | The plugin body. Answers "is this the event, and is it safe?" and nothing else. |
| `bin/agent-layout` | The recipe. Owns the layout and the "already laid out?" guard. |
| `bin/config-env` | The single reader of the `.env`, for every entry path. |

`bin/on-event` contains **no layout logic** on purpose, and `bin/agent-layout`
contains no event logic. The split is what lets the recipe be applied to a
workspace that already exists: a plain repo, a worktree opened rather than
created, anything opened by hand.

## POSIX sh, not bash

The manifest declares `linux` as well as `macos`, and a minimal Linux image
ships `/bin/sh` as dash. Nothing in either shell script is a bashism, checked by
running the files under a **real dash** and not only by reading them.

`pipefail` went with the shebang and is not missed. Every pipeline in
`bin/agent-layout` ends in a python that dereferences the JSON it was piped, so
a failed `herdr` leaves empty stdin and python exits non-zero on its own.

## Entry paths

`bin/agent-layout` has three ways in, and they are equals:

1. `bin/on-event` execs it on `worktree.created`, with **no arguments**.
2. A `[[keys.command]]` binding (prefix+ctrl+l by convention) runs it by hand,
   also with **no arguments**.
3. Another program runs it as an executable, with arguments.

### The no-arguments contract

**No arguments must keep meaning what it has always meant**: the focused
workspace, an agent name derived from that workspace's label, and an agent
started. The first two ways in pass none, and both are live. `bin/on-event`
execs the recipe bare, and the README's `[[keys.command]]` example invokes it
bare. A changed default would break the pair of them silently.

A custom `AGENT_LAYOUT_RECIPE` is handed the same empty argument list.

### The third way in

The third exists for `herdr-plugin-project-finder`, which builds these same
three panes inline in its own `create_workspace()`. The recipe is duplicated
across the two checkouts, and this is the plugin that owns it, so the picker
calls this file instead.

## The arguments

```sh
bin/agent-layout [--workspace <id>] [--agent-name <name>]
```

### They are arguments, not settings

Neither joins `bin/config-env`'s key set. The ten `AGENT_LAYOUT_` names are
documented in both plugins' READMEs as one shared vocabulary of **user
settings**, so a value learned in either place reads the same in the other. A
per-call argument hiding among them would let a stray `export` in an interactive
shell silently retarget a keybinding press. An argument cannot leak that way,
because it is passed per call or not at all.

### Every parse failure is fatal, and happens first

Parsing runs before anything is read or changed, so a usage error costs the
caller nothing and leaves the workspace exactly as it was found.

- **An unknown flag is fatal.** A caller that misspells a flag must get an error
  rather than a layout it did not ask for.
- **A missing value is fatal.**
- **An empty value is fatal.** `--workspace ""` would otherwise read as "no id
  given" to a bare `-n` test and silently fall back to the focused workspace,
  which is the one workspace such a caller certainly did not mean. This is the
  fail-open that check closes.
- **A repeated flag is fatal.** A caller that passes the same flag twice does not
  know what it is asking for, and silently honouring the second one hides that
  at the one moment it could still be fixed. Because an empty value is already
  refused, a non-empty variable is a sound "already seen" test and no extra
  bookkeeping is needed.

In the shell, `[ "$#" -ge 2 ] && [ -n "$2" ]` short-circuits before `"$2"` is
expanded, so `set -u` is safe on a trailing flag.

### `--workspace <id>`

Lays out the workspace with that id rather than the focused one.

Both choices come out of the same `workspace list`, so `--workspace` changes
which **row** is picked and nothing else. `active_tab_id` and `label` are read
off that row either way, and every step after resolution is identical.

It exists because "which workspace is focused?" is the wrong question for the
picker. That plugin is a reconciler: it opens N workspaces in one pass with
`--no-focus` and only chooses a focus target at the end, so at the moment it
wants each one laid out, none of them is focused.

Asking the API stays the **default** because it is the right answer for the
other two ways in. An event hook and a keybinding both mean "the workspace I am
looking at".

The two failure messages differ because the two failures do. A named workspace
that is absent is a caller passing a stale or wrong id. No focused workspace at
all is a different situation, and sending either one the other's message would
send the reader to the wrong place. Falling back to the focused workspace on a
stale id would lay out whatever the user happens to be looking at, which is the
exact accident this flag exists to prevent.

### `--agent-name <name>`

Starts the agent under that exact name, instead of one derived from the
workspace label.

The name is used **verbatim**: not lowercased, not prefixed, not truncated. The
caller reserved that exact string so `herdr agent prompt <name>` reaches the
workspace, so re-normalising it here would break the one guarantee the
reservation was built for. It reaches Herdr as a single argv element and never
as shell text, so an invalid one is Herdr's to reject and report. That is the
same division of labour this plugin already applies to `--direction`, `--ratio`
and `--kind`.

### Why the target is not read from the environment

The `HERDR_ACTIVE_*` variables a keybinding press is handed would serve, and are
deliberately unused. A plugin event hook gets a different set, and a hand-run
invocation from a shell gets neither. Asking the API which workspace is focused
is the one answer correct in all three **environments**, which is what lets
`bin/on-event` exec this file unchanged. It is also the better semantic: "lay
out the workspace I am looking at".

A caller that means a workspace it is **not** looking at says so with
`--workspace`, which is an argument rather than a fourth guess at the
environment. That is the point of the flag: the environment cannot tell these
cases apart, so the caller has to.

## There is no way to skip the agent

The first pane always holds the preferred agent. A layout that deliberately
leaves it empty is not a shape this plugin offers, and no flag should be added
to produce one.

A caller that must not block on `agent start` detaches the whole
`bin/agent-layout` call and passes `--agent-name`. That gets the panes, the
reserved name and no blocking wait together, so a second route was never
necessary.

## Resolving the target

Each read `eval`s shell assignments that python emits already `shlex`-quoted, so
a label or path containing spaces survives.

The exit status is captured **outside** the command substitution on purpose. A
`die` inside one would exit only its subshell, and `set -u` would then fail on
the unset variable instead of saying anything useful.

## The guard

`bin/agent-layout` refuses to touch a tab that already holds more than one pane,
or whose pane already hosts an agent. That is what makes a second run a **no-op**
rather than a second layout.

The guard reads the pane count of the workspace **being laid out**, so it guards
a `--workspace` target as readily as a focused one.

It makes a **later** run safe, not a **simultaneous** one. It reads pane count
over the API, so two concurrent copies would both see one pane and both would
split. See [`open-questions.md`](open-questions.md) for why that matters to the
subscription list.

## Fatal versus non-fatal

The split is deliberate and is a mirror image.

**The tab rename is fatal.** It happens before any structural change, so dying
there leaves a clean single pane that the next run can lay out.

**Everything from the first split onward is non-fatal.** Once the tab holds three
panes, the guard turns every later run into a no-op. Dying there would report a
layout that was in fact built, and no rerun could ever finish it. A failed tool
command or a failed label is therefore a toast, and the run still succeeds.

`say` still reaches the user either way, as a Herdr toast and in the plugin
command log.

## Messages are toasts as well as stderr

The recipe runs detached, so stderr goes nowhere a human will read. Every message
is therefore **also** a Herdr toast. A silent no-op is the one failure mode worth
engineering against here.

The toast is best-effort and must never mask the real message, which is why it is
suffixed with `|| true`.

Read the log back with:

```sh
herdr plugin log list --plugin mikebronner.agentic-panes-layout
```

## Resolving the herdr binary

`HERDR_BIN_PATH` is injected into both keybinding commands and plugin commands,
so it is the answer whenever either script runs for real. The fallback to the
Homebrew location covers a hand-run invocation from an ordinary shell.

`PLUGIN_ROOT` works the same way: `HERDR_PLUGIN_ROOT` when Herdr injects it, and
otherwise derived from the script's own location, which keeps both files runnable
straight out of a checkout by the test suite and by hand.

## The two splits

The first split divides the arriving pane, so the agent keeps
`AGENT_LAYOUT_RATIO` by **being** the pane named in `--pane`. The second split
divides the pane the first one made, so the tool pane keeps the leading share and
the bare shell takes the remainder beneath it.

Splitting the tool pane rather than the agent pane a second time is what puts the
tool above the shell. Splitting the first pane twice would stack three panes down
the agent's side instead, and every ratio would then apply to the wrong pane.

The second split needs the first one's answer, because a new pane's id is not
derivable. Carrying on with an empty pane id would aim `pane split` and
`pane run` at nothing, so a failed first split must not reach the second.

### `--no-focus` is insurance, not mechanism

The layout opens on the agent for a plainer reason than the flag: the agent
starts in the pane the workspace already arrived on, and nothing in the recipe
moves the cursor off it.

`--no-focus` is kept anyway, so **do not strip it as a dead no-op**. An
accidental `--focus`, or a later Herdr that changes the default, would land the
user in the bare shell the second split makes.

## Labelling

`label_pane` is called three times, written out rather than looped over
`"$pane:$label"` pairs. A pane id is itself colon-separated (`w1:p1`), so packing
the two into one word and splitting on the colon would read the id as the pane
and the pane number as the label.

## The agent name

Derived from the workspace label so `herdr agent prompt <label>` reaches it,
unless `--agent-name` supplied one.

The derivation lowercases, replaces every character outside `[a-z0-9_-]` with
`-`, strips leading and trailing dashes, falls back to `agent` if nothing is
left, and prefixes `a` when the result does not start with a letter.

**It truncates last, not before the prefix.** A 32-character label starting with
a digit would otherwise come out 33 characters and be rejected.

### Deduping a derived name

A derived name can collide: two workspaces whose labels normalise to the same
string derive the same name, and so does a run that meets an agent already live
under it. Herdr refuses the second `agent start` with `agent_name_taken`.

So a derived name **retries**. On `agent_name_taken` the recipe tries
`<base>-2`, then `<base>-3`, and so on, bounded at 20 attempts before it gives
up and dies. Exhausting the bound is a real failure and is reported as one.

**The base is trimmed, not the suffix.** `-17` has to fit inside the same
32-character limit as the name it is appended to, so the base is cut to
`32 - len(suffix)` before the suffix goes on. Trimming the suffix instead would
produce a name that is no longer distinct, which is the one thing the retry
exists to guarantee.

**It is a loop, not a single second attempt**, and it re-reads the error each
time round. Two concurrent processes can both read one failure and both pick
`-2`, so the second of them must be free to fail again and move on to `-3`.

A name from `--agent-name` **never** retries and dies loudly when it is taken.
The caller reserved that exact string so `herdr agent prompt <name>` reaches the
workspace. Quietly starting the agent under a different name would break the one
guarantee the reservation was built for, so the collision is the caller's to
resolve.

### Why the retry lives here and not in the caller

The server arbitrates `agent start` and returns `agent_name_taken` to **every**
caller, so retrying on that error deduplicates across concurrent, unrelated
processes.

That is strictly stronger than an in-process reservation set. Such a set covers
one run of one program, cannot see an agent started by hand, and cannot see the
run happening in the next process along. The error can see all three, because
the server is the only thing that knows every live name at once.

## Classifying the `agent start` result

`agent start` blocks, and this process is already detached, so the 30s default
costs nothing. Blocking is what makes a startup failure reportable at all.
`--timeout` is not passed, because it has a 3000ms minimum.

stderr is captured so the error code can be matched. `2>&1 >/dev/null` redirects
stderr into the substitution and still discards the ordinary stdout. It is
re-emitted on the fatal path, where Herdr's own wording is the useful part of the
log entry.

The `case` has **three** arms, and they are the whole set:

| Error code | Outcome |
| --- | --- |
| `agent_not_ready` | **Success.** The agent did start and is waiting at a prompt. |
| `agent_name_taken` | **Retry** under the next derived name, unless `--agent-name` supplied it. |
| anything else | **Fatal**, `agent_pane_busy` and `timeout` included. |

Fatal is the default arm rather than a list, so a code Herdr adds later is
reported instead of being silently swallowed.

Each match is against the JSON `"code":"<code>"` rather than the bare word. A
message that merely *mentions* `agent_not_ready` is not a payload that *reports*
it, and matching the bare word would turn a real failure green.

## `bin/on-event`: the two gates

### Gate 1: the right event

Only `worktree.created` is acted on. The compare is what makes the
`worktree.opened` subscription log-only for free: Herdr records every event it
spawns the hook for, and gate 1 exits before any layout runs.

That is the whole reason the gate was kept when the manifest was cut to one
acting event. It makes adding a subscription a no-op instead of a bug.

**A non-zero exit is not used to refuse an event.** Herdr would record it as a
failed plugin command, and "this event was not mine" is not a failure.

### Gate 2: the new workspace is the focused one

`bin/agent-layout`, run with no arguments as the hook runs it, targets whatever
workspace is focused. That is correct when the new worktree has focus, and wrong
when it does not, because `worktree create --no-focus` emits `worktree.created`
with `"focused": false`. Acting on it would aim the layout at whatever the user
is looking at and start an agent in it.

So gate 2 **fails closed**. Any missing key or malformed payload raises, python
exits non-zero, and the hook does nothing rather than laying out a stranger.

### Choosing the recipe

A failure to read the settings is fatal rather than "use the defaults". If the
user pointed `AGENT_LAYOUT_RECIPE` somewhere, silently running the bundled recipe
instead would be the wrong layout applied without a word.

A recipe that is named but not runnable is a **broken setting**, not "this event
was not mine". It fails loudly, because falling back to the bundled recipe would
silently apply a layout the user asked to replace.

The handoff is an `exec`, so the recipe owns the exit code and stderr. Herdr
records both in the plugin command log, which is where the recipe's "left alone"
and failure messages become readable. Herdr does not block on event hooks, so
there is no reason to detach.

## `bin/config-env`

Both halves of the plugin read their settings through this one script, so the
`.env` parsing rules live in exactly one place:

```sh
eval "$(/usr/bin/python3 "$PLUGIN_ROOT/bin/config-env")"
```

`bin/on-event` needs `AGENT_LAYOUT_RECIPE`. `bin/agent-layout` needs the other
ten, and is also reachable straight from a keybinding, which never passes through
`bin/on-event` and sees a different environment entirely. One reader covers all
three callers.

### The rules it implements

- **Every value is `shlex`-quoted**, so a tab name containing a space survives
  the `eval`. Without it the shell would split `my tab` into two words and the
  tab would be named `my`.
- **An unset key and an empty key both mean "default".** An `.env` line reading
  `AGENT_LAYOUT_KIND=` is indistinguishable from a missing one, and treating it
  as "no agent kind at all" would only produce a confusing Herdr error.
- **Real environment variables win over the file.**
- **Matched surrounding quotes are stripped.**
- **A line without an `=`, a blank line and a `#` line are skipped**, and the
  file is *not* abandoned at the first bad line. The settings are optional user
  config, so a typo must cost the typo and nothing else.
- **Only the documented keys are emitted.** The output is `eval`'d by a shell, so
  the emitted set is a contract and a stray key from the user's `.env` must not
  become a shell variable.
- **`PLUGIN_ID` must match the id in `herdr-plugin.toml`.**
  `herdr plugin config-dir` is keyed on it, so drift between the two would
  silently read a different directory's `.env`. The test suite pins them
  together.
- **A missing config directory is not an error.** It means "no file", which
  yields the documented defaults, which is what an uninstalled config directory
  should produce anyway. An unreachable `herdr` yields the defaults too rather
  than failing.

### What it deliberately does not do

Nothing here validates a value beyond "is it set". Herdr's own CLI already
rejects a bad `--direction`, `--ratio` or `--kind` with a named error, and
`bin/agent-layout` reports that error. A second copy of Herdr's enumerations here
would only go stale the next time Herdr adds an agent kind.

## The trust boundary

Setting values go straight into herdr's argv, never into a string
`bin/agent-layout` evaluates, so nothing in the `.env` is executed by the recipe.

Two values do reach a shell, one layer further out, and both are shell text **by
design**:

- `AGENT_LAYOUT_TOOL_COMMAND` is typed into the tool pane's own shell by
  `pane run`, which is what makes a command with flags work.
- `AGENT_LAYOUT_RECIPE` is an arbitrary executable.

Both trust the `.env`, which is user-owned config in the plugin's config
directory and not input off the wire.

## Testing

Nothing is imported. Every script is run for real, as a subprocess, against stubs
planted in a temporary directory. That is the only honest way to test these:
`bin/on-event`'s entire job is which process it does or does not exec, and
`bin/agent-layout`'s entire job is which herdr commands it does or does not run.
Stubbing the exec target and the herdr binary is what makes both observable
without splitting a real pane.

The tests run with the launchd `PATH`, because a script that only works because
of the developer's `PATH` is a script that fails in production. Their environment
is built from scratch rather than inherited, so a stray `HERDR_*` or
`AGENT_LAYOUT_*` variable in the developer's shell cannot decide a test.
