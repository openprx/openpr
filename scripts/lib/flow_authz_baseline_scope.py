#!/usr/bin/env python3
"""Scope the v0.5 authz dependency to the frozen v0.4 member baseline."""

from typing import Any


def evaluate_member_baseline(
    baseline_result: dict[str, Any], verifier_exit: int
) -> tuple[bool, str | None, dict[str, Any], list[Any]]:
    """Return the scoped member-baseline verdict without inheriting unrelated checks."""

    member_baseline = baseline_result.get("member_baseline_no_behaviour_regression")
    if not isinstance(member_baseline, dict):
        return False, "v0_4_member_baseline_fixture_not_covered", {}, []

    violations = member_baseline.get("violations")
    violations_valid = isinstance(violations, list)
    normalized_violations = violations if violations_valid else []
    status = member_baseline.get("status")

    if status == "passed" and violations_valid and not normalized_violations:
        return True, None, member_baseline, normalized_violations
    if status == "failed" or (violations_valid and bool(normalized_violations)):
        return (
            False,
            "v0_4_member_baseline_fixture_failed_observed_regressions",
            member_baseline,
            normalized_violations,
        )

    # Exit 0 cannot upgrade a missing or malformed scoped verdict, and exit 1
    # cannot downgrade a complete passing child verdict merely because an
    # unrelated check in the dependency also ran and failed.
    del verifier_exit
    return False, "v0_4_member_baseline_fixture_not_covered", member_baseline, normalized_violations
