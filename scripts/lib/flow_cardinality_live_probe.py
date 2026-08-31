#!/usr/bin/env python3
"""Exercise the v0.4 cardinality/idempotency races against a live API and PostgreSQL."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import subprocess
import urllib.error
import urllib.request
import uuid


def request(api: str, token: str, method: str, path: str, body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body, separators=(",", ":")).encode()
    req = urllib.request.Request(
        api + path,
        data=data,
        method=method,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", "replace")
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return {"code": error.code, "message": raw[:500]}


def sql(database_url: str, statement: str, *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["psql", database_url, "-v", "ON_ERROR_STOP=1", "-At", "-c", statement],
        text=True,
        capture_output=True,
        check=check,
    )


def scalar(database_url: str, statement: str) -> str:
    return sql(database_url, statement).stdout.strip()


def object_id(response: dict) -> str:
    data = response.get("data") or {}
    return str(data.get("id") or (data.get("object") or {}).get("id") or "")


def event_id(response: dict) -> str:
    return str((response.get("data") or {}).get("event_id") or "")


def create_body(title: str, key: str) -> dict:
    return {"object_type": "page", "title": title, "idempotency_key": key}


def command_body(command_type: str, payload: dict, key: str) -> dict:
    return {"command": {"type": command_type, "payload": payload}, "idempotency_key": key}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api", required=True)
    parser.add_argument("--token", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--database-url", required=True)
    args = parser.parse_args()

    api = args.api.rstrip("/")
    workspace = args.workspace
    create_path = f"/api/v1/workspaces/{workspace}/flow/objects"
    violations: list[str] = []
    observations: dict[str, dict] = {}

    # 1. Concurrent identical retries must converge on one complete aggregate.
    same_key = f"card-same-{uuid.uuid4()}"
    same_body = create_body("same-key race", same_key)
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        same_responses = list(pool.map(lambda _: request(api, args.token, "POST", create_path, same_body), range(8)))
    same_ids = sorted({object_id(response) for response in same_responses if object_id(response)})
    same_events = sorted({event_id(response) for response in same_responses if event_id(response)})
    same_db = {
        "events": int(scalar(args.database_url, f"SELECT count(*) FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}'")),
        "objects": int(scalar(args.database_url, f"SELECT count(*) FROM flow_objects WHERE id IN (SELECT aggregate_id::uuid FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}')")),
        "documents": int(scalar(args.database_url, f"SELECT count(*) FROM collab_documents WHERE object_id IN (SELECT aggregate_id::uuid FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}')")),
        "projections": int(scalar(args.database_url, f"SELECT count(*) FROM flow_object_projections WHERE object_id IN (SELECT aggregate_id::uuid FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}')")),
        "dispatch": int(scalar(args.database_url, f"SELECT count(*) FROM event_dispatch WHERE event_id IN (SELECT id FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}')")),
    }
    same_negative = request(api, args.token, "POST", create_path, create_body("different body", same_key))
    canonical_same_id = scalar(args.database_url, f"SELECT aggregate_id FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{same_key}'")
    same_passed = (
        all(response.get("code") == 0 for response in same_responses)
        and same_ids == [canonical_same_id]
        and len(same_events) == 1
        and all(value == 1 for value in same_db.values())
        and same_negative.get("code") == 409
    )
    if not same_passed:
        violations.append("concurrent same-idempotency-key requests did not converge to one aggregate or reject a different-body reuse")
    observations["concurrent_same_idempotency_key"] = {
        "status": "passed" if same_passed else "failed",
        "request_count": 8,
        "response_codes": [response.get("code") for response in same_responses],
        "distinct_object_ids": same_ids,
        "canonical_object_id": canonical_same_id,
        "all_response_object_ids_canonical": same_ids == [canonical_same_id],
        "response_projection_note": "every successful replay response must identify the one canonical object; a rolled-back temporary object ID is a phantom success",
        "distinct_event_ids": same_events,
        "database_counts": same_db,
        "negative_different_body": same_negative,
    }

    # 2. Different keys contending on one canonical document must allocate two rows/seqs.
    lineage_create = request(api, args.token, "POST", create_path, create_body("lineage base", f"card-lineage-create-{uuid.uuid4()}"))
    lineage_object = object_id(lineage_create)
    if not lineage_object:
        violations.append(f"could not create the competing-lineage base object: {lineage_create}")
        observations["different_keys_same_lineage"] = {
            "status": "failed",
            "create_response": lineage_create,
        }
        print(json.dumps({"status": "failed", "fixtures": observations, "violations": violations}, separators=(",", ":")))
        return 1
    document_id = scalar(args.database_url, f"SELECT id FROM collab_documents WHERE object_id='{lineage_object}'") if lineage_object else ""
    lineage_keys = [f"card-lineage-a-{uuid.uuid4()}", f"card-lineage-b-{uuid.uuid4()}"]
    lineage_titles = ["lineage contender A", "lineage contender B"]
    command_path = f"/api/v1/flow/objects/{lineage_object}/commands"
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [
            pool.submit(request, api, args.token, "POST", command_path, command_body("set_title", {"title": title}, key))
            for title, key in zip(lineage_titles, lineage_keys, strict=True)
        ]
        lineage_responses = [future.result() for future in futures]
    lineage_seqs = [int(value) for value in scalar(args.database_url, f"SELECT seq FROM collab_updates WHERE document_id='{document_id}' ORDER BY seq").splitlines() if value]
    lineage_event_count = int(scalar(args.database_url, "SELECT count(*) FROM business_events WHERE workspace_id='{}' AND idempotency_key IN ('{}','{}')".format(workspace, *lineage_keys)))
    lineage_update_count = int(scalar(args.database_url, "SELECT count(*) FROM collab_updates WHERE document_id='{}' AND idempotency_key IN ('{}','{}')".format(document_id, *lineage_keys)))
    lineage_passed = (
        all(response.get("code") == 0 for response in lineage_responses)
        and lineage_seqs == [1, 2]
        and lineage_event_count == 2
        and lineage_update_count == 2
    )
    if not lineage_passed:
        violations.append("different idempotency keys contending on one lineage did not produce two unique, contiguous committed updates")
    observations["different_keys_same_lineage"] = {
        "status": "passed" if lineage_passed else "failed",
        "response_codes": [response.get("code") for response in lineage_responses],
        "document_id": document_id,
        "committed_seqs": lineage_seqs,
        "event_rows": lineage_event_count,
        "update_rows": lineage_update_count,
    }

    # 3. v0.4 has no non-zero legacy importer, so inject the externally-known UUID collision
    # at the real canonical database boundary, then prove through the live API that it neither
    # overwrote the original nor left the transaction's earlier marker behind.
    pre_key = f"card-preallocated-{uuid.uuid4()}"
    pre_create = request(api, args.token, "POST", create_path, create_body("preallocated UUID owner", pre_key))
    proposed_target_id = object_id(pre_create)
    epoch_before = int(scalar(args.database_url, f"SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id='{workspace}'"))
    collision_sql = (
        "BEGIN; "
        f"UPDATE flow_workspace_settings SET authz_epoch=authz_epoch+1000 WHERE workspace_id='{workspace}'; "
        f"INSERT INTO flow_objects(id,workspace_id,object_type,created_by) SELECT '{proposed_target_id}',id,'page',created_by FROM workspaces WHERE id='{workspace}'; "
        "COMMIT;"
    )
    collision = sql(args.database_url, collision_sql, check=False)
    epoch_after = int(scalar(args.database_url, f"SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id='{workspace}'"))
    pre_after = request(api, args.token, "GET", f"/api/v1/flow/objects/{proposed_target_id}")
    pre_title = str(((pre_after.get("data") or {}).get("title") or ""))
    pre_count = int(scalar(args.database_url, f"SELECT count(*) FROM flow_objects WHERE id='{proposed_target_id}'"))
    collision_error = (collision.stderr + "\n" + collision.stdout).strip()
    constraint_error = "duplicate key" in collision_error.lower() or "unique constraint" in collision_error.lower()
    pre_passed = collision.returncode != 0 and constraint_error and epoch_after == epoch_before and pre_count == 1 and pre_title == "preallocated UUID owner"
    if not pre_passed:
        violations.append("preallocated UUID unique-key collision did not roll back the whole transaction without overwriting the canonical object")
    observations["preallocated_uuid_conflict"] = {
        "status": "passed" if pre_passed else "failed",
        "producer_surface": "not_applicable_zero_legacy_inventory",
        "injection_boundary": "real_postgresql_canonical_transaction",
        "proposed_target_id": proposed_target_id,
        "collision_exit_code": collision.returncode,
        "constraint_error_observed": constraint_error,
        "database_error": collision_error[:1000],
        "marker_epoch_before": epoch_before,
        "marker_epoch_after": epoch_after,
        "canonical_row_count": pre_count,
        "live_api_title_after_collision": pre_title,
    }

    # 4. Close the first connection without reading the response, then repeat the exact request.
    lost_key = f"card-lost-{uuid.uuid4()}"
    lost_body = create_body("lost response retry", lost_key)
    # Read the upstream response completely so commit is known to have completed, then discard
    # it at the caller boundary. This is the deterministic equivalent of a reverse proxy losing
    # the response after receiving it; closing the upstream socket immediately after send would
    # instead allow request cancellation before commit and test the wrong failure mode.
    discarded_response = request(api, args.token, "POST", create_path, lost_body)
    first_event = scalar(args.database_url, f"SELECT id FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{lost_key}'")
    retry = request(api, args.token, "POST", create_path, lost_body)
    lost_object = object_id(retry)
    lost_counts = {
        "events": int(scalar(args.database_url, f"SELECT count(*) FROM business_events WHERE workspace_id='{workspace}' AND idempotency_key='{lost_key}'")),
        "objects": int(scalar(args.database_url, f"SELECT count(*) FROM flow_objects WHERE id='{lost_object}'")) if lost_object else 0,
        "documents": int(scalar(args.database_url, f"SELECT count(*) FROM collab_documents WHERE object_id='{lost_object}'")) if lost_object else 0,
        "projections": int(scalar(args.database_url, f"SELECT count(*) FROM flow_object_projections WHERE object_id='{lost_object}'")) if lost_object else 0,
    }
    lost_passed = retry.get("code") == 0 and first_event == event_id(retry) and all(value == 1 for value in lost_counts.values())
    if not lost_passed:
        violations.append("retry after a deliberately lost HTTP response did not return the original complete aggregate")
    observations["lost_response_retry"] = {
        "status": "passed" if lost_passed else "failed",
        "upstream_response_received_then_discarded": discarded_response.get("code") == 0,
        "committed_event_before_retry": first_event,
        "retry_event_id": event_id(retry),
        "retry_object_id": lost_object,
        "database_counts": lost_counts,
    }

    passed = not violations and all(item.get("status") == "passed" for item in observations.values())
    print(json.dumps({"status": "passed" if passed else "failed", "fixtures": observations, "violations": violations}, separators=(",", ":")))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
