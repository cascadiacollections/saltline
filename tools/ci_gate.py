"""Decide whether the always-on `ci-gate` check passed.

A ruleset on main should require the check named `ci-gate` and not the heavy
jobs. Those jobs are skipped when the diff changes nothing they cover, and a
required check that was skipped never goes green. This script is the whole of
that decision: every job `ci-gate` waits on succeeded, or was skipped because
the change was not relevant (or because that job is defined to skip). A
failure, a cancellation, or a skip of a job that should have run fails the gate.
"""

import json
import os
import sys

# Job id, then the check name GitHub shows. The check names of the existing
# jobs are load-bearing: a ruleset that still requires one of them hangs on a
# documentation-only pull request. `ci-gate` is the name to require instead.
JOBS = (
    ("changes", "detect relevant changes"),
    ("platforms", "select platform coverage"),
    ("sim", "sim"),
    ("sim-android", "sim (android x86_64 emulator)"),
    ("sim-ios", "sim (ios arm64 simulator)"),
    ("determinism", "determinism across platforms"),
    ("version", "version consistency"),
    ("solvability", "batch solvability"),
    ("lint", "fmt + clippy"),
    ("art", "source art contracts"),
    ("gpu-check", "gpu check (llvmpipe)"),
    ("hygiene", "typos + zizmor + machete"),
    ("coverage", "coverage"),
    ("upload-coverage-rust", "upload coverage (rust)"),
    ("miri", "miri"),
    ("fuzz-build", "fuzz targets build"),
    ("deny", "cargo-deny"),
    ("playable", "playable"),
    ("android", "android (arm64)"),
    ("ios", "ios (arm64, unsigned)"),
    ("windows-arm64", "windows (arm64, native smoke)"),
    ("sbom", "sbom"),
    ("dev-release", "dev release"),
    ("release", "release"),
)

# These run on every relevant change. A skip here means the job did not run
# when it was needed.
MUST_SUCCEED = (
    "platforms",
    "sim",
    "sim-android",
    "determinism",
    "version",
    "solvability",
    "lint",
    "art",
    "gpu-check",
    "hygiene",
    "coverage",
    "miri",
    "fuzz-build",
    "deny",
    "playable",
    "android",
    "windows-arm64",
    "sbom",
)

JOB_IDS = tuple(job_id for job_id, _name in JOBS)
DISPLAY = dict(JOBS)


def evaluate(needs, event_name, ref, head_repo="", repository=""):
    """Return the reasons the gate failed. An empty list means it passed."""
    problems = []
    if not isinstance(needs, dict):
        return ["needs is not an object"]

    present = set(needs)
    expected = set(JOB_IDS)
    missing = sorted(expected - present)
    extra = sorted(present - expected)
    if missing:
        problems.append("missing results for " + ", ".join(missing))
    if extra:
        problems.append("unexpected jobs " + ", ".join(extra))
    if problems:
        return problems

    changes = _info(needs, "changes")
    if changes["result"] != "success":
        problems.append(_line("changes", changes["result"]))
        return problems
    relevant_text = changes["outputs"].get("relevant")
    if relevant_text not in ("true", "false"):
        problems.append("changes did not set relevant to true or false")
        return problems
    relevant = relevant_text == "true"

    if not relevant:
        for job_id in JOB_IDS:
            result = _info(needs, job_id)["result"]
            if result not in ("success", "skipped"):
                problems.append(_line(job_id, result) + " on a change the heavy jobs skip")
        return problems

    for job_id in MUST_SUCCEED:
        result = _info(needs, job_id)["result"]
        if result != "success":
            problems.append(_line(job_id, result) + "; a relevant change has to run it")

    problems.extend(_sim_ios(needs))
    problems.extend(_ios(needs, ref))
    problems.extend(_upload(needs, event_name, head_repo, repository))
    problems.extend(_publish("dev-release", needs, relevant and event_name == "push" and ref == "refs/heads/main"))
    problems.extend(_publish("release", needs, relevant and event_name == "push" and ref.startswith("refs/tags/v")))
    return problems


def _sim_ios(needs):
    result = _info(needs, "sim-ios")["result"]
    if result == "success":
        return []
    if result != "skipped":
        return [_line("sim-ios", result)]
    # Skipped is the no-Apple pull request. A selected Apple run has to execute.
    if _info(needs, "platforms")["result"] == "success" and _apple(needs):
        return [_line("sim-ios", "skipped") + "; Apple coverage was selected"]
    return []


def _ios(needs, ref):
    result = _info(needs, "ios")["result"]
    if result == "success":
        return []
    if result != "skipped":
        return [_line("ios", result)]
    # The ios job does not run on tags (no signing) or when Apple is out of scope.
    if ref.startswith("refs/tags/v"):
        return []
    if _info(needs, "platforms")["result"] == "success" and _apple(needs):
        return [_line("ios", "skipped") + "; Apple coverage was selected"]
    return []


def _upload(needs, event_name, head_repo, repository):
    result = _info(needs, "upload-coverage-rust")["result"]
    if result == "success":
        return []
    if result != "skipped":
        return [_line("upload-coverage-rust", result)]
    # Fork pull requests and a failed coverage job skip the upload on purpose.
    if _info(needs, "coverage")["result"] != "success":
        return []
    fork = event_name == "pull_request" and head_repo not in ("", repository)
    if fork:
        return []
    return [_line("upload-coverage-rust", "skipped") + " after coverage passed"]


def _publish(job_id, needs, required):
    result = _info(needs, job_id)["result"]
    if result == "success":
        return []
    if result != "skipped":
        return [_line(job_id, result)]
    if not required:
        return []
    # A failed or skipped upstream already fails the gate, and the publish job
    # is defined to wait for those. Only a skip with every upstream green means
    # the publish itself did not run.
    upstream = {
        "dev-release": ("version", "determinism", "solvability", "lint", "art", "playable", "android", "windows-arm64", "sbom"),
        "release": ("version", "determinism", "solvability", "lint", "art", "deny", "playable", "android", "windows-arm64", "sbom"),
    }[job_id]
    if any(_info(needs, name)["result"] != "success" for name in upstream):
        return []
    return [_line(job_id, "skipped") + " on a ref that publishes"]


def _apple(needs):
    return _info(needs, "platforms")["outputs"].get("apple") == "true"


def _info(needs, job_id):
    raw = needs.get(job_id)
    if not isinstance(raw, dict):
        return {"result": "missing", "outputs": {}}
    result = raw.get("result")
    outputs = raw.get("outputs")
    if not isinstance(result, str) or not result:
        result = "missing"
    if not isinstance(outputs, dict):
        outputs = {}
    return {"result": result, "outputs": outputs}


def _line(job_id, result):
    return f"{job_id} ({DISPLAY[job_id]}) {result}"


def _summary(relevant, problems, lines):
    title = "ci-gate failed" if problems else "ci-gate passed"
    body = [f"## {title}", ""]
    if relevant is None:
        body.append("Change detection did not finish.")
    elif relevant:
        body.append("Relevant files changed, so the heavy jobs had to succeed.")
    else:
        body.append("Nothing the heavy jobs cover changed. Skipped heavy jobs are a pass.")
    body.append("")
    if problems:
        body.append("### Why it failed")
        body.extend(f"- {problem}" for problem in problems)
        body.append("")
    body.append("### Results")
    body.extend(f"- {line}" for line in lines)
    body.append("")
    return "\n".join(body)


def main():
    try:
        needs = json.loads(os.environ["NEEDS_JSON"])
    except (KeyError, json.JSONDecodeError) as error:
        print(f"ci-gate could not read job results: {error}", file=sys.stderr)
        return 1
    event_name = os.environ.get("EVENT_NAME", "")
    ref = os.environ.get("REF", "")
    head_repo = os.environ.get("HEAD_REPO", "")
    repository = os.environ.get("REPOSITORY", "")
    problems = evaluate(needs, event_name, ref, head_repo, repository)

    relevant = None
    if isinstance(needs, dict):
        changes = _info(needs, "changes")
        if changes["outputs"].get("relevant") == "true":
            relevant = True
        elif changes["outputs"].get("relevant") == "false":
            relevant = False
    lines = []
    if isinstance(needs, dict):
        for job_id, _name in JOBS:
            if job_id in needs:
                lines.append(f"{_line(job_id, _info(needs, job_id)['result'])}")
    print(f"relevant={relevant}")
    print("\n".join(lines))
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as summary:
            summary.write(_summary(relevant, problems, lines))
    if problems:
        print("ci-gate failed:", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1
    print("ci-gate passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
