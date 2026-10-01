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

    def run_checks(self, *args, failures=()):
        env = dict(os.environ)
        env.update(
            PATH=str(self.bin) + os.pathsep + env["PATH"],
            CIDE_RUNNER_CALLS=str(self.calls_file),
            CIDE_RUNNER_FAILURES=json.dumps(failures),
            GITHUB_ACTIONS="false",
        )
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

    def test_invalid_group_never_starts_a_check(self):
        result, calls = self.run_checks("--only", "typo")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
