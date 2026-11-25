"""The ci-gate passes only for a real green run or a documentation-only skip."""
import json
import os
from pathlib import Path
import subprocess
import unittest

import ci_gate


WORKFLOW = Path(__file__).resolve().parents[1] / ".github" / "workflows" / "ci.yml"

# Check names a ruleset must not require. They are skipped on docs-only diffs.
HEAVY_CHECK_NAMES = (
    "fmt + clippy",
    "version consistency",
    "batch solvability",
    "source art contracts",
    "typos + zizmor + machete",
    "cargo-deny",
    "sbom",
    "determinism across platforms",
    "android (arm64)",
    "windows (arm64, native smoke)",
)


def _needs(relevant="true", apple="true", result="success"):
    needs = {}
    for job_id, _name in ci_gate.JOBS:
        needs[job_id] = {"result": result, "outputs": {}}
    needs["changes"] = {"result": "success", "outputs": {"relevant": relevant}}
    needs["platforms"]["outputs"] = {"apple": apple}
    return needs


def _pull_request(relevant="true", apple="true"):
    """A same-repo pull request: publish jobs are skipped, and so is Apple
    when this diff did not ask for it."""
    needs = _needs(relevant, apple)
    needs["dev-release"]["result"] = "skipped"
    needs["release"]["result"] = "skipped"
    if apple != "true":
        needs["sim-ios"]["result"] = "skipped"
        needs["ios"]["result"] = "skipped"
    return needs


def _docs_only():
    needs = _needs(relevant="false", result="skipped")
    needs["changes"]["result"] = "success"
    return needs


class GateTests(unittest.TestCase):
    def test_relevant_pull_request_with_apple_passes(self):
        self.assertEqual(ci_gate.evaluate(_pull_request(apple="true"), "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline"), [])

    def test_relevant_pull_request_without_apple_passes(self):
        self.assertEqual(ci_gate.evaluate(_pull_request(apple="false"), "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline"), [])

    def test_docs_only_skip_passes(self):
        self.assertEqual(ci_gate.evaluate(_docs_only(), "pull_request", "refs/pull/174/merge"), [])

    def test_docs_only_still_passes_when_a_heavy_job_also_succeeded(self):
        needs = _docs_only()
        needs["lint"]["result"] = "success"
        self.assertEqual(ci_gate.evaluate(needs, "pull_request", "refs/pull/174/merge"), [])

    def test_docs_only_failure_fails(self):
        needs = _docs_only()
        needs["lint"]["result"] = "failure"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/174/merge")
        self.assertTrue(any("lint" in problem and "failure" in problem for problem in problems))

    def test_relevant_lint_failure_fails(self):
        needs = _pull_request()
        needs["lint"]["result"] = "failure"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline")
        self.assertTrue(any("fmt + clippy" in problem for problem in problems))

    def test_relevant_skip_of_a_required_job_fails(self):
        needs = _pull_request()
        needs["determinism"]["result"] = "skipped"
        needs["android"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline")
        self.assertTrue(any("determinism across platforms" in problem for problem in problems))
        self.assertTrue(any("android (arm64)" in problem for problem in problems))

    def test_cancelled_job_fails_even_on_docs(self):
        needs = _docs_only()
        needs["sbom"]["result"] = "cancelled"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/174/merge")
        self.assertTrue(any("cancelled" in problem for problem in problems))

    def test_apple_selected_but_sim_ios_skipped_fails(self):
        needs = _pull_request(apple="true")
        needs["sim-ios"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline")
        self.assertTrue(any("sim-ios" in problem for problem in problems))

    def test_ios_skip_on_a_tag_is_expected(self):
        needs = _needs(apple="true")
        needs["ios"]["result"] = "skipped"
        needs["dev-release"]["result"] = "skipped"
        self.assertEqual(ci_gate.evaluate(needs, "push", "refs/tags/v1.2.3"), [])

    def test_release_must_run_on_a_tag(self):
        needs = _needs(apple="true")
        needs["ios"]["result"] = "skipped"
        needs["dev-release"]["result"] = "skipped"
        needs["release"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "push", "refs/tags/v1.2.3")
        self.assertTrue(any(problem.startswith("release ") for problem in problems))

    def test_dev_release_must_run_on_main(self):
        needs = _needs(apple="true")
        needs["release"]["result"] = "skipped"
        needs["dev-release"]["result"] = "failure"
        problems = ci_gate.evaluate(needs, "push", "refs/heads/main")
        self.assertTrue(any("dev-release" in problem and "failure" in problem for problem in problems))

    def test_publish_skip_after_an_upstream_failure_is_not_a_second_fault(self):
        needs = _needs(apple="true")
        needs["determinism"]["result"] = "failure"
        needs["dev-release"]["result"] = "skipped"
        needs["release"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "push", "refs/heads/main")
        self.assertTrue(any("determinism" in problem for problem in problems))
        self.assertFalse(any("dev-release" in problem for problem in problems))

    def test_dev_release_skip_on_a_pull_request_passes(self):
        self.assertEqual(
            ci_gate.evaluate(_pull_request(), "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline"),
            [],
        )

    def test_fork_may_skip_the_coverage_upload(self):
        needs = _pull_request()
        needs["upload-coverage-rust"]["result"] = "skipped"
        self.assertEqual(
            ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "someone/saltline", "cascadiacollections/saltline"),
            [],
        )

    def test_same_repo_coverage_upload_must_run(self):
        needs = _pull_request()
        needs["upload-coverage-rust"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline")
        self.assertTrue(any("upload-coverage-rust" in problem for problem in problems))

    def test_coverage_failure_does_not_also_blame_the_upload_skip(self):
        needs = _pull_request()
        needs["coverage"]["result"] = "failure"
        needs["upload-coverage-rust"]["result"] = "skipped"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge", "cascadiacollections/saltline", "cascadiacollections/saltline")
        self.assertTrue(any("coverage" in problem and "failure" in problem for problem in problems))
        self.assertFalse(any("upload-coverage-rust" in problem for problem in problems))

    def test_changes_failure_fails_closed(self):
        needs = _docs_only()
        needs["changes"]["result"] = "failure"
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge")
        self.assertTrue(any("changes" in problem for problem in problems))

    def test_missing_relevant_output_fails_closed(self):
        needs = _pull_request()
        needs["changes"]["outputs"] = {}
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge")
        self.assertTrue(any("relevant" in problem for problem in problems))

    def test_unknown_job_fails_closed(self):
        needs = _pull_request()
        needs["surprise"] = {"result": "success", "outputs": {}}
        problems = ci_gate.evaluate(needs, "pull_request", "refs/pull/1/merge")
        self.assertTrue(any("surprise" in problem for problem in problems))

    def test_cli_passes_a_docs_only_result(self):
        env = dict(os.environ)
        env.pop("GITHUB_STEP_SUMMARY", None)
        env["NEEDS_JSON"] = json.dumps(_docs_only())
        env["EVENT_NAME"] = "pull_request"
        env["REF"] = "refs/pull/174/merge"
        env["HEAD_REPO"] = "cascadiacollections/saltline"
        env["REPOSITORY"] = "cascadiacollections/saltline"
        completed = subprocess.run(
            ["python3", str(Path(ci_gate.__file__))],
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("ci-gate passed", completed.stdout)
        self.assertIn("relevant=False", completed.stdout)

    def test_cli_fails_a_relevant_failure(self):
        needs = _pull_request()
        needs["windows-arm64"]["result"] = "failure"
        env = dict(os.environ)
        env.pop("GITHUB_STEP_SUMMARY", None)
        env["NEEDS_JSON"] = json.dumps(needs)
        env["EVENT_NAME"] = "pull_request"
        env["REF"] = "refs/pull/1/merge"
        env["HEAD_REPO"] = "cascadiacollections/saltline"
        env["REPOSITORY"] = "cascadiacollections/saltline"
        completed = subprocess.run(
            ["python3", str(Path(ci_gate.__file__))],
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 1)
        self.assertIn("windows (arm64, native smoke)", completed.stderr)


class WorkflowContractTests(unittest.TestCase):
    def test_trigger_has_no_paths_filter_and_names_stay(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        header, jobs = text.split("\njobs:", 1)
        self.assertNotIn("\n    paths:", header)
        self.assertIn("pull_request:", header)
        self.assertIn("branches: [main]", header)
        self.assertIn("tags: ['v*']", header)
        for name in HEAVY_CHECK_NAMES:
            self.assertEqual(text.count(f"name: {name}\n"), 1, name)
        self.assertEqual(text.count("name: ci-gate\n"), 1)
        self.assertEqual(text.count("name: detect relevant changes\n"), 1)
        self.assertIn("GODOT_VERSION: 4.7.2\n", text)
        self.assertIn(
            "dorny/paths-filter@ceb8a2b8f2d89434be7ff52d3de7ec3738c5cc9d # v4.0.3",
            text,
        )
        # The heavy jobs are gated on the changes output, not removed.
        self.assertIn("needs.changes.outputs.relevant == 'true'", jobs)
        self.assertIn("NOTES.md", jobs)
        self.assertIn("addons/xdg_portals/THIRD_PARTY_NOTICES.md", jobs)
        self.assertIn("!site/assets/readme/**", jobs)


if __name__ == "__main__":
    unittest.main()
