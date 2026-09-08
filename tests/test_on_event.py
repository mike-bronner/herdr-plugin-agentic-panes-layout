"""Unit tests for the whole plugin: the two gates, the settings, the layout.

Run: python3 -m unittest discover tests

Nothing is imported. Every script is run for real, as a subprocess, against
stubs planted in a temporary directory. That is the only honest way to test
these: bin/on-event's entire job is which process it does or does not exec, and
bin/agent-layout's entire job is which herdr commands it does or does not run.
Stubbing the exec target and the herdr binary is what makes both observable
without splitting a real pane.
"""
import json, os, stat, subprocess, tempfile, unittest

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
    "AGENT_LAYOUT_RECIPE": "",
}

# Stand-in for the herdr CLI. Records every invocation to $STUB_LOG, answers the
# two list calls from fixture files, and takes its `agent start` outcome from the
# environment so a test can pick success, agent_not_ready, or a real failure.
STUB_HERDR = """#!/bin/sh
printf '%s\\n' "$*" >> "$STUB_LOG"
case "$1 $2" in
    "workspace list")    cat "$STUB_WORKSPACES" ;;
    "pane list")         cat "$STUB_PANES" ;;
    "plugin config-dir") printf '%s\\n' "${STUB_CONFIG_DIR:-}" ;;
    "tab rename")        exit "${STUB_RENAME_STATUS:-0}" ;;
    "pane split")        exit "${STUB_SPLIT_STATUS:-0}" ;;
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
                "HERDR_BIN_PATH": self.herdr, "STUB_LOG": self.log}

    def herdr_calls(self):
        try:
            with open(self.log) as f:
                return [line.rstrip("\n") for line in f]
        except OSError:
            return []


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
        self.write_env("AGENT_LAYOUT_KIND=codex\n"
                       "AGENT_LAYOUT_DIRECTION=down\n"
                       "AGENT_LAYOUT_RATIO=0.3\n"
                       "AGENT_LAYOUT_TAB_NAME=work\n"
                       "AGENT_LAYOUT_RECIPE=/tmp/mine\n")
        self.assertEqual(self.settings(), {
            "AGENT_LAYOUT_KIND": "codex",
            "AGENT_LAYOUT_DIRECTION": "down",
            "AGENT_LAYOUT_RATIO": "0.3",
            "AGENT_LAYOUT_TAB_NAME": "work",
            "AGENT_LAYOUT_RECIPE": "/tmp/mine",
        })

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
    """The three commands that make up the layout, and the settings they read."""

    def test_defaults_reach_the_herdr_command_line(self):
        self.run_layout()
        calls = self.herdr_calls()
        self.assertIn("tab rename t1 agent", calls)
        self.assertIn("pane split --pane p1 --direction right --ratio 0.5"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("agent start proj-one --kind claude --pane p1", calls)

    def test_settings_reach_the_herdr_command_line(self):
        self.write_env("AGENT_LAYOUT_KIND=codex\n"
                       "AGENT_LAYOUT_DIRECTION=down\n"
                       "AGENT_LAYOUT_RATIO=0.35\n"
                       "AGENT_LAYOUT_TAB_NAME=work\n")
        self.run_layout()
        calls = self.herdr_calls()
        self.assertIn("tab rename t1 work", calls)
        self.assertIn("pane split --pane p1 --direction down --ratio 0.35"
                      " --cwd /tmp/proj --no-focus", calls)
        self.assertIn("agent start proj-one --kind codex --pane p1", calls)

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

    def test_no_focused_workspace_is_fatal(self):
        self.set_workspaces({"result": {"workspaces": [
            {"workspace_id": "w9", "active_tab_id": "t1", "label": "x",
             "focused": False}]}})
        err = self.run_layout(expect_status=1)
        self.assertIn("cannot read a focused workspace", err)


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
                                           "agent start"))], [])

    def test_a_pane_already_running_an_agent_is_left_alone(self):
        self.set_panes({"result": {"panes": [
            {"pane_id": "p1", "tab_id": "t1", "cwd": "/tmp/proj",
             "agent": "claude"}]}})
        err = self.run_layout()
        self.assertIn("pane already runs claude — left alone", err)
        self.assertEqual([c for c in self.herdr_calls()
                          if c.startswith(("tab rename", "pane split",
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
        self.assertIn("shell split right", err)
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

    def test_the_recipe_is_executable(self):
        """bin/on-event execs it and the keybinding runs it directly, so a lost
        executable bit breaks both entry paths. bin/on-event and bin/config-env
        are always spawned through an interpreter and do not need one."""
        self.assertTrue(os.stat(LAYOUT).st_mode & stat.S_IXUSR)


if __name__ == "__main__":
    unittest.main()
