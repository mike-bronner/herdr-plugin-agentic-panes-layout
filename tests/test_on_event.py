"""Unit tests for the whole plugin: the two gates, the settings, the layout.

Run: python3 -m unittest discover tests

Nothing is imported. Every script is run for real, as a subprocess, against
stubs planted in a temporary directory. That is the only honest way to test
these: bin/on-event's entire job is which process it does or does not exec, and
bin/agent-layout's entire job is which herdr commands it does or does not run.
Stubbing the exec target and the herdr binary is what makes both observable
without splitting a real pane.

The scripts and the manifest carry no comments. The reasoning behind the code
under test lives in docs/design.md, the measured Herdr behaviour it relies on
lives in docs/herdr-behaviour.md, and the one decision still open lives in
docs/open-questions.md. NoComments at the foot of this file keeps them that way.

Fixtures and constants
----------------------

LAUNCHD_PATH is the PATH Herdr's server really runs with, so the tests run with
it too. A script that only works because of the developer's PATH is a script
that fails in production.

EXPECTED_EVENTS is the manifest's subscription list, in file order. OPENED is
temporary instrumentation: when its exit condition is met it is either promoted
into gate 1 or deleted, and this list changes with it. The exit condition is in
docs/open-questions.md and must be read before that list is touched.

DEFAULTS is the settings bin/config-env is contracted to emit, with the
behaviour that was hardcoded before each became a setting. Pinned here rather
than read from the script, so a silently changed default reddens.

STUB_HERDR stands in for the herdr CLI. It records every invocation to
$STUB_LOG, answers the two list calls from fixture files, and takes its
`agent start` outcome from the environment, so a test can pick success,
agent_not_ready, or a real failure.

`pane split` is the one call with a real answer to model. The recipe reads the
new pane's id back out of it, and the SECOND split targets the pane the FIRST
one returned, so the stub hands out a fresh id each time rather than a fixed
one. It counts calls in a file and answers p2 then p3, which is what lets a test
tell the two panes apart. STUB_SPLIT_STATUS fails the first split and
STUB_SPLIT2_STATUS the second, so each can be exercised on its own.

There are two logs, because "$*" flattens argv. `pane rename p2 "my tool"` and
`pane rename p2 my tool` are the same line in $STUB_LOG and different lines in
$STUB_ARGV_LOG, which is the only way to pin that a label with a space stays ONE
argument. $STUB_LOG stays because it keeps the ordinary assertions readable.

The tomllib banner
------------------

ManifestIsValidToml parses herdr-plugin.toml for real, which needs tomllib,
added in Python 3.11. The interpreter this plugin targets is /usr/bin/python3,
which PY pins for the same reason bin/config-env's shebang spells it: it is the
one Herdr's launchd server can reach. On macOS it is 3.9, so the check has to be
skippable. A skip that reads as a pass would be worse than no check at all,
hence the banner printed once at import, on stderr.
"""
import json, os, re, stat, subprocess, sys, tempfile, unittest

try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None

NO_TOML = ("Python %d.%d.%d has no tomllib, which arrived in 3.11"
           % sys.version_info[:3])

if tomllib is None:
    print("\n%(bar)s\n"
          "!! herdr-plugin.toml WAS NOT CHECKED: %(why)s.\n"
          "!! The manifest parse test is SKIPPED, not passed. Herdr re-reads\n"
          "!! that file at dispatch time, so one syntax error in it stops this\n"
          "!! plugin dispatching, silently and with nothing surfaced. A green\n"
          "!! run below does NOT say the manifest is valid TOML.\n"
          "!! To really check it, re-run under a 3.11+ interpreter:\n"
          "!!     python3.11 -m unittest discover tests\n"
          "%(bar)s\n" % {"bar": "!" * 70, "why": NO_TOML}, file=sys.stderr)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))
HOOK = os.path.join(ROOT, "bin", "on-event")
LAYOUT = os.path.join(ROOT, "bin", "agent-layout")
CONFIG_ENV = os.path.join(ROOT, "bin", "config-env")
MANIFEST = os.path.join(ROOT, "herdr-plugin.toml")

PY = "/usr/bin/python3"
LAUNCHD_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"

CREATED = "worktree.created"
OPENED = "worktree.opened"

EXPECTED_EVENTS = [CREATED, OPENED]

DEFAULTS = {
    "AGENT_LAYOUT_KIND": "claude",
    "AGENT_LAYOUT_DIRECTION": "right",
    "AGENT_LAYOUT_RATIO": "0.5",
    "AGENT_LAYOUT_TAB_NAME": "agent",
    "AGENT_LAYOUT_TOOL_COMMAND": "lazygit",
    "AGENT_LAYOUT_TOOL_DIRECTION": "down",
    "AGENT_LAYOUT_TOOL_RATIO": "0.6",
    "AGENT_LAYOUT_AGENT_LABEL": "agent",
    "AGENT_LAYOUT_TOOL_LABEL": "lazygit",
    "AGENT_LAYOUT_SHELL_LABEL": "shell",
    "AGENT_LAYOUT_RECIPE": "",
}

STUB_HERDR = """#!/bin/sh
printf '%s\\n' "$*" >> "$STUB_LOG"
for a in "$@"; do printf '[%s]' "$a" >> "$STUB_ARGV_LOG"; done
printf '\\n' >> "$STUB_ARGV_LOG"
case "$1 $2" in
    "workspace list")    cat "$STUB_WORKSPACES" ;;
    "pane list")         cat "$STUB_PANES" ;;
    "plugin config-dir") printf '%s\\n' "${STUB_CONFIG_DIR:-}" ;;
    "tab rename")        exit "${STUB_RENAME_STATUS:-0}" ;;
    "pane rename")       exit "${STUB_LABEL_STATUS:-0}" ;;
    "pane run")          exit "${STUB_RUN_STATUS:-0}" ;;
    "pane split")
        n=$(cat "$STUB_SPLIT_COUNT" 2>/dev/null || printf 0)
        n=$((n + 1))
        printf '%s' "$n" > "$STUB_SPLIT_COUNT"
        if [ "$n" = 1 ]; then
            [ "${STUB_SPLIT_STATUS:-0}" = 0 ] || exit "${STUB_SPLIT_STATUS}"
        else
            [ "${STUB_SPLIT2_STATUS:-0}" = 0 ] || exit "${STUB_SPLIT2_STATUS}"
        fi
        printf '{"result":{"pane":{"pane_id":"p%d"}},"type":"pane_info"}\\n' \
            "$((n + 1))"
        ;;
    "notification show") : ;;
    "agent start")
        for taken in ${STUB_TAKEN_NAMES:-}; do
            if [ "$3" = "$taken" ]; then
                printf '{"error":{"code":"agent_name_taken","message":"stub"}\
,"id":"cli:agent:start"}\\n' >&2
                exit 1
            fi
        done
        if [ -n "${STUB_AGENT_STDERR:-}" ]; then
            printf '%s\\n' "$STUB_AGENT_STDERR" >&2
        fi
        exit "${STUB_AGENT_STATUS:-0}"
        ;;
    *)
        printf 'stub herdr: unhandled invocation: %s\\n' "$*" >&2
        exit 99
        ;;
esac
"""


def error_json(code):
    """A herdr CLI error exactly as measured on 0.8.2: compact JSON on stderr."""
    return ('{"error":{"code":"%s","message":"stub"},"id":"cli:agent:start"}'
            % code)


def payload(focused):
    return json.dumps({
        "event": "worktree_created",
        "data": {"type": "worktree_created",
                 "workspace": {"workspace_id": "w9", "focused": focused}},
    })


def opened_payload(already_open, focused=True):
    """A real worktree_opened payload: same `workspace` object as
    worktree_created, plus `already_open`. Shape taken from
    `herdr api schema --json` on 0.8.2 and confirmed against a live event."""
    return json.dumps({
        "event": "worktree_opened",
        "data": {"type": "worktree_opened", "already_open": already_open,
                 "workspace": {"workspace_id": "w9", "focused": focused}},
    })


def write_script(path, body):
    with open(path, "w") as f:
        f.write(body)
    os.chmod(path, 0o755)
    return path


class TempPluginCase(unittest.TestCase):
    """A throwaway sandbox: a config directory, and a stub herdr binary."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.config_dir = os.path.join(self.tmp.name, "config")
        os.makedirs(self.config_dir)
        self.log = os.path.join(self.tmp.name, "herdr.log")
        self.argv_log = os.path.join(self.tmp.name, "herdr-argv.log")
        self.herdr = write_script(os.path.join(self.tmp.name, "herdr"),
                                  STUB_HERDR)

    def write_env(self, text):
        with open(os.path.join(self.config_dir, ".env"), "w") as f:
            f.write(text)

    def base_env(self):
        """Built from scratch, never inherited: a stray HERDR_* or
        AGENT_LAYOUT_* variable in the developer's shell must not decide a
        test."""
        return {"PATH": LAUNCHD_PATH, "HOME": self.tmp.name,
                "HERDR_BIN_PATH": self.herdr, "STUB_LOG": self.log,
                "STUB_ARGV_LOG": self.argv_log,
                "STUB_SPLIT_COUNT": os.path.join(self.tmp.name, "splits")}

    def reset_stub_state(self):
        """Forget every recorded call and the `pane split` counter.

        For a test that runs the script more than once. Without it the previous
        run's log answers the assertion, so an assertIn would pass on a case that
        never really ran, and the split counter would keep counting up and hand
        out pane ids the second run's assertions do not expect.
        """
        for path in (self.log, self.argv_log,
                     os.path.join(self.tmp.name, "splits")):
            if os.path.exists(path):
                os.remove(path)

    def read_log(self, path):
        try:
            with open(path) as f:
                return [line.rstrip("\n") for line in f]
        except OSError:
            return []

    def herdr_calls(self):
        return self.read_log(self.log)

    def herdr_argv(self):
        """Each call as "[herdr][pane][rename][p2][my tool]", so argument
        boundaries are visible rather than flattened by "$*"."""
        return self.read_log(self.argv_log)


class ConfigEnv(TempPluginCase):
    """Every setting reaches both halves of the plugin through this script."""

    def read(self, env=None, config_dir_injected=True):
        e = self.base_env()
        if config_dir_injected:
            e["HERDR_PLUGIN_CONFIG_DIR"] = self.config_dir
        e.update(env or {})
        p = subprocess.run([PY, CONFIG_ENV], env=e, capture_output=True,
                           text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def settings(self, **kwargs):
        """Parse the emitted assignments by having a real shell eval them,
        which is exactly what the two callers do. Anything the quoting gets
        wrong shows up here rather than in production."""
        out = self.read(**kwargs)
        script = out + "\n" + "".join(
            'printf "%%s\\n" "$%s"\n' % k for k in DEFAULTS)
        p = subprocess.run(["/bin/sh", "-c", script],
                           env={"PATH": LAUNCHD_PATH}, capture_output=True,
                           text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        return dict(zip(DEFAULTS, p.stdout.split("\n")))

    def test_no_file_yields_the_documented_defaults(self):
        self.assertEqual(self.settings(), DEFAULTS)

    def test_every_key_can_be_set(self):
        """Every key, and no key left at its default: a setting that silently
        ignores the file is the failure this catches, so the fixture value
        differs from the default in every single row."""
        overrides = {
            "AGENT_LAYOUT_KIND": "codex",
            "AGENT_LAYOUT_DIRECTION": "down",
            "AGENT_LAYOUT_RATIO": "0.3",
            "AGENT_LAYOUT_TAB_NAME": "work",
            "AGENT_LAYOUT_TOOL_COMMAND": "gitui",
            "AGENT_LAYOUT_TOOL_DIRECTION": "right",
            "AGENT_LAYOUT_TOOL_RATIO": "0.75",
            "AGENT_LAYOUT_AGENT_LABEL": "claude",
            "AGENT_LAYOUT_TOOL_LABEL": "git",
            "AGENT_LAYOUT_SHELL_LABEL": "term",
            "AGENT_LAYOUT_RECIPE": "/tmp/mine",
        }
        self.assertEqual(set(overrides), set(DEFAULTS))
        for key, value in overrides.items():
            self.assertNotEqual(value, DEFAULTS[key], key)
        self.write_env("".join("%s=%s\n" % kv for kv in overrides.items()))
        self.assertEqual(self.settings(), overrides)

    def test_one_key_set_leaves_the_others_at_their_defaults(self):
        self.write_env("AGENT_LAYOUT_RATIO=0.25\n")
        got = self.settings()
        self.assertEqual(got["AGENT_LAYOUT_RATIO"], "0.25")
        self.assertEqual(got["AGENT_LAYOUT_KIND"], "claude")
        self.assertEqual(got["AGENT_LAYOUT_TAB_NAME"], "agent")

    def test_real_environment_wins_over_the_file(self):
        self.write_env("AGENT_LAYOUT_KIND=codex\n")
        got = self.settings(env={"AGENT_LAYOUT_KIND": "gemini"})
        self.assertEqual(got["AGENT_LAYOUT_KIND"], "gemini")

    def test_a_malformed_line_does_not_cost_the_settings_after_it(self):
        """Named for what it pins, not for the branch it exercises. Skipping a
        junk line is unobservable on its own, because a key like
        "this line has no equals sign" collides with no setting. What matters,
        and what this reddens on, is the file being abandoned at the first bad
        line: the settings are optional user config, so a typo must cost the
        typo and nothing else. Hence the good line comes last."""
        self.write_env("# a comment\n"
                       "\n"
                       "   \n"
                       "this line has no equals sign\n"
                       "AGENT_LAYOUT_TAB_NAME=work\n")
        self.assertEqual(self.settings()["AGENT_LAYOUT_TAB_NAME"], "work")

    def test_only_the_documented_settings_are_emitted(self):
        """The output is eval'd by a shell, so the emitted set is a contract.
        A stray key from the user's .env must not become a shell variable."""
        self.write_env("SOMETHING_ELSE=1\nAGENT_LAYOUT_KIND=codex\n")
        emitted = [line.split("=")[0] for line in self.read().splitlines()]
        self.assertEqual(emitted, list(DEFAULTS))

    def test_matched_quotes_are_stripped(self):
        self.write_env("AGENT_LAYOUT_TAB_NAME=\"my tab\"\n"
                       "AGENT_LAYOUT_KIND='codex'\n")
        got = self.settings()
        self.assertEqual(got["AGENT_LAYOUT_TAB_NAME"], "my tab")
        self.assertEqual(got["AGENT_LAYOUT_KIND"], "codex")

    def test_a_value_with_a_space_survives_the_eval(self):
        """The whole reason the output is shlex-quoted. Without it the shell
        would split "my tab" into two words and the tab would be named "my"."""
        self.write_env("AGENT_LAYOUT_TAB_NAME=my tab\n")
        self.assertEqual(self.settings()["AGENT_LAYOUT_TAB_NAME"], "my tab")

    def test_a_value_containing_shell_metacharacters_is_not_executed(self):
        self.write_env("AGENT_LAYOUT_TAB_NAME=$(touch /tmp/pwned);x\n")
        self.assertEqual(self.settings()["AGENT_LAYOUT_TAB_NAME"],
                         "$(touch /tmp/pwned);x")

    def test_an_empty_value_falls_back_to_the_default(self):
        """`AGENT_LAYOUT_KIND=` is indistinguishable from a missing line, so it
        must mean the default rather than "no agent kind at all"."""
        self.write_env("AGENT_LAYOUT_KIND=\n")
        self.assertEqual(self.settings()["AGENT_LAYOUT_KIND"], "claude")

    def test_config_dir_is_asked_for_when_herdr_did_not_inject_it(self):
        """The keybinding path. A [[keys.command]] press is handed no
        HERDR_PLUGIN_* variable at all, so the directory has to come from
        `herdr plugin config-dir`."""
        self.write_env("AGENT_LAYOUT_KIND=codex\n")
        got = self.settings(env={"STUB_CONFIG_DIR": self.config_dir},
                            config_dir_injected=False)
        self.assertEqual(got["AGENT_LAYOUT_KIND"], "codex")
        self.assertIn("plugin config-dir mikebronner.agentic-panes-layout",
                      self.herdr_calls())

    def test_an_unreachable_herdr_yields_the_defaults_rather_than_failing(self):
        got = self.settings(
            env={"HERDR_BIN_PATH": os.path.join(self.tmp.name, "nope")},
            config_dir_injected=False)
        self.assertEqual(got, DEFAULTS)


class HookCase(TempPluginCase):
    """The hook is run against a stub plugin root, so "it handed off" is
    observable as the stub recipe's output rather than as a split pane.

    HERDR_PLUGIN_ROOT points at the stub root while the hook itself is run from
    the real checkout, which also pins that the hook resolves the recipe from
    the injected root and never from its working directory.
    """

    def setUp(self):
        super().setUp()
        self.root = os.path.join(self.tmp.name, "plugin")
        os.makedirs(os.path.join(self.root, "bin"))
        self.stub_recipe = write_script(
            os.path.join(self.root, "bin", "agent-layout"),
            "#!/bin/sh\necho RAN\n")
        os.symlink(CONFIG_ENV, os.path.join(self.root, "bin", "config-env"))

    def run_hook(self, event=CREATED, event_json=None, env=None,
                 plugin_root=True, expect_status=0):
        """Return the hook's stdout. "RAN" means it handed off to the recipe."""
        e = self.base_env()
        e["HERDR_PLUGIN_CONFIG_DIR"] = self.config_dir
        if plugin_root:
            e["HERDR_PLUGIN_ROOT"] = self.root
        if event is not None:
            e["HERDR_PLUGIN_EVENT"] = event
        if event_json is not None:
            e["HERDR_PLUGIN_EVENT_JSON"] = event_json
        e.update(env or {})
        p = subprocess.run(["sh", "bin/on-event"], cwd=ROOT, env=e,
                           capture_output=True, text=True)
        self.assertEqual(p.returncode, expect_status, p.stderr)
        self.stderr = p.stderr
        return p.stdout.strip()


class Gate1Event(HookCase):
    """Only worktree.created runs the layout; the others exist to be logged.

    A non-zero exit is not used to refuse an event: Herdr would record it as a
    failed plugin command, and "this event was not mine" is not a failure.
    """

    def test_worktree_created_hands_off(self):
        self.assertEqual(self.run_hook(CREATED, payload(True)), "RAN")

    def test_worktree_opened_does_not(self):
        """The one non-hypothetical case: worktree.opened IS subscribed, so the
        hook really is spawned for it. Gate 1 is the only thing stopping it
        laying out a workspace that may already be laid out."""
        self.assertEqual(self.run_hook(OPENED, opened_payload(True)), "")

    def test_worktree_opened_does_not_even_when_focused_and_fresh(self):
        """already_open=false is the payload that will one day justify acting.
        Until the exit condition in docs/open-questions.md is settled, it must
        not."""
        self.assertEqual(self.run_hook(OPENED, opened_payload(False)), "")

    def test_workspace_created_does_not(self):
        self.assertEqual(self.run_hook("workspace.created", payload(True)), "")

    def test_workspace_focused_does_not(self):
        self.assertEqual(self.run_hook("workspace.focused", payload(True)), "")

    def test_missing_event_variable_does_not(self):
        self.assertEqual(self.run_hook(None, payload(True)), "")


class Gate2Focused(HookCase):
    """bin/agent-layout targets the FOCUSED workspace, so an unfocused create
    must be refused rather than aimed at whatever the user is looking at."""

    def test_unfocused_workspace_is_refused(self):
        self.assertEqual(self.run_hook(CREATED, payload(False)), "")

    def test_malformed_json_is_refused(self):
        self.assertEqual(self.run_hook(CREATED, "not json"), "")

    def test_empty_json_is_refused(self):
        self.assertEqual(self.run_hook(CREATED, ""), "")

    def test_missing_json_variable_is_refused(self):
        self.assertEqual(self.run_hook(CREATED, None), "")

    def test_payload_without_workspace_is_refused(self):
        self.assertEqual(
            self.run_hook(CREATED, '{"event":"worktree_created","data":{}}'), "")


class RecipeSelection(HookCase):
    """Which script the hook execs, and where it looks for it."""

    def custom_recipe(self, body="#!/bin/sh\necho CUSTOM\n", executable=True):
        path = os.path.join(self.tmp.name, "my-recipe")
        with open(path, "w") as f:
            f.write(body)
        if executable:
            os.chmod(path, 0o755)
        else:
            os.chmod(path, 0o644)
        return path

    def test_bundled_recipe_is_found_under_the_injected_plugin_root(self):
        """The stub recipe lives in the temporary root, not in the checkout the
        hook is run from, so "RAN" can only come from HERDR_PLUGIN_ROOT."""
        self.assertEqual(self.run_hook(CREATED, payload(True)), "RAN")

    def test_plugin_root_falls_back_to_the_script_location(self):
        """Nothing injects HERDR_PLUGIN_ROOT when the hook is run by hand or by
        this suite. Without the fallback bin/config-env is not found and the
        hook exits 1, so reaching the recipe at all proves the fallback."""
        recipe = self.custom_recipe()
        out = self.run_hook(CREATED, payload(True), plugin_root=False,
                            env={"AGENT_LAYOUT_RECIPE": recipe})
        self.assertEqual(out, "CUSTOM")

    def test_recipe_setting_replaces_the_bundled_recipe(self):
        recipe = self.custom_recipe()
        self.write_env("AGENT_LAYOUT_RECIPE=%s\n" % recipe)
        self.assertEqual(self.run_hook(CREATED, payload(True)), "CUSTOM")

    def test_recipe_setting_from_the_environment_beats_the_file(self):
        from_file = self.custom_recipe()
        self.write_env("AGENT_LAYOUT_RECIPE=%s\n" % from_file)
        override = os.path.join(self.tmp.name, "override")
        write_script(override, "#!/bin/sh\necho OVERRIDE\n")
        out = self.run_hook(CREATED, payload(True),
                            env={"AGENT_LAYOUT_RECIPE": override})
        self.assertEqual(out, "OVERRIDE")

    def test_an_unrunnable_recipe_fails_loudly(self):
        """A named-but-broken setting is a real failure, unlike a wrong event.
        Falling back to the bundled recipe would silently apply a layout the
        user asked to replace."""
        recipe = self.custom_recipe(executable=False)
        self.write_env("AGENT_LAYOUT_RECIPE=%s\n" % recipe)
        out = self.run_hook(CREATED, payload(True), expect_status=1)
        self.assertEqual(out, "")
        self.assertIn("AGENT_LAYOUT_RECIPE is not executable", self.stderr)

    def test_unreadable_settings_fail_loudly(self):
        """Same reason: a settings reader that cannot run may be hiding an
        AGENT_LAYOUT_RECIPE, so running the bundled one is not a safe default."""
        os.remove(os.path.join(self.root, "bin", "config-env"))
        self.run_hook(CREATED, payload(True), expect_status=1)
        self.assertIn("cannot read the plugin settings", self.stderr)

    def test_the_recipe_is_handed_no_arguments(self):
        """The no-argument contract. bin/agent-layout's default behaviour is
        defined as exactly what this hook gets, so the hook must never start
        passing flags. A recipe that echoes its argv proves the list is empty."""
        recipe = self.custom_recipe(
            body="#!/bin/sh\nprintf 'ARGS[%s]\\n' \"$*\"\n")
        self.write_env("AGENT_LAYOUT_RECIPE=%s\n" % recipe)
        self.assertEqual(self.run_hook(CREATED, payload(True)), "ARGS[]")


WORKSPACES = {"result": {"workspaces": [
    {"workspace_id": "w9", "active_tab_id": "t1", "label": "proj one",
     "focused": True}]}}

ONE_BARE_PANE = {"result": {"panes": [
    {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj", "agent": None}]}}

TWO_WORKSPACES = {"result": {"workspaces": [
    {"workspace_id": "w9", "active_tab_id": "t1", "label": "proj one",
     "focused": True},
    {"workspace_id": "w7", "active_tab_id": "t7", "label": "other proj",
     "focused": False}]}}

ONE_BARE_PANE_IN_T7 = {"result": {"panes": [
    {"pane_id": "p1", "tab_id": "t7", "cwd": "/tmp/other", "agent": None}]}}


class LayoutCase(TempPluginCase):
    """bin/agent-layout run for real against a stub herdr binary.

    TWO_WORKSPACES is the picker's situation, built so that targeting is
    OBSERVABLE. The workspace --workspace names is deliberately NOT the focused
    one, and differs from the focused one in every field the layout reads: a
    different active tab, a different label (so a different derived agent name)
    and a different cwd. Aiming at the focused workspace by mistake therefore
    cannot produce a passing run.
    """

    def setUp(self):
        super().setUp()
        self.workspaces = os.path.join(self.tmp.name, "workspaces.json")
        self.panes = os.path.join(self.tmp.name, "panes.json")
        self.set_workspaces(WORKSPACES)
        self.set_panes(ONE_BARE_PANE)

    def set_workspaces(self, doc):
        with open(self.workspaces, "w") as f:
            json.dump(doc, f)

    def set_panes(self, doc):
        with open(self.panes, "w") as f:
            json.dump(doc, f)

    def run_layout(self, args=(), env=None, expect_status=0):
        """`args` is the argument list, and defaults to EMPTY on purpose.

        Most tests in this file call this with no arguments, which is what pins
        the bare invocation bin/on-event and the README's [[keys.command]]
        example both rely on. A new flag that changed any of those reddens here
        rather than in production.

        The file is run directly, not via `sh <path>`: the executable bit and
        the shebang are what bin/on-event's exec and the keybinding both rely on.
        """
        e = self.base_env()
        e["HERDR_PLUGIN_CONFIG_DIR"] = self.config_dir
        e["STUB_WORKSPACES"] = self.workspaces
        e["STUB_PANES"] = self.panes
        e.update(env or {})
        p = subprocess.run([LAYOUT] + list(args), env=e, capture_output=True,
                           text=True)
        self.assertEqual(p.returncode, expect_status, p.stderr)
        return p.stderr

    def changing_calls(self):
        """Every stub call that would alter a workspace. The read-only calls
        (`workspace list`, `pane list`) and the toast are not layout steps, so a
        refusal is proved by this list being empty rather than the log being."""
        return [c for c in self.herdr_calls()
                if c.startswith(("tab rename", "pane split", "pane run",
                                 "pane rename", "agent start"))]


class LayoutApplies(LayoutCase):
    """Every command that makes up the layout, and the settings each reads."""

    def test_defaults_reach_the_herdr_command_line(self):
        self.run_layout()
        calls = self.herdr_calls()
        self.assertIn("tab rename t1 agent", calls)
        self.assertIn("pane split --pane p1 --direction right --ratio 0.5"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("pane split --pane p2 --direction down --ratio 0.6"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("pane run p2 lazygit", calls)
        self.assertIn("agent start proj-one --kind claude --pane p1", calls)

    def test_settings_reach_the_herdr_command_line(self):
        self.write_env("AGENT_LAYOUT_KIND=codex\n"
                       "AGENT_LAYOUT_DIRECTION=down\n"
                       "AGENT_LAYOUT_RATIO=0.35\n"
                       "AGENT_LAYOUT_TAB_NAME=work\n"
                       "AGENT_LAYOUT_TOOL_COMMAND=gitui\n"
                       "AGENT_LAYOUT_TOOL_DIRECTION=right\n"
                       "AGENT_LAYOUT_TOOL_RATIO=0.75\n")
        self.run_layout()
        calls = self.herdr_calls()
        self.assertIn("tab rename t1 work", calls)
        self.assertIn("pane split --pane p1 --direction down --ratio 0.35"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("pane split --pane p2 --direction right --ratio 0.75"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("pane run p2 gitui", calls)
        self.assertIn("agent start proj-one --kind codex --pane p1", calls)

    def test_the_second_split_targets_the_pane_the_first_one_made(self):
        """The crux of the two-split layout. The tool pane's id is not knowable
        in advance: it only exists in `pane split`'s answer. Splitting p1 twice
        would stack three panes down the agent's side instead of dividing the
        column beside it, and every ratio would then apply to the wrong pane."""
        self.run_layout()
        splits = [c for c in self.herdr_calls() if c.startswith("pane split")]
        self.assertEqual(len(splits), 2)
        self.assertIn("--pane p1", splits[0])
        self.assertIn("--pane p2", splits[1])

    def test_the_tool_command_runs_in_the_tool_pane_not_the_others(self):
        """p2 is the pane the first split made. Running lazygit in p1 would
        replace the agent, and in p3 would fill the bare shell."""
        self.run_layout()
        runs = [c for c in self.herdr_calls() if c.startswith("pane run")]
        self.assertEqual(runs, ["pane run p2 lazygit"])

    def test_all_three_panes_are_labelled(self):
        self.run_layout()
        renames = [c for c in self.herdr_calls()
                   if c.startswith("pane rename")]
        self.assertEqual(renames, ["pane rename p1 agent",
                                   "pane rename p2 lazygit",
                                   "pane rename p3 shell"])

    def test_the_labels_are_settings(self):
        self.write_env("AGENT_LAYOUT_AGENT_LABEL=claude\n"
                       "AGENT_LAYOUT_TOOL_LABEL=git\n"
                       "AGENT_LAYOUT_SHELL_LABEL=term\n")
        self.run_layout()
        renames = [c for c in self.herdr_calls()
                   if c.startswith("pane rename")]
        self.assertEqual(renames, ["pane rename p1 claude",
                                   "pane rename p2 git",
                                   "pane rename p3 term"])

    def test_a_label_containing_a_space_stays_one_argument(self):
        """`pane rename` takes LABEL as variadic, so an unquoted expansion would
        silently work. The tab name has the same property and is already pinned
        in bin/config-env's tests: this pins it at the pane-rename call site."""
        self.write_env("AGENT_LAYOUT_TOOL_LABEL=my tool\n")
        self.run_layout()
        self.assertIn("[pane][rename][p2][my tool]", self.herdr_argv())
        self.assertNotIn("[pane][rename][p2][my][tool]", self.herdr_argv())

    def test_the_agent_name_is_derived_from_the_workspace_label(self):
        self.set_workspaces({"result": {"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1",
             "label": "9 Bible/Models", "focused": True}]}})
        self.run_layout()
        self.assertIn("agent start a9-bible-models --kind claude --pane p1",
                      self.herdr_calls())

    def test_a_failed_rename_stops_before_the_split(self):
        err = self.run_layout(env={"STUB_RENAME_STATUS": "1"}, expect_status=1)
        self.assertIn("could not rename the tab", err)
        self.assertNotIn("pane split --pane p1 --direction right --ratio 0.5"
                         " --cwd /tmp/proj --no-focus", self.herdr_calls())

    def test_a_failed_split_names_the_settings_it_used(self):
        err = self.run_layout(env={"STUB_SPLIT_STATUS": "1"}, expect_status=1)
        self.assertIn("could not split the pane (right, ratio 0.5)", err)

    def test_a_failed_first_split_never_reaches_the_second(self):
        """The second split needs the first one's answer. Carrying on with an
        empty pane id would aim `pane split` and `pane run` at nothing."""
        self.run_layout(env={"STUB_SPLIT_STATUS": "1"}, expect_status=1)
        calls = self.herdr_calls()
        self.assertEqual([c for c in calls if c.startswith("pane split")],
                         ["pane split --pane p1 --direction right --ratio 0.5"
                          " --cwd /tmp/proj --no-focus"])
        self.assertEqual([c for c in calls if c.startswith("pane run")], [])

    def test_a_failed_second_split_is_fatal_and_names_its_own_settings(self):
        """Distinct from the first split's message, which names the other pair
        of settings. Reporting the wrong ratio would send the user to the wrong
        line of their .env."""
        err = self.run_layout(env={"STUB_SPLIT2_STATUS": "1"}, expect_status=1)
        self.assertIn("could not split the tool pane (down, ratio 0.6)", err)
        self.assertNotIn("could not split the pane (", err)

    def test_a_failed_second_split_stops_before_running_the_tool(self):
        self.run_layout(env={"STUB_SPLIT2_STATUS": "1"}, expect_status=1)
        self.assertEqual([c for c in self.herdr_calls()
                          if c.startswith(("pane run", "pane rename",
                                           "agent start"))], [])

    def test_no_focused_workspace_is_fatal(self):
        self.set_workspaces({"result": {"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1", "label": "x",
             "focused": False}]}})
        err = self.run_layout(expect_status=1)
        self.assertIn("cannot read a focused workspace", err)


class PostStructureFailures(LayoutCase):
    """Once the panes exist, a failure is reported and the run still succeeds.

    The split of responsibility is deliberate and is the mirror of
    LayoutApplies' fatal cases. A failed TAB rename happens before any pane is
    created, so dying leaves a clean single pane the next run can lay out. A
    failed lazygit or label happens after three panes exist, where the guard in
    LayoutGuards makes every later run a no-op — so exiting non-zero would
    record a layout that was in fact built as a failure, and nothing could ever
    finish it. The toast still fires either way.
    """

    def test_a_failed_tool_command_warns_but_the_layout_stands(self):
        err = self.run_layout(env={"STUB_RUN_STATUS": "1"})
        self.assertIn('"lazygit" would not run', err)

    def test_a_failed_tool_command_does_not_stop_the_labels_or_the_agent(self):
        self.run_layout(env={"STUB_RUN_STATUS": "1"})
        calls = self.herdr_calls()
        self.assertIn("pane rename p3 shell", calls)
        self.assertIn("agent start proj-one --kind claude --pane p1", calls)

    def test_a_failed_label_warns_but_the_layout_stands(self):
        err = self.run_layout(env={"STUB_LABEL_STATUS": "1"})
        self.assertIn("could not label pane p1", err)

    def test_a_failed_label_does_not_stop_the_remaining_labels(self):
        """One unlabelled pane must not cost the other two their labels."""
        self.run_layout(env={"STUB_LABEL_STATUS": "1"})
        renames = [c for c in self.herdr_calls()
                   if c.startswith("pane rename")]
        self.assertEqual(len(renames), 3)

    def test_a_failed_label_does_not_stop_the_agent(self):
        self.run_layout(env={"STUB_LABEL_STATUS": "1"})
        self.assertIn("agent start proj-one --kind claude --pane p1",
                      self.herdr_calls())


class LayoutGuards(LayoutCase):
    """Running twice must be a no-op, not a second split."""

    def test_a_tab_that_is_already_split_is_left_alone(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj"},
            {"pane_id": "p2", "tab_id": "t1", "cwd": "/tmp/proj"}]}})
        err = self.run_layout()
        self.assertIn("active tab has 2 panes, not 1 — left alone", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_pane_already_running_an_agent_is_left_alone(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}})
        err = self.run_layout()
        self.assertIn("pane already runs claude — left alone", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_pane_in_another_tab_is_not_counted(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": None},
            {"pane_id": "p9", "tab_id": "t2", "cwd": "/tmp/other"}]}})
        self.run_layout()
        self.assertIn("agent start proj-one --kind claude --pane p1",
                      self.herdr_calls())

    def test_a_pane_with_no_working_directory_is_fatal(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": None, "agent": None}]}})
        err = self.run_layout(expect_status=1)
        self.assertIn("pane reports no working directory", err)


class AgentStartOutcome(LayoutCase):
    """`agent start` exiting non-zero is not automatically a failure.

    Herdr's own docs on 0.8.2: "If the agent is blocked during startup, the
    command returns `agent_not_ready` immediately but keeps the name available
    for `agent read` and `agent send-keys`." The agent DID start. On a fresh
    worktree that is Claude Code's trust-this-folder prompt, which is the normal
    case, and reporting it as a failed layout was the bug.
    """

    def test_success_reports_the_layout_it_applied(self):
        err = self.run_layout()
        self.assertIn("claude started as \"proj-one\"", err)
        self.assertIn("lazygit down beside it", err)
        self.assertIn("tab named agent", err)

    def test_agent_not_ready_is_a_success_with_its_own_message(self):
        err = self.run_layout(env={
            "STUB_AGENT_STATUS": "1",
            "STUB_AGENT_STDERR": error_json("agent_not_ready")})
        self.assertIn("waiting on a prompt", err)
        self.assertNotIn("did not start", err)

    def test_agent_pane_busy_is_still_fatal(self):
        """Deliberately out of scope: the pane's shell is not yet interactive
        when the hook fires, and Herdr 0.8.2 exposes no readiness probe to wait
        on. Reclassifying this one would hide a layout that really did fail."""
        err = self.run_layout(
            env={"STUB_AGENT_STATUS": "1",
                 "STUB_AGENT_STDERR": error_json("agent_pane_busy")},
            expect_status=1)
        self.assertIn("did not start", err)
        self.assertNotIn("waiting on a prompt", err)

    def test_any_other_error_code_is_still_fatal(self):
        err = self.run_layout(
            env={"STUB_AGENT_STATUS": "1",
                 "STUB_AGENT_STDERR": error_json("unsupported_agent_kind")},
            expect_status=1)
        self.assertIn("did not start", err)

    def test_a_failure_with_no_payload_at_all_is_still_fatal(self):
        """Fail closed: an unrecognised failure must never be read as the one
        benign code."""
        err = self.run_layout(env={"STUB_AGENT_STATUS": "1"}, expect_status=1)
        self.assertIn("did not start", err)

    def test_herdrs_own_error_text_reaches_the_log(self):
        """stderr is captured to classify it, so it has to be re-emitted or the
        useful half of the plugin log entry disappears."""
        err = self.run_layout(
            env={"STUB_AGENT_STATUS": "1",
                 "STUB_AGENT_STDERR": error_json("agent_pane_busy")},
            expect_status=1)
        self.assertIn('"code":"agent_pane_busy"', err)

    def test_the_code_is_matched_not_merely_mentioned(self):
        """A message that talks about agent_not_ready is not a payload that
        reports it. Matching the bare word would turn a real failure green."""
        err = self.run_layout(
            env={"STUB_AGENT_STATUS": "1",
                 "STUB_AGENT_STDERR": '{"error":{"code":"agent_pane_busy",'
                                      '"message":"agent_not_ready"},'
                                      '"id":"cli:agent:start"}'},
            expect_status=1)
        self.assertIn("did not start", err)


class ArgumentParsing(LayoutCase):
    """A bad argument list is refused before anything is read or changed.

    Every case here asserts changing_calls() is empty, which is the point: the
    parse happens first, so a usage error must cost the caller nothing.
    Asserting the message alone would pass just as well on a script that dies
    halfway through a layout it had already half-applied.
    """

    def test_an_unknown_argument_is_fatal(self):
        """Fail closed. A caller that misspells a flag must get an error rather
        than a layout aimed somewhere it did not ask for."""
        err = self.run_layout(["--workspaces", "w7"], expect_status=1)
        self.assertIn("unknown argument: --workspaces", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_removed_flag_is_rejected_like_any_other_unknown_one(self):
        """--no-agent was considered and deliberately not built: the first pane
        always holds the agent. It must read as an unknown flag, not be quietly
        ignored, or a caller written against the discarded design would silently
        get an agent it asked not to have."""
        err = self.run_layout(["--no-agent"], expect_status=1)
        self.assertIn("unknown argument: --no-agent", err)
        self.assertEqual(self.changing_calls(), [])

    def test_nothing_is_even_read_before_the_arguments_are_parsed(self):
        """Stronger than the guard above: not one API call is made, so a usage
        error cannot leave a workspace half-inspected either."""
        self.run_layout(["--nope"], expect_status=1)
        self.assertEqual([c for c in self.herdr_calls()
                          if not c.startswith("notification show")], [])

    def test_workspace_without_a_value_is_fatal(self):
        err = self.run_layout(["--workspace"], expect_status=1)
        self.assertIn("--workspace needs a workspace id", err)
        self.assertEqual(self.changing_calls(), [])

    def test_an_empty_workspace_id_is_fatal_rather_than_the_focused_one(self):
        """The fail-open this closes. `--workspace ""` reads as "no id given" to
        a bare -n test, which would silently lay out the FOCUSED workspace: the
        one workspace a caller passing --workspace certainly did not mean."""
        err = self.run_layout(["--workspace", ""], expect_status=1)
        self.assertIn("--workspace needs a workspace id", err)
        self.assertEqual(self.changing_calls(), [])

    def test_agent_name_without_a_value_is_fatal(self):
        err = self.run_layout(["--agent-name"], expect_status=1)
        self.assertIn("--agent-name needs an agent name", err)
        self.assertEqual(self.changing_calls(), [])

    def test_an_empty_agent_name_is_fatal_rather_than_the_derived_one(self):
        err = self.run_layout(["--agent-name", ""], expect_status=1)
        self.assertIn("--agent-name needs an agent name", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_repeated_workspace_is_fatal(self):
        """Last-wins would hide the caller's own confusion at the one moment it
        could still be fixed. The two ids differ so that a silently accepted
        second one would be observable."""
        err = self.run_layout(["--workspace", "w7", "--workspace", "w9"],
                              expect_status=1)
        self.assertIn("--workspace given more than once", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_repeated_agent_name_is_fatal(self):
        err = self.run_layout(["--agent-name", "one", "--agent-name", "two"],
                              expect_status=1)
        self.assertIn("--agent-name given more than once", err)
        self.assertEqual(self.changing_calls(), [])

    def test_a_flag_repeated_with_the_same_value_is_still_fatal(self):
        """The rule is about the caller not knowing what it asks, so an
        identical repeat is refused too. Comparing values instead would let
        `--workspace w7 --workspace w7` through and make the rule depend on a
        coincidence."""
        err = self.run_layout(["--workspace", "w7", "--workspace", "w7"],
                              expect_status=1)
        self.assertIn("--workspace given more than once", err)
        self.assertEqual(self.changing_calls(), [])

    def test_both_flags_together_are_not_a_repeat(self):
        """The repeat check is per flag. Guarding them with one shared variable
        would reject the picker's own call."""
        self.set_workspaces(TWO_WORKSPACES)
        self.set_panes(ONE_BARE_PANE_IN_T7)
        self.run_layout(["--workspace", "w7", "--agent-name", "reserved-2"])
        self.assertIn("agent start reserved-2 --kind claude --pane p1",
                      self.herdr_calls())


class WorkspaceTargeting(LayoutCase):
    """--workspace lays out the workspace it names, focused or not."""

    def setUp(self):
        super().setUp()
        self.set_workspaces(TWO_WORKSPACES)
        self.set_panes(ONE_BARE_PANE_IN_T7)

    def test_no_arguments_still_picks_the_focused_workspace(self):
        """The bare invocation, on the very fixture that could hide a regression
        in it. bin/on-event and the keybinding both pass no arguments, so the
        focused workspace must still win when another one exists to be chosen by
        mistake. w9's panes live in t1, so the layout stops at the guard, which
        is itself the proof that w9 and not w7 was resolved."""
        err = self.run_layout()
        self.assertIn("proj one: active tab has 0 panes", err)
        self.assertIn("pane list --workspace w9", self.herdr_calls())
        self.assertNotIn("pane list --workspace w7", self.herdr_calls())

    def test_the_named_workspace_is_laid_out_though_it_is_not_focused(self):
        self.run_layout(["--workspace", "w7"])
        calls = self.herdr_calls()
        self.assertIn("pane list --workspace w7", calls)
        self.assertIn("tab rename t7 agent", calls)
        self.assertIn("pane split --pane p1 --direction right --ratio 0.5"
                      " --cwd /tmp/other --no-focus", calls)

    def test_the_agent_name_comes_from_the_named_workspaces_label(self):
        """The derivation reads LABEL, so targeting the wrong workspace would
        start the agent under the wrong name even if every id were right."""
        self.run_layout(["--workspace", "w7"])
        self.assertIn("agent start other-proj --kind claude --pane p1",
                      self.herdr_calls())
        self.assertNotIn("agent start proj-one --kind claude --pane p1",
                         self.herdr_calls())

    def test_an_unknown_workspace_id_is_fatal_and_lays_nothing_out(self):
        """Falling back to the focused workspace on a stale id would lay out
        whatever the user happens to be looking at, which is the exact accident
        --workspace exists to prevent."""
        err = self.run_layout(["--workspace", "w404"], expect_status=1)
        self.assertIn('no workspace "w404" in the Herdr server', err)
        self.assertEqual(self.changing_calls(), [])

    def test_the_unknown_id_message_is_not_the_focused_one(self):
        """Two different failures. Telling a caller with a stale id that no
        workspace is focused sends it looking in the wrong place."""
        err = self.run_layout(["--workspace", "w404"], expect_status=1)
        self.assertNotIn("cannot read a focused workspace", err)

    def test_no_focused_workspace_does_not_stop_a_targeted_run(self):
        """The picker's own situation: it opens with --no-focus, so a pass can
        reach this script with nothing focused at all."""
        self.set_workspaces({"result": {"workspaces": [
            {"workspace_id": "w7", "active_tab_id": "t7", "label": "other proj",
             "focused": False}]}})
        self.run_layout(["--workspace", "w7"])
        self.assertIn("agent start other-proj --kind claude --pane p1",
                      self.herdr_calls())

    def test_the_guard_reads_the_named_workspace_not_the_focused_one(self):
        """The guard is what makes a second run a no-op, and it must guard the
        workspace being laid out. Reading the focused workspace's panes instead
        would re-split a target that is already laid out."""
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t7", "cwd": "/tmp/other"},
            {"pane_id": "p2", "tab_id": "t7", "cwd": "/tmp/other"}]}})
        err = self.run_layout(["--workspace", "w7"])
        self.assertIn("other proj: active tab has 2 panes, not 1 — left alone",
                      err)
        self.assertEqual(self.changing_calls(), [])


class SuppliedAgentName(LayoutCase):
    """--agent-name replaces the derived name, and is passed through verbatim."""

    def test_the_supplied_name_reaches_agent_start(self):
        self.run_layout(["--agent-name", "picked-name"])
        self.assertIn("agent start picked-name --kind claude --pane p1",
                      self.herdr_calls())
        self.assertNotIn("agent start proj-one --kind claude --pane p1",
                         self.herdr_calls())

    def test_the_supplied_name_is_not_normalised(self):
        """The load-bearing one. The picker reserves an exact string in its batch
        set so `herdr agent prompt <name>` reaches the workspace, so this script
        must not lowercase, prefix or truncate it. The derivation would turn each
        of these into something else, which is why they are the fixture."""
        for supplied in ("Weird_Name", "9leading", "x" * 40):
            with self.subTest(supplied=supplied):
                self.reset_stub_state()
                self.run_layout(["--agent-name", supplied])
                self.assertIn(
                    "agent start %s --kind claude --pane p1" % supplied,
                    self.herdr_calls())

    def test_the_supplied_name_stays_one_argument(self):
        """An invalid name is Herdr's to reject, which it can only do if the
        whole string reaches it as one argv element rather than word-split."""
        self.run_layout(["--agent-name", "two words"])
        self.assertIn("[agent][start][two words][--kind][claude][--pane][p1]",
                      self.herdr_argv())

    def test_a_supplied_name_still_reports_the_layout_it_applied(self):
        err = self.run_layout(["--agent-name", "picked-name"])
        self.assertIn('claude started as "picked-name"', err)

    def test_the_name_is_supplied_alongside_a_targeted_workspace(self):
        """Both flags at once, which is how the picker calls this script: it
        detaches the whole call with the workspace it opened and the name it
        reserved."""
        self.set_workspaces(TWO_WORKSPACES)
        self.set_panes(ONE_BARE_PANE_IN_T7)
        self.run_layout(["--workspace", "w7", "--agent-name", "reserved-2"])
        calls = self.herdr_calls()
        self.assertIn("tab rename t7 agent", calls)
        self.assertIn("agent start reserved-2 --kind claude --pane p1", calls)


class DerivedNameCollision(LayoutCase):
    """A DERIVED name that Herdr says is taken is retried with a suffix.

    The Herdr server is the arbiter, not an in-process set: `agent start`
    answers agent_name_taken to every caller alike, so it also sees a `claude`
    somebody started by hand and a second process racing this one. That is why
    the dedupe lives here rather than in a caller.

    The derivation is byte-identical to herdr-plugin-project-finder's
    agent_name(), suffix included, so the two cannot drift apart.
    """

    def starts(self):
        return [c for c in self.herdr_calls() if c.startswith("agent start")]

    def test_a_taken_derived_name_retries_with_a_suffix(self):
        self.run_layout(env={"STUB_TAKEN_NAMES": "proj-one"})
        self.assertIn("agent start proj-one-2 --kind claude --pane p1",
                      self.herdr_calls())

    def test_the_suffix_increments_until_a_free_name_is_found(self):
        """Also pins that this is a REAL loop that re-reads the error each time,
        rather than a suffix computed once from the first failure. The exact
        call sequence is asserted, so a one-shot implementation that jumped
        straight to -3 would redden even though it found a free name."""
        self.run_layout(env={"STUB_TAKEN_NAMES": "proj-one proj-one-2"})
        self.assertEqual(self.starts(), [
            "agent start proj-one --kind claude --pane p1",
            "agent start proj-one-2 --kind claude --pane p1",
            "agent start proj-one-3 --kind claude --pane p1"])

    def test_a_free_name_is_not_retried_at_all(self):
        """The loop must cost nothing in the ordinary case."""
        self.run_layout()
        self.assertEqual(len(self.starts()), 1)

    def test_the_base_is_trimmed_so_the_suffix_fits_in_32_characters(self):
        """Herdr names are at most 32 characters, and the picker trims the BASE
        rather than the suffix. Trimming the suffix instead would produce a
        33-character name that Herdr rejects, and would also let two different
        bases collapse onto one name."""
        self.set_workspaces({"result": {"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1",
             "label": "a" * 40, "focused": True}]}})
        base = "a" * 32
        self.run_layout(env={"STUB_TAKEN_NAMES": base})
        expected = "a" * 30 + "-2"
        self.assertEqual(len(expected), 32)
        self.assertIn("agent start %s --kind claude --pane p1" % expected,
                      self.herdr_calls())

    def test_the_success_message_names_the_agent_that_actually_started(self):
        """Reporting the base name after falling through to a suffix would send
        the reader to `herdr agent prompt proj-one`, which reaches somebody
        else's agent."""
        err = self.run_layout(env={"STUB_TAKEN_NAMES": "proj-one"})
        self.assertIn('claude started as "proj-one-2"', err)
        self.assertNotIn('started as "proj-one"', err)

    def test_agent_not_ready_on_a_retried_name_is_still_a_success(self):
        """The two classifications compose: the retry finds a free name, and
        that attempt then reports the agent is waiting at a prompt."""
        err = self.run_layout(env={
            "STUB_TAKEN_NAMES": "proj-one",
            "STUB_AGENT_STATUS": "1",
            "STUB_AGENT_STDERR": error_json("agent_not_ready")})
        self.assertIn('started as "proj-one-2" and is waiting on a prompt', err)

    def test_another_error_code_during_the_retry_is_still_fatal(self):
        """Only agent_name_taken is retried. A different failure on the second
        attempt must stop, not spin."""
        err = self.run_layout(
            env={"STUB_TAKEN_NAMES": "proj-one",
                 "STUB_AGENT_STATUS": "1",
                 "STUB_AGENT_STDERR": error_json("agent_pane_busy")},
            expect_status=1)
        self.assertIn("did not start", err)
        self.assertEqual(len(self.starts()), 2)

    def test_exhausting_the_retries_is_fatal_and_says_what_it_tried(self):
        """An unbounded retry against an error that is not really about the name
        would spin forever, so there is a cap. Exhausting it must fail loudly:
        a silent give-up is the failure mode this whole change removes."""
        taken = ["proj-one"] + ["proj-one-%d" % n for n in range(2, 21)]
        err = self.run_layout(env={"STUB_TAKEN_NAMES": " ".join(taken)},
                              expect_status=1)
        self.assertIn('every agent name from "proj-one" to "proj-one-20"'
                      ' is already taken', err)

    def test_the_cap_is_reached_rather_than_overshot(self):
        """One attempt per name and no more, so the bound is exactly the cap."""
        taken = ["proj-one"] + ["proj-one-%d" % n for n in range(2, 21)]
        self.run_layout(env={"STUB_TAKEN_NAMES": " ".join(taken)},
                        expect_status=1)
        self.assertEqual(len(self.starts()), 20)

    def test_herdrs_own_taken_payload_reaches_the_log(self):
        taken = ["proj-one"] + ["proj-one-%d" % n for n in range(2, 21)]
        err = self.run_layout(env={"STUB_TAKEN_NAMES": " ".join(taken)},
                              expect_status=1)
        self.assertIn('"code":"agent_name_taken"', err)

    def test_a_supplied_name_that_is_taken_is_fatal_and_never_retried(self):
        """The retry is for DERIVED names only. A caller that reserved an exact
        string did so precisely so `herdr agent prompt <name>` reaches this
        workspace, and silently starting the agent as something else defeats the
        reservation more completely than refusing does."""
        err = self.run_layout(["--agent-name", "reserved-2"],
                              env={"STUB_TAKEN_NAMES": "reserved-2"},
                              expect_status=1)
        self.assertIn('the agent name "reserved-2" given with --agent-name'
                      ' is already taken', err)
        self.assertEqual(self.starts(),
                         ["agent start reserved-2 --kind claude --pane p1"])

    def test_a_supplied_name_is_not_silently_suffixed(self):
        """The specific accident the rule above prevents."""
        self.run_layout(["--agent-name", "reserved-2"],
                        env={"STUB_TAKEN_NAMES": "reserved-2"},
                        expect_status=1)
        self.assertNotIn("agent start reserved-2-2 --kind claude --pane p1",
                         self.herdr_calls())


class Manifest(unittest.TestCase):
    """Exactly EXPECTED_EVENTS, in order, and nothing else.

    Both halves matter. Losing worktree.created stops the layout entirely.
    Gaining an unplanned subscription is worse than useless: the events fire
    ~1ms apart, Herdr does not serialize hooks, and bin/agent-layout's guard
    reads pane count over the API, so two concurrent ACTING copies would both
    see one pane and both would split it. Gate 1 in bin/on-event absorbs that,
    which is what makes worktree.opened safe to subscribe as log-only
    instrumentation. The full reasoning is in docs/open-questions.md.

    Pinned to the exact list on purpose, never a membership check: the whole
    value of this test is catching a subscription nobody argued for. A
    deliberate change to the list is a one-line edit here, with the exit
    condition in docs/open-questions.md resolved alongside it.
    """

    def events(self):
        with open(MANIFEST) as f:
            return [line.split('"')[1] for line in f
                    if line.startswith('on = "')]

    def test_subscribes_worktree_created(self):
        self.assertIn(CREATED, self.events())

    def test_subscribes_nothing_else(self):
        self.assertEqual(self.events(), EXPECTED_EVENTS)

    def test_the_plugin_id_matches_the_one_config_env_asks_herdr_about(self):
        """bin/config-env passes its own copy of the id to
        `herdr plugin config-dir`. A drift between the two would read a
        different directory's .env and silently apply the defaults."""
        with open(MANIFEST) as f:
            declared = [line.split('"')[1] for line in f
                        if line.startswith('id = "')]
        with open(CONFIG_ENV) as f:
            asked = [line.split('"')[1] for line in f
                     if line.startswith("PLUGIN_ID = ")]
        self.assertEqual(declared, asked)

    def test_the_readme_documents_exactly_the_settings_that_exist(self):
        """The README lists every key and its default by hand, so it drifts the
        moment a setting is added or renamed. Both directions matter: an
        undocumented setting is invisible to the user, and a documented one that
        no longer exists sends them to edit a key nothing reads."""
        with open(os.path.join(ROOT, "README.md")) as f:
            readme = f.read()
        documented = set(re.findall(r"AGENT_LAYOUT_[A-Z_]+", readme))
        self.assertEqual(documented, set(DEFAULTS))

    def test_the_readme_states_each_default_value(self):
        """Pinning the names alone would let `(default: 0.6)` rot into a lie
        the next time a default moves, which is the kind of stale number a
        reader trusts."""
        with open(os.path.join(ROOT, "README.md")) as f:
            readme = f.read()
        for key, default in DEFAULTS.items():
            if not default:
                continue
            self.assertIn(default, readme, "%s default %r" % (key, default))

    def test_the_recipe_is_executable(self):
        """bin/on-event execs it and the keybinding runs it directly, so a lost
        executable bit breaks both entry paths. bin/on-event and bin/config-env
        are always spawned through an interpreter and do not need one."""
        self.assertTrue(os.stat(LAYOUT).st_mode & stat.S_IXUSR)


class NoComments(unittest.TestCase):
    """The scripts and the manifest carry no comments, and stay that way.

    This is not style policing. The rationale and the measured Herdr behaviour
    those comments used to hold now live in docs/, and a comment creeping back
    into a script is the first step to the two copies disagreeing. Anything
    worth saying about this code belongs in docs/design.md,
    docs/herdr-behaviour.md or docs/open-questions.md.

    Docstrings are documentation rather than comments, so this file keeps its
    own. It is checked here too, for its `#` lines only.
    """

    FILES = ["bin/agent-layout", "bin/on-event", "bin/config-env",
             "herdr-plugin.toml", "tests/test_on_event.py"]

    DOCS = ["docs/design.md", "docs/herdr-behaviour.md",
            "docs/open-questions.md"]

    def comment_lines(self, path):
        """Every `#` line of `path`, excluding a line-1 shebang, as
        (lineno, text). An indented comment counts, which is why the test is on
        the stripped line rather than the raw one."""
        with open(path) as f:
            lines = f.read().splitlines()
        return [(n, line) for n, line in enumerate(lines, 1)
                if line.lstrip().startswith("#")
                and not (n == 1 and line.startswith("#!"))]

    def test_no_file_under_test_carries_a_comment(self):
        for relative in self.FILES:
            with self.subTest(path=relative):
                found = self.comment_lines(os.path.join(ROOT, relative))
                self.assertEqual(
                    found, [],
                    "%s carries %d comment line(s); move the content into "
                    "docs/ instead" % (relative, len(found)))

    def test_the_check_really_finds_a_comment(self):
        """A canary on the test above, which would pass just as happily if
        comment_lines() were broken. It runs the SAME method, so a broken
        detector reddens here rather than passing everywhere.

        The fixture also pins the two exemptions: a line-1 shebang is not a
        comment, and an indented comment is."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        planted = os.path.join(tmp.name, "planted.sh")
        with open(planted, "w") as f:
            f.write("#!/bin/sh\ntrue\n    # an indented comment\n# a bare one\n")
        self.assertEqual(self.comment_lines(planted),
                         [(3, "    # an indented comment"),
                          (4, "# a bare one")])

    def test_the_documentation_that_replaced_them_exists(self):
        """Deleting a comment is only safe because its content moved. A docs
        file going missing must redden here rather than at the next reader."""
        for relative in self.DOCS:
            with self.subTest(path=relative):
                path = os.path.join(ROOT, relative)
                self.assertTrue(os.path.isfile(path), path)
                self.assertGreater(os.path.getsize(path), 1000, path)


class ManifestIsValidToml(unittest.TestCase):
    """The manifest has to PARSE, which nothing else in this suite checks.

    Measured on 0.8.2, 2026-09-08: Herdr re-reads herdr-plugin.toml from disk
    when it dispatches an event, rather than trusting the copy it cached in
    plugins.json. An edit takes effect on the very next event — no re-link, no
    restart, no reload-config. This is the other half of that: a syntax error
    in the manifest stops every dispatch for this plugin, with no toast, no
    error and nothing surfaced. The plugin simply goes quiet, and the first
    sign is a worktree that never gets laid out.

    The Manifest class above reads this same file, but by line prefix, so it
    passes happily on a file Herdr itself cannot load. This class is the one
    that would not.

    Skipped, loudly, where tomllib is unavailable: see the banner at the top of
    this file. A skip is not a pass.
    """

    def setUp(self):
        if tomllib is None:
            self.skipTest("herdr-plugin.toml was NOT parsed: " + NO_TOML)

    def parse(self, path):
        """The manifest at `path`, read exactly as Herdr's loader would."""
        with open(path, "rb") as f:
            return tomllib.load(f)

    def test_the_manifest_parses(self):
        try:
            self.parse(MANIFEST)
        except tomllib.TOMLDecodeError as e:
            self.fail("herdr-plugin.toml is not valid TOML: %s" % e)

    def test_a_typo_in_the_manifest_is_really_caught(self):
        """A canary on the test above, which would pass for two very different
        reasons: the manifest is valid, or nothing is really parsing it.

        The fixture is the REAL manifest plus one unterminated string, which is
        what a typo looks like, written to a temporary directory. Corrupting
        the real file to prove the point would be the same class of mistake
        this test exists to catch.
        """
        with open(MANIFEST, "rb") as f:
            typo = f.read() + b'\nname = "unterminated\n'
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        broken = os.path.join(tmp.name, "herdr-plugin.toml")
        with open(broken, "wb") as f:
            f.write(typo)
        with self.assertRaises(tomllib.TOMLDecodeError):
            self.parse(broken)


if __name__ == "__main__":
    unittest.main()
