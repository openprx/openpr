#!/usr/bin/env python3
"""Shared fail-closed acceptance policy for Flow gate contract statuses."""

from __future__ import annotations

import sys


ACCEPTED_CONTRACT_STATUSES = frozenset({"active", "accepted_with_known_gap"})


def is_accepted_contract_status(status: object) -> bool:
    return isinstance(status, str) and status in ACCEPTED_CONTRACT_STATUSES


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: flow_contract_status.py STATUS")
    raise SystemExit(0 if is_accepted_contract_status(sys.argv[1]) else 1)
