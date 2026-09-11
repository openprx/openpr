#!/usr/bin/env python3
"""Regression checks for scoped v0.4 member-baseline consumption."""

import pathlib
import sys


sys.path.insert(0, str(pathlib.Path(__file__).parent / "lib"))

from flow_authz_baseline_scope import evaluate_member_baseline  # noqa: E402


def evaluate(child: object, verifier_exit: int = 1):
    return evaluate_member_baseline(
        {"passed": False, "member_baseline_no_behaviour_regression": child}, verifier_exit
    )


passed, reason, _child, violations = evaluate({"status": "passed", "violations": []})
assert passed and reason is None and violations == []

passed, reason, _child, violations = evaluate(
    {"status": "failed", "violations": ["MCP title projection differs"]}
)
assert not passed
assert reason == "v0_4_member_baseline_fixture_failed_observed_regressions"
assert violations == ["MCP title projection differs"]

passed, reason, _child, _violations = evaluate({"status": "passed"}, verifier_exit=0)
assert not passed and reason == "v0_4_member_baseline_fixture_not_covered"

passed, reason, _child, _violations = evaluate(None, verifier_exit=0)
assert not passed and reason == "v0_4_member_baseline_fixture_not_covered"

print("PASS: v0.5 consumes the member-baseline child verdict without inheriting unrelated failures")
