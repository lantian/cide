#!/usr/bin/env python3
"""Check failure aggregation and CI scopes without compiling or contacting a remote."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
MOCK = r'''
import json
import os
from pathlib import Path
import sys

tool = Path(sys.argv[0]).name
args = sys.argv[1:]
call = [tool, *args]
with open(os.environ['CIDE_RUNNER_CALLS'], 'a') as out:
    out.write(json.dumps(call) + '\n')
if call in json.loads(os.environ.get('CIDE_RUNNER_FAILURES', '[]')):
    print('fixture command failed:', ' '.join(call), file=sys.stderr)
    sys.exit(23)
if call in json.loads(os.environ.get('CIDE_RUNNER_KILLS', '[]')):
    # Kill the shell that launched this command, as an OOM kill or a stray signal would:
    # the runner never learns an exit code for it.
    import signal
    os.kill(os.getppid(), signal.SIGKILL)
    sys.exit(1)
if tool == 'node':
    scripts = json.loads(Path('ui/package.json').read_text())['scripts']
    print('\n'.join(s for s in scripts if s.startswith('check:')))
elif tool == 'cargo' and args[0] == 'tree':
    print('tauri v2.0.0' if args[args.index('-p') + 1] == 'cide-app' else 'cide-core v1.0.0')
elif tool == 'cargo' and args[0] == 'metadata':
    root = Path.cwd()
    print(json.dumps({'workspace_root': str(root), 'packages': [
        {'name': name, 'manifest_path': str(root / 'crates' / name / 'Cargo.toml')}
        for name in ['cide-app', 'cide-core']
    ]}))
elif tool == 'git' and args[0] == 'ls-files':
    print('\n'.join(str(p) for p in Path('.').rglob('*.sh')))
elif tool == 'git' and args[0] == 'ls-remote':
    print('0123456789abcdef\trefs/tags/' + args[-1])
'''


class RunnerChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cide-check-runner-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout with spaces"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy(ROOT / "test.sh", self.root / "test.sh")
        for name in ["check-bash32.sh", "check-no-tauri.sh", "check-fork-pins.sh"]:
            shutil.copy(ROOT / "scripts" / name, self.root / "scripts" / name)
        # The runner's own check must not recursively invoke this fixture.
        (self.root / "scripts/check-test-runner.py").write_text("print('fixture self-check')\n")
        (self.root / "ui").mkdir()
        (self.root / "ui/package.json").write_text(json.dumps({"scripts": {
            "build": "fixture", "check:first": "fixture", "check:added-later": "fixture",
        }}))
        (self.root / "packaging").mkdir()
        for name, key in [("rust-analyzer", "RA"), ("salsa", "SALSA"), ("gopls", "GOPLS")]:
            (self.root / "packaging" / f"{name}.lock").write_text(
                f"CIDE_{key}_URL=https://example.invalid/{name}\nCIDE_{key}_REV=fixture\n"
            )
        self.calls_file = Path(self.temp.name) / "calls.jsonl"
        self.bin = Path(self.temp.name) / "bin"
        self.bin.mkdir()
        for tool in ["cargo", "pnpm", "node", "git"]:
            mock = self.bin / tool
            mock.write_text(f"#!{sys.executable}\n" + MOCK)
            mock.chmod(0o755)

    def run_checks(self, *args, failures=(), kills=(), jobs=None, serial=None):
        env = dict(os.environ)
        env.update(
            PATH=str(self.bin) + os.pathsep + env["PATH"],
            CIDE_RUNNER_CALLS=str(self.calls_file),
            CIDE_RUNNER_FAILURES=json.dumps(failures),
            CIDE_RUNNER_KILLS=json.dumps(kills),
            GITHUB_ACTIONS="false",
        )
        if jobs is not None:
            env["CIDE_CHECK_JOBS"] = str(jobs)
        if serial is not None:
            env["CIDE_SERIAL_CHECKS"] = serial
        result = subprocess.run(
            ["bash", str(self.root / "test.sh"), *args],
            cwd=self.temp.name, env=env, text=True, capture_output=True, timeout=30,
        )
        calls = [json.loads(line) for line in self.calls_file.read_text().splitlines()] \
            if self.calls_file.exists() else []
        return result, calls

    def test_all_can_pass_from_another_directory(self):
        result, calls = self.run_checks()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 failed", result.stdout)
        self.assertIn(["cargo", "test", "--locked", "--workspace", "--no-fail-fast"], calls)
        self.assertIn(["pnpm", "--dir", "ui", "install", "--frozen-lockfile"], calls)
        self.assertIn(["pnpm", "--dir", "ui", "run", "check:added-later"], calls)

    def test_failures_do_not_conceal_other_checks(self):
        failed_commands = [
            ["cargo", "fmt", "--all", "--check"],
            ["cargo", "test", "--locked", "--workspace", "--no-fail-fast"],
            ["pnpm", "--dir", "ui", "run", "check:first"],
        ]
        result, calls = self.run_checks(failures=failed_commands)
        self.assertEqual(result.returncode, 1)
        self.assertIn("3 failed", result.stdout)
        for label in ["rust/fmt", "rust/test", "ui/check:first"]:
            self.assertIn(label, result.stderr)
        self.assertIn(["cargo", "--locked", "xtask", "codegen", "--check"], calls)
        self.assertIn(["pnpm", "--dir", "ui", "run", "check:added-later"], calls)
        self.assertIn(["pnpm", "--dir", "ui", "build"], calls)
        self.assertEqual(sum(call[:2] == ["git", "ls-remote"] for call in calls), 3)

    def test_failed_install_cannot_validate_stale_frontend_dependencies(self):
        install = ["pnpm", "--dir", "ui", "install", "--frozen-lockfile"]
        result, calls = self.run_checks(failures=[install])
        self.assertEqual(result.returncode, 1)
        self.assertEqual([call for call in calls if call[0] == "pnpm"], [install])
        self.assertEqual(sum(call[:2] == ["git", "ls-remote"] for call in calls), 3)

    def test_listing_runs_no_checks_or_installation(self):
        result, calls = self.run_checks("--list")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(all(call[0] == "node" for call in calls))
        self.assertIn("ui/check:added-later:", result.stdout)
        self.assertFalse((self.root / "target").exists())

    def test_structural_cargo_errors_cannot_be_reported_as_clean(self):
        tree = ["cargo", "tree", "--locked", "-p", "cide-core", "--edges",
                "normal,build,dev", "--prefix", "none", "--no-dedupe"]
        result, _ = self.run_checks("--only", "no-tauri", failures=[tree])
        self.assertEqual(result.returncode, 1)
        self.assertIn("structure/no-tauri", result.stderr)

    def test_failed_remote_check_still_checks_the_other_pins(self):
        remote = ["git", "ls-remote", "https://example.invalid/rust-analyzer", "fixture"]
        result, calls = self.run_checks("--only", "fork-pins", failures=[remote])
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(calls), 3)

    def results(self):
        runs = sorted((self.root / "target" / "ci-checks").glob("run-*"))
        self.assertEqual(len(runs), 1)
        return [line.split("\t") for line in (runs[0] / "results.tsv").read_text().splitlines()]

    def test_parallel_ui_checks_report_in_order_with_real_exit_codes(self):
        # The check:* scripts run concurrently; each must still be reported once, numbered and
        # ordered as package.json lists them, with its own exit code.
        failing = ["pnpm", "--dir", "ui", "run", "check:added-later"]
        result, _ = self.run_checks("--only", "ui", failures=[failing], jobs=2)
        self.assertEqual(result.returncode, 1)
        rows = self.results()
        self.assertEqual([r[0] for r in rows], [
            "ui/install", "ui/typecheck", "ui/check:first", "ui/check:added-later", "ui/build",
        ])
        self.assertEqual([r[1] for r in rows], ["0", "0", "0", "23", "0"])
        self.assertEqual([Path(r[2]).name for r in rows],
                         ["001.log", "002.log", "003.log", "004.log", "005.log"])
        self.assertIn("ui/check:added-later (exit 23)", result.stderr)
        self.assertIn("1 failed", result.stdout)

    def test_a_serial_ui_check_keeps_its_place_in_the_report(self):
        # check:first is made serial, so it runs after check:added-later — and must still be
        # numbered and reported before it.
        result, calls = self.run_checks("--only", "ui", jobs=2, serial="check:first")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        runs = [c for c in calls if c[:4] == ["pnpm", "--dir", "ui", "run"]]
        self.assertEqual([c[4] for c in runs], ["check:added-later", "check:first"])
        rows = self.results()
        self.assertEqual([r[0] for r in rows][2:4], ["ui/check:first", "ui/check:added-later"])
        self.assertEqual([Path(r[2]).name for r in rows][2:4], ["003.log", "004.log"])

    def test_a_ui_check_that_never_reports_is_a_failure_not_a_gap(self):
        # Killing the first check's shell stops xargs launching the second as well. Neither
        # may vanish from the summary or be counted as a pass.
        killed = ["pnpm", "--dir", "ui", "run", "check:first"]
        result, _ = self.run_checks("--only", "ui", kills=[killed], jobs=1)
        self.assertEqual(result.returncode, 1)
        rows = {r[0]: r for r in self.results()}
        self.assertEqual(rows["ui/check:first"][1], "125")
        self.assertEqual(rows["ui/check:added-later"][1], "125")
        self.assertIn("did not finish", Path(rows["ui/check:first"][2]).read_text())
        self.assertIn("2 failed", result.stdout)

    def test_the_two_rust_jobs_add_up_to_the_rust_group(self):
        # CI runs rust-lint and rust-test as separate jobs. A step in neither would be a check
        # CI silently stopped running while every job stayed green; a step in both is a job
        # paying for the other's work.
        def cargo_calls(group):
            self.calls_file.unlink(missing_ok=True)
            result, calls = self.run_checks("--only", group)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return [tuple(call) for call in calls if call[0] == "cargo"]

        whole = cargo_calls("rust")
        lint = cargo_calls("rust-lint")
        tests = cargo_calls("rust-test")
        self.assertFalse(set(lint) & set(tests))
        self.assertEqual(sorted(lint + tests), sorted(whole))
        self.assertEqual(tests, [("cargo", "test", "--locked", "--workspace", "--no-fail-fast")])

    def test_invalid_group_never_starts_a_check(self):
        result, calls = self.run_checks("--only", "typo")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
