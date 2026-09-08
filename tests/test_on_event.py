"""Unit tests for the whole plugin: the two gates, the settings, the layout.

Run: python3 -m unittest discover tests

Nothing is imported. Every script is run for real, as a subprocess, against
stubs planted in a temporary directory. That is the only honest way to test
these: bin/on-event's entire job is which process it does or does not exec, and
bin/agent-layout's entire job is which herdr commands it does or does not run.
Stubbing the exec target and the herdr binary is what makes both observable
without splitting a real pane.
"""
import json, os, re, stat, subprocess, sys, tempfile, unittest

# ManifestIsValidToml at the foot of this file parses herdr-plugin.toml for
# real, which needs tomllib — added in Python 3.11. The interpreter this plugin
# targets is /usr/bin/python3, which PY below pins for the same reason
# bin/config-env's shebang spells it: it is the one Herdr's launchd server can
# reach. On macOS it is 3.9, so the check has to be skippable.
#
# A skip that reads as a pass would be worse than no check at all, hence the
# banner: it is printed once, at import, on stderr, so no green run can be
# mistaken for a checked manifest.
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
# Herdr's server runs under launchd with exactly this PATH, so the tests run
# with it too: a script that only works because of the developer's PATH is a
# script that fails in production.
LAUNCHD_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"

CREATED = "worktree.created"
OPENED = "worktree.opened"  # subscribed, but log-only by design

# The subscription list the manifest is pinned to, in file order. OPENED is
# temporary instrumentation — when its exit condition is met it is either
# promoted into gate 1 or deleted, and this list changes with it.
EXPECTED_EVENTS = [CREATED, OPENED]

# The settings bin/config-env is contracted to emit, with the behaviour that was
# hardcoded before each became a setting. Pinned here, not read from the script,
# so a silently changed default reddens.
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

# Stand-in for the herdr CLI. Records every invocation to $STUB_LOG, answers the
# two list calls from fixture files, and takes its `agent start` outcome from the
# environment so a test can pick success, agent_not_ready, or a real failure.
#
# `pane split` is the one call with a real answer to model. The recipe reads the
# new pane's id back out of it, and the SECOND split targets the pane the FIRST
# one returned, so the stub has to hand out a fresh id each time rather than a
# fixed one. It counts calls in a file and answers p2 then p3, which is what lets
# a test tell the two panes apart. STUB_SPLIT_STATUS fails the first split and
# STUB_SPLIT2_STATUS the second, so each can be exercised on its own.
#
# Two logs, because "$*" flattens argv. `pane rename p2 "my tool"` and
# `pane rename p2 my tool` are the same line in $STUB_LOG and different lines in
# $STUB_ARGV_LOG, which is the only way to pin that a label with a space stays
# ONE argument. $STUB_LOG stays because it keeps the ordinary assertions
# readable.
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


# ---------------------------------------------------------------------------
# bin/config-env — the single reader of the .env, for both entry paths
# ---------------------------------------------------------------------------

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


# ---------------------------------------------------------------------------
# bin/on-event — the two gates, and the choice of recipe
# ---------------------------------------------------------------------------

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
        # The real reader, so the settings the hook sees are the real ones.
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
        # The exact command the manifest declares.
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
        Until the exit condition in herdr-plugin.toml is settled, it must not."""
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


# ---------------------------------------------------------------------------
# bin/agent-layout — the recipe itself
# ---------------------------------------------------------------------------

WORKSPACES = {"result": {"workspaces": [
    {"workspace_id": "w9", "active_tab_id": "t1", "label": "proj one",
     "focused": True}]}}

ONE_BARE_PANE = {"result": {"panes": [
    {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj", "agent": None}]}}


class LayoutCase(TempPluginCase):
    """bin/agent-layout run for real against a stub herdr binary."""

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

    def run_layout(self, env=None, expect_status=0):
        e = self.base_env()
        e["HERDR_PLUGIN_CONFIG_DIR"] = self.config_dir
        e["STUB_WORKSPACES"] = self.workspaces
        e["STUB_PANES"] = self.panes
        e.update(env or {})
        # Run the file directly, not via `sh <path>`: the executable bit and the
        # shebang are what bin/on-event's exec and the keybinding both rely on.
        p = subprocess.run([LAYOUT], env=e, capture_output=True, text=True)
        self.assertEqual(p.returncode, expect_status, p.stderr)
        return p.stderr


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
        # Lowercased, non-name characters replaced, prefixed because Herdr
        # requires a leading lowercase letter.
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
        self.assertEqual([c for c in self.herdr_calls()
                          if c.startswith(("tab rename", "pane split",
                                           "pane run", "pane rename",
                                           "agent start"))], [])

    def test_a_pane_already_running_an_agent_is_left_alone(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}})
        err = self.run_layout()
        self.assertIn("pane already runs claude — left alone", err)
        self.assertEqual([c for c in self.herdr_calls()
                          if c.startswith(("tab rename", "pane split",
                                           "pane run", "pane rename",
                                           "agent start"))], [])

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


# ---------------------------------------------------------------------------
# packaging
# ---------------------------------------------------------------------------

class Manifest(unittest.TestCase):
    """Exactly EXPECTED_EVENTS, in order, and nothing else.

    Both halves matter. Losing worktree.created stops the layout entirely.
    Gaining an unplanned subscription is worse than useless: the events fire
    ~1ms apart, Herdr does not serialize hooks, and bin/agent-layout's guard
    reads pane count over the API, so two concurrent ACTING copies would both
    see one pane and both would split it. Gate 1 in bin/on-event absorbs that,
    which is what makes worktree.opened safe to subscribe as log-only
    instrumentation.

    Pinned to the exact list on purpose, never a membership check: the whole
    value of this test is catching a subscription nobody argued for. A
    deliberate change to the list is a one-line edit here, with the manifest's
    reasoning updated alongside it.
    """

    def events(self):
        with open(MANIFEST) as f:
            # Comments discuss the events that were measured and dropped, so
            # match declarations only, not the prose that explains them.
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
                continue  # AGENT_LAYOUT_RECIPE: documented as "unset"
            self.assertIn(default, readme, "%s default %r" % (key, default))

    def test_the_recipe_is_executable(self):
        """bin/on-event execs it and the keybinding runs it directly, so a lost
        executable bit breaks both entry paths. bin/on-event and bin/config-env
        are always spawned through an interpreter and do not need one."""
        self.assertTrue(os.stat(LAYOUT).st_mode & stat.S_IXUSR)


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
