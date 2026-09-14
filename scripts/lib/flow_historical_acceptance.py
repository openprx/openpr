#!/usr/bin/env python3
"""Fail-closed policy for whether a historical Flow release was accepted."""

from __future__ import annotations

import sys


ACCEPTED_HISTORICAL_STATUSES = frozenset({"accepted", "accepted_with_known_gap"})


def is_accepted_historical_status(status: object) -> bool:
    return isinstance(status, str) and status in ACCEPTED_HISTORICAL_STATUSES


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: flow_historical_acceptance.py STATUS")
    raise SystemExit(0 if is_accepted_historical_status(sys.argv[1]) else 1)
