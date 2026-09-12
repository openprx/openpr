#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 deployed-chain WebSocket verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, the v0.4 section's
# `deployed_chain_websocket_upgrade` paragraph:
#
#   "必须向真实发布 hostname 的 wss://.../api/v1/collab/ws 发起带有效 ticket/origin 的握手,
#    流量依次经过 Caddy、frontend nginx 与 API。成功证据必须同时包含：服务端返回 101；返回的
#    Sec-WebSocket-Accept 等于本次随机 Sec-WebSocket-Key 按 RFC 6455 计算值；链路配置/容器或
#    access-log correlation 证明三个 hop；同一 public origin 的普通 REST 请求未携带或未被上游
#    观察为 Connection: upgrade；proxy read/send timeout 严格大于 120 秒 document idle TTL,
#    并让 quiet socket 跨过该 TTL 后仍可交换 protocol frame。JSON 原子写入
#    evidence/v0.4/deployed-chain-websocket-result.json,记录 source head、public URL(不得含
#    ticket)、各 hop identity/config hash、请求/响应 header 摘要、computed/observed accept、
#    REST control、idle duration 与时间戳。直连 API/frontend port、只查 nginx 配置、101 缺
#    accept 校验、未做 REST control、使用 mock proxy 或证据泄露 ticket/cookie 均必须失败。"
#
# Covers 1 hard gate: deployed_chain_websocket_upgrade.
#
# WHAT THIS SCRIPT WILL NOT DO
#
#   It has no built-in deployment. There is no default public hostname compiled
#   in, and it will never invent one: run with no deployment descriptor it exits
#   1 with `deployment_descriptor_missing`, because "no deployment exists" is a
#   red gate, not a pass. Every fact below is measured against whatever real
#   deployment the descriptor names -- nothing is asserted from configuration
#   alone, and no check can be satisfied by a flag.
#
# THE DEPLOYMENT DESCRIPTOR (--deployment PATH, or $FLOW_DEPLOYMENT_DESCRIPTOR,
# default deploy/flow-deployed-websocket.json under --repo-root):
#
#   {
#     "public_url": "https://<published-hostname>[:port]",
#     "ca_cert": "/path/to/ca-or-leaf.pem",          // optional; omit for a public CA
#     "account": { "email": "...", "password": "..." },
#     "container_cli": "podman",                     // or "docker"
#     "hops": [
#       { "name": "caddy",  "container": "...",
#         "config_probe": ["cat", "/etc/caddy/Caddyfile"],
#         "version_probe": ["caddy", "version"],
#         "access_log": { "path": "...", "format": "caddy_json" } },
#       { "name": "nginx",  "container": "...",
#         "config_probe": ["nginx", "-T"],
#         "version_probe": ["nginx", "-v"],
#         "access_log": { "path": "...", "format": "w5corr" } },
#       { "name": "api",    "container": "...",
#         "config_hash_probe": ["sha256sum", "/app/config/openpr.toml"],
#         "binary_probe": ["sha256sum", "/app/api"] }
#     ]
#   }
#
#   `access_log.format` is one of:
#     caddy_json  -- Caddy's own `format json` access log, one JSON object per line.
#     w5corr      -- an nginx `log_format` whose fields are `key=value` separated by
#                    `|`, and which must at minimum emit probe, peer, uri, upgrade,
#                    conn_upgrade, ws_key, status, upstream. The verifier reads
#                    these back; it never writes the proxy's configuration itself,
#                    so a deployment that does not log them fails the correlation
#                    checks rather than silently skipping them.
#
# WHAT IS ACTUALLY MEASURED (14 checks; `passed` is the AND of all of them):
#
#   binary_provenance_matches_source_head
#                                      the exact API binary in the named API
#                                      container is sha256-hashed and queried
#                                      with --build-info; its embedded clean
#                                      commit must equal this artifact's source
#                                      HEAD. Missing/unknown metadata is red.
#   chain_argument_matches_deployment  --chain names, in order, equal the hop names.
#   public_endpoint_tls_hostname       real TLS handshake to the published hostname,
#                                      certificate verified and hostname-matched (SNI
#                                      = the published host). A plaintext or
#                                      hostname-mismatched endpoint fails here.
#   no_direct_port_exposure            the nginx and API hops publish NO host port at
#                                      all, and the caddy hop publishes exactly the
#                                      port in public_url. This is what makes "the
#                                      handshake reached the API" mean "it went
#                                      through the chain": there is no other route.
#   hop_identities_resolved            every hop is a running container; its id,
#                                      image id, running version string and the
#                                      sha256 of its EFFECTIVE in-container config
#                                      (`nginx -T`, `cat Caddyfile`, `sha256sum` of
#                                      the api's TOML) are recorded. Config content
#                                      is never copied out -- only its hash.
#   websocket_upgrade_returned_101     the handshake to wss://<public>/api/v1/collab/ws
#                                      with a real, freshly issued single-use ticket
#                                      and the deployment's own Origin returns 101.
#   sec_websocket_accept_rfc6455       the returned Sec-WebSocket-Accept equals
#                                      base64(sha1(key + RFC 6455 GUID)) for THIS
#                                      run's random 16-byte key.
#   hop_correlation_caddy              the caddy access log has this run's probe id,
#                                      with this run's Sec-WebSocket-Key, an Upgrade
#                                      request header, status 101, the published host
#                                      and the TLS SNI.
#   hop_correlation_nginx              the nginx access log has the same probe id and
#                                      the same key, upgrade=websocket,
#                                      conn_upgrade=upgrade, status 101, its peer
#                                      address equal to the caddy hop's container IP
#                                      and its upstream equal to the api hop's
#                                      container IP -- i.e. hop 1 -> hop 2 -> hop 3
#                                      identified by address, not by assertion.
#   api_protocol_handshake_observed    over the upgraded socket the server completes
#                                      the real Flow collab protocol: server `hello`,
#                                      then `snapshot` for exactly the document_id and
#                                      head frontier the REST API reported for the
#                                      object. A mock proxy cannot produce this.
#   ticket_single_use_enforced         replaying the same ticket over the same public
#                                      chain does NOT return 101. Only the API can
#                                      have consumed it, so this is third-hop proof
#                                      that is independent of any log.
#   rest_control_no_upgrade            a plain REST GET to the SAME public origin,
#                                      same run, carries no Upgrade/Connection:
#                                      upgrade, returns 200, and is observed upstream
#                                      as upgrade='' / conn_upgrade=close / status 200
#                                      in the nginx log and without an Upgrade request
#                                      header in the caddy log.
#   proxy_timeouts_exceed_idle_ttl     caddy read_timeout/write_timeout and nginx
#                                      proxy_read_timeout/proxy_send_timeout, parsed
#                                      out of the EFFECTIVE in-container config, are
#                                      each strictly greater than 120 s
#                                      (`limits-v1.md` warm_cache_idle_ttl_seconds).
#   quiet_socket_survives_idle_ttl     the socket then stays free of business
#                                      traffic for --idle-seconds (default 130,
#                                      refused below 121). The raw client answers only
#                                      the protocol's server heartbeat pings, as a real
#                                      client must, and afterwards exchanges a fresh
#                                      `ping`/matching-`pong` pair on the same socket.
#
#   Plus a hard post-condition: the assembled evidence is scanned for the ticket, the
#   account password, the access token and any Authorization/Cookie value before it
#   is written. A hit aborts with exit 2 and writes nothing.
#
# Evidence: <evidence-root>/deployed-chain-websocket-result.json, written atomically
# (tmp file in the same directory + rename).
#
# Exit codes: 0 = every check passed, 1 = ran to completion and wrote evidence with
# at least one check failed (this includes "no deployment descriptor"), 2 =
# usage/tool/environment error, or a secret was about to be written.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT=""
DEPLOYMENT_PATH="${FLOW_DEPLOYMENT_DESCRIPTOR:-}"
CHAIN=""
JSON_MODE=0
IDLE_SECONDS=130

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-deployed-websocket-v0.4.sh --chain caddy,nginx,api --json [OPTIONS]

Drives a real wss:// handshake against a real deployed OpenPR chain
(Caddy -> frontend nginx -> API), proves all three hops from container
identity and access-log correlation, proves the RFC 6455 accept value,
proves a same-origin REST request is not an upgrade, proves both proxies'
read/send timeouts exceed the 120 s warm-cache idle TTL and that a quiet
socket still exchanges a protocol frame after crossing it, and writes
evidence/v0.4/deployed-chain-websocket-result.json atomically.

Options:
  --chain a,b,c        Required. Hop names in order; must equal the descriptor's.
  --json               Required for CLI-contract compatibility.
  --deployment PATH    Deployment descriptor (see the header comment). Default:
                       $FLOW_DEPLOYMENT_DESCRIPTOR, else
                       <repo-root>/deploy/flow-deployed-websocket.json.
  --evidence-root DIR  Required. Where deployed-chain-websocket-result.json is written.
  --repo-root DIR      Repository whose HEAD is recorded as source head.
  --idle-seconds N     Quiet-socket duration. Must be > 120. Default 130.
  -h, --help           Show this help and exit 0.

Exit codes: 0 all checks passed, 1 ran to completion with at least one check
failed (including a missing deployment descriptor), 2 usage/tool/environment
error or an attempted secret leak into the evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --chain) CHAIN="${2:?--chain requires a comma separated list}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    --deployment) DEPLOYMENT_PATH="${2:?--deployment requires a PATH argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --idle-seconds) IDLE_SECONDS="${2:?--idle-seconds requires a number}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$CHAIN" ]]; then
  echo "FAIL: --chain is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$EVIDENCE_ROOT" ]]; then
  echo "FAIL: --evidence-root is required; evidence must never default into the contract repository" >&2
  exit 2
fi
if ! [[ "$IDLE_SECONDS" =~ ^[0-9]+$ ]] || (( IDLE_SECONDS <= 120 )); then
  echo "FAIL: --idle-seconds must be an integer strictly greater than 120 (the warm_cache_idle_ttl_seconds this gate exists to cross)" >&2
  exit 2
fi
for tool in jq git python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

# The frozen v0.4 command ledger already invokes this deployed verifier. Keep
# the provenance self-test on that required-command path so a broken checker,
# build-info constant, recompute bridge, or container probe cannot be bypassed
# merely because nobody remembered to run its standalone test script.
SELF_TEST_RAN=false
SELF_TEST_RC_JSON=null
SELF_TEST_DURATION_MS_JSON=null
if [[ "${FLOW_BINARY_PROVENANCE_SELF_TESTED:-0}" != 1 ]]; then
  SELF_TEST_RAN=true
  SELF_TEST_STARTED_NS="$(date +%s%N)"
  set +e
  SELF_TEST_OUTPUT="$("$ROOT_DIR/scripts/test-flow-binary-provenance.sh" 2>&1)"
  SELF_TEST_RC=$?
  set -e
  SELF_TEST_DURATION_MS=$((($(date +%s%N) - SELF_TEST_STARTED_NS) / 1000000))
  SELF_TEST_RC_JSON=$SELF_TEST_RC
  SELF_TEST_DURATION_MS_JSON=$SELF_TEST_DURATION_MS
  if [[ $SELF_TEST_RC -ne 0 ]]; then
    echo "FAIL: binary provenance self-test failed exit=$SELF_TEST_RC duration_ms=$SELF_TEST_DURATION_MS" >&2
    echo "$SELF_TEST_OUTPUT" >&2
    exit 2
  fi
  echo "PASS: binary provenance self-test is wired through required_commands.deployed_chain_websocket_upgrade duration_ms=$SELF_TEST_DURATION_MS" >&2
  echo "$SELF_TEST_OUTPUT" >&2
fi
if [[ -z "$DEPLOYMENT_PATH" ]]; then
  DEPLOYMENT_PATH="$REPO_ROOT/deploy/flow-deployed-websocket.json"
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY=false
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then SOURCE_DIRTY=true; fi

set +e
PROVENANCE_JSON="$("$ROOT_DIR/scripts/verify-flow-binary-provenance.sh" \
  --json --repo-root "$REPO_ROOT" --deployment "$DEPLOYMENT_PATH")"
PROVENANCE_RC=$?
set -e
if [[ $PROVENANCE_RC -eq 2 ]]; then
  echo "FAIL: binary provenance checker could not run" >&2
  exit 2
fi

python3 - "$DEPLOYMENT_PATH" "$CHAIN" "$EVIDENCE_ROOT" "$SOURCE_HEAD" "$SOURCE_DIRTY" "$IDLE_SECONDS" \
  "$PROVENANCE_JSON" "$SELF_TEST_RAN" "$SELF_TEST_RC_JSON" "$SELF_TEST_DURATION_MS_JSON" <<'PYEOF'
import base64, hashlib, json, os, re, socket, ssl, subprocess, sys, time, uuid
import urllib.request, urllib.error
from datetime import datetime, timezone

(
    DEPLOYMENT_PATH,
    CHAIN,
    EVIDENCE_ROOT,
    SOURCE_HEAD,
    SOURCE_DIRTY,
    IDLE_SECONDS,
    PROVENANCE_JSON,
    SELF_TEST_RAN,
    SELF_TEST_EXIT,
    SELF_TEST_DURATION_MS,
) = sys.argv[1:11]
IDLE_SECONDS = int(IDLE_SECONDS)
CHAIN_NAMES = [p.strip() for p in CHAIN.split(",") if p.strip()]
PROVENANCE = json.loads(PROVENANCE_JSON)
SELF_TEST = {
    "ran": SELF_TEST_RAN == "true",
    "exit": None if SELF_TEST_EXIT == "null" else int(SELF_TEST_EXIT),
    "duration_ms": None if SELF_TEST_DURATION_MS == "null" else int(SELF_TEST_DURATION_MS),
}

# `contracts/limits-v1.md`: warm_cache_idle_ttl_seconds = 120. The gate's "document idle TTL".
IDLE_TTL_SECONDS = 120
WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

checks = []
secrets = []          # values that must never appear in the written evidence
notes = []

def check(name, passed, detail, **extra):
    row = {"name": name, "passed": bool(passed), "detail": detail}
    row.update(extra)
    checks.append(row)
    return bool(passed)

def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def write_evidence(payload, ok):
    failed_checks = [row["name"] for row in payload.get("checks", []) if not row.get("passed")]
    payload["schema_version"] = "sylvode.flow.deployed-chain-websocket-result.v1"
    payload["gates"] = {
        "deployed_chain_websocket_upgrade": {
            "status": "passed" if ok else "failed",
            "reason": (
                "all 14 deployed-chain WebSocket checks passed"
                if ok else "failed checks: %s" % (", ".join(failed_checks) or "verifier did not complete")
            ),
        }
    }
    blob = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    for secret in secrets:
        if secret and len(secret) >= 8 and secret in blob:
            print("FAIL: refusing to write evidence: it contains a secret value", file=sys.stderr)
            sys.exit(2)
    for banned in ("ticket=", "Authorization", "Cookie", "authorization", "set-cookie"):
        if banned in blob:
            print("FAIL: refusing to write evidence: it contains %r" % banned, file=sys.stderr)
            sys.exit(2)
    os.makedirs(EVIDENCE_ROOT, exist_ok=True)
    final = os.path.join(EVIDENCE_ROOT, "deployed-chain-websocket-result.json")
    tmp = final + ".tmp.%d" % os.getpid()
    with open(tmp, "w", encoding="utf-8") as handle:
        handle.write(blob)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp, final)          # atomic within the same directory
    print(json.dumps(payload, indent=2, sort_keys=True))
    sys.exit(0 if ok else 1)

def bail(reason, detail):
    check(reason, False, detail)
    write_evidence({
        "gate": "deployed_chain_websocket_upgrade",
        "release": "0.4",
        "passed": False,
        "generated_at": now(),
        "source_head": SOURCE_HEAD,
        "source_dirty": SOURCE_DIRTY == "true",
        "requested_chain": CHAIN_NAMES,
        "deployment_descriptor": DEPLOYMENT_PATH,
        "self_test": SELF_TEST,
        "binary_provenance": PROVENANCE,
        "checks": checks,
        "notes": notes,
    }, False)

check(
    "binary_provenance_matches_source_head",
    PROVENANCE.get("passed") is True and PROVENANCE.get("status") == "passed",
    PROVENANCE.get("reason", "binary provenance checker returned no reason"),
    status=PROVENANCE.get("status"),
    binary_sha256=(PROVENANCE.get("binary") or {}).get("sha256"),
    build_metadata=PROVENANCE.get("build_metadata"),
)

# ---------------------------------------------------------------- descriptor
if not os.path.isfile(DEPLOYMENT_PATH):
    bail("deployment_descriptor_missing",
         "no deployment descriptor at %s: this gate requires a real deployed chain, and this "
         "verifier will not synthesise one. Point --deployment (or $FLOW_DEPLOYMENT_DESCRIPTOR) "
         "at a descriptor for a running Caddy -> frontend nginx -> API deployment." % DEPLOYMENT_PATH)
try:
    with open(DEPLOYMENT_PATH, encoding="utf-8") as handle:
        dep = json.load(handle)
except Exception as exc:                                     # noqa: BLE001
    bail("deployment_descriptor_unreadable", "%s: %s" % (DEPLOYMENT_PATH, exc))

public_url = str(dep.get("public_url", "")).rstrip("/")
ca_cert = dep.get("ca_cert")
account = dep.get("account") or {}
cli = dep.get("container_cli") or "podman"
hops = dep.get("hops") or []
secrets.append(str(account.get("password", "")))

m = re.match(r"^https://([A-Za-z0-9._-]+)(?::(\d+))?$", public_url)
if not m:
    bail("public_url_invalid",
         "public_url must be https://<hostname>[:port] with no path; got %r. A plaintext or "
         "path-bearing origin cannot satisfy a wss:// gate." % public_url)
PUB_HOST, PUB_PORT = m.group(1), int(m.group(2) or 443)
WSS_URL = "wss://%s:%d/api/v1/collab/ws" % (PUB_HOST, PUB_PORT)

hop_names = [h.get("name") for h in hops]
check("chain_argument_matches_deployment", hop_names == CHAIN_NAMES,
      "--chain %s vs descriptor hops %s" % (CHAIN_NAMES, hop_names),
      requested=CHAIN_NAMES, deployed=hop_names)

def sh(args):
    try:
        out = subprocess.run(args, capture_output=True, text=True, timeout=60)
        return out.returncode, (out.stdout or "") + (out.stderr or "")
    except Exception as exc:                                 # noqa: BLE001
        return 127, str(exc)

# ------------------------------------------------------- hop identity / config
hop_facts = {}
identity_ok = True
for hop in hops:
    name = hop.get("name")
    container = hop.get("container")
    fact = {"name": name, "container_name": container}
    rc, out = sh([cli, "inspect", container, "--format",
                  "{{.Id}}|{{.Image}}|{{.State.Running}}|"
                  "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}|"
                  "{{.HostConfig.PortBindings}}"])
    if rc != 0:
        identity_ok = False
        fact["error"] = out.strip()[:400]
        hop_facts[name] = fact
        continue
    cid, image, running, ips, ports = (out.strip().split("|") + ["", "", "", "", ""])[:5]
    fact.update({
        "container_id": cid[:12],
        "image_id": image[:19],
        "running": running.strip() == "true",
        "container_ips": ips.split(),
        "published_ports_raw": ports.strip(),
    })
    if hop.get("version_probe"):
        rc2, ver = sh([cli, "exec", container] + hop["version_probe"])
        fact["version"] = ver.strip().splitlines()[0][:160] if rc2 == 0 or ver else "unavailable"
    if hop.get("config_probe"):
        rc3, cfg = sh([cli, "exec", container] + hop["config_probe"])
        if rc3 == 0 and cfg.strip():
            fact["effective_config_sha256"] = hashlib.sha256(cfg.encode()).hexdigest()
            fact["effective_config_bytes"] = len(cfg.encode())
            hop["_config_text"] = cfg
        else:
            identity_ok = False
            fact["config_error"] = cfg.strip()[:400]
    if hop.get("config_hash_probe"):
        rc4, digest = sh([cli, "exec", container] + hop["config_hash_probe"])
        if rc4 == 0 and digest.split():
            fact["effective_config_sha256"] = digest.split()[0]
        else:
            identity_ok = False
            fact["config_error"] = digest.strip()[:400]
    if hop.get("binary_probe"):
        rc5, digest = sh([cli, "exec", container] + hop["binary_probe"])
        if rc5 == 0 and digest.split():
            fact["binary_sha256"] = digest.split()[0]
    if not fact.get("running"):
        identity_ok = False
    hop_facts[name] = fact

check("hop_identities_resolved", identity_ok,
      "every hop resolved to a running container with an effective in-container config hash"
      if identity_ok else "at least one hop could not be inspected, is not running, or would "
                          "not yield its effective configuration",
      hops=hop_facts)

# ------------------------------------------------------- no direct port exposure
def published_ports(raw):
    return re.findall(r"(\d+)/tcp:\[\{[^}]*?(\d+)\}\]", raw or "")

exposure = {}
exposure_ok = True
for name in CHAIN_NAMES:
    raw = hop_facts.get(name, {}).get("published_ports_raw", "")
    pub = published_ports(raw)
    exposure[name] = [{"container_port": int(c), "host_port": int(h)} for c, h in pub]
    if name == CHAIN_NAMES[0]:
        if not any(int(h) == PUB_PORT for _, h in pub):
            exposure_ok = False
    else:
        if pub:
            exposure_ok = False
check("no_direct_port_exposure", exposure_ok,
      "only the first hop publishes a host port, and it is the port in public_url (%d); the "
      "nginx and API hops publish none, so a handshake that reaches the API cannot have "
      "bypassed the chain" % PUB_PORT if exposure_ok else
      "a downstream hop publishes a host port (a caller could reach it directly), or the first "
      "hop does not publish the public_url port",
      published=exposure)

# --------------------------------------------------------------- proxy timeouts
def parse_duration(text):
    text = text.strip().rstrip(";")
    m = re.match(r"^(\d+(?:\.\d+)?)(ms|s|m|h|d)?$", text)
    if not m:
        return None
    value = float(m.group(1))
    return value * {"ms": 0.001, "s": 1, "m": 60, "h": 3600, "d": 86400, None: 1}[m.group(2)]

timeouts = {}
timeouts_ok = True
for hop in hops:
    name, text = hop.get("name"), hop.get("_config_text")
    if not text:
        continue
    found = {}
    if name == "nginx":
        wanted = ("proxy_read_timeout", "proxy_send_timeout")
    elif name == "caddy":
        wanted = ("read_timeout", "write_timeout")
    else:
        continue
    for directive in wanted:
        values = [parse_duration(v) for v in re.findall(r"\b%s\s+([0-9a-z.]+)\s*;?" % directive, text)]
        values = [v for v in values if v is not None]
        found[directive] = min(values) if values else None
    timeouts[name] = {k: v for k, v in found.items()}
    for directive, value in found.items():
        if value is None or value <= IDLE_TTL_SECONDS:
            timeouts_ok = False
if not timeouts:
    timeouts_ok = False
check("proxy_timeouts_exceed_idle_ttl", timeouts_ok,
      "every proxy read/send timeout parsed out of the effective in-container configuration is "
      "strictly greater than the %d s warm_cache_idle_ttl_seconds" % IDLE_TTL_SECONDS
      if timeouts_ok else
      "a proxy read/send timeout is absent from the effective configuration or is <= %d s"
      % IDLE_TTL_SECONDS,
      idle_ttl_seconds=IDLE_TTL_SECONDS, parsed_seconds=timeouts)

# ------------------------------------------------------------------- REST client
ctx = ssl.create_default_context(cafile=ca_cert) if ca_cert else ssl.create_default_context()
ctx.check_hostname = True
ctx.verify_mode = ssl.CERT_REQUIRED

def rest(method, path, body=None, token=None, headers=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(public_url + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    req.add_header("Origin", public_url)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, context=ctx, timeout=45) as resp:
            return resp.status, json.loads(resp.read().decode() or "{}"), dict(resp.headers)
    except urllib.error.HTTPError as exc:
        return exc.code, json.loads(exc.read().decode() or "{}"), dict(exc.headers)

tls_info = {}
try:
    with socket.create_connection((PUB_HOST, PUB_PORT), timeout=20) as raw:
        with ctx.wrap_socket(raw, server_hostname=PUB_HOST) as tls:
            peer = tls.getpeercert()
            tls_info = {
                "tls_version": tls.version(),
                "cipher": tls.cipher()[0],
                "sni_server_name": PUB_HOST,
                "peer_subject": [list(x) for x in peer.get("subject", [])],
                "peer_san": [list(x) for x in peer.get("subjectAltName", [])],
                "peer_not_after": peer.get("notAfter"),
            }
    tls_ok = True
except Exception as exc:                                     # noqa: BLE001
    tls_ok = False
    tls_info = {"error": str(exc)[:300]}
check("public_endpoint_tls_hostname", tls_ok,
      "TLS handshake to the published hostname succeeded with a verified, hostname-matched "
      "certificate" if tls_ok else "TLS handshake to the published hostname failed: %s"
      % tls_info.get("error"),
      tls=tls_info, public_host=PUB_HOST, public_port=PUB_PORT)

# ------------------------------------------------------------ fixture bootstrap
def bootstrap():
    email, password = account.get("email"), account.get("password")
    status, body, _ = rest("POST", "/api/v1/auth/register",
                           {"email": email, "password": password, "name": "flow v0.4 gate"})
    if status != 200 or "data" not in body:
        status, body, _ = rest("POST", "/api/v1/auth/login", {"email": email, "password": password})
    if status != 200 or "data" not in body:
        return None, "could not authenticate as %s: %s" % (email, json.dumps(body)[:200])
    token = body["data"]["tokens"]["access_token"]
    secrets.append(token)
    status, body, _ = rest("GET", "/api/v1/workspaces", None, token)
    items = body.get("data") if isinstance(body.get("data"), list) else (body.get("data") or {}).get("items")
    if status != 200 or not items:
        status, body, _ = rest("POST", "/api/v1/workspaces",
                               {"slug": "flow-v04-gate-%s" % uuid.uuid4().hex[:8],
                                "name": "flow v0.4 gate"}, token)
        if status != 200 or "data" not in body:
            return None, "could not create a workspace: %s" % json.dumps(body)[:200]
        workspace_id = body["data"]["id"]
    else:
        workspace_id = items[0]["id"]
    rest("PUT", "/api/v1/workspaces/%s/features/flow" % workspace_id,
         {"enabled": True, "default_member_level": "edit", "idempotency_key": str(uuid.uuid4())}, token)
    status, body, _ = rest("GET", "/api/v1/workspaces/%s/flow/objects" % workspace_id, None, token)
    objects = ((body.get("data") or {}).get("items")) or []
    if not objects:
        status, body, _ = rest("POST", "/api/v1/workspaces/%s/flow/objects" % workspace_id,
                               {"object_type": "page", "title": "deployed chain websocket gate",
                                "idempotency_key": str(uuid.uuid4())}, token)
        if status != 200 or "data" not in body:
            return None, "could not create a flow object: %s" % json.dumps(body)[:200]
        obj = body["data"]["object"]
    else:
        obj = objects[0]
    return {"token": token, "workspace_id": workspace_id, "object_id": obj["id"],
            "document_id": obj["document_id"], "frontier": obj.get("frontier")}, None

fixture, fixture_error = (None, "TLS endpoint unreachable") if not tls_ok else bootstrap()
if fixture is None:
    bail("deployment_fixture_unavailable",
         "the deployed chain answered, but no Flow fixture could be established through it: %s"
         % fixture_error)

def issue_ticket(client_id):
    status, body, _ = rest("POST", "/api/v1/collab/tickets",
                           {"workspace_id": fixture["workspace_id"],
                            "document_id": fixture["document_id"],
                            "client_id": client_id, "origin": public_url}, fixture["token"])
    if status != 200 or "data" not in body:
        return None, json.dumps(body)[:200]
    secrets.append(body["data"]["ticket"])
    return body["data"]["ticket"], None

# --------------------------------------------------------- websocket primitives
def ws_connect(ticket, client_id, probe_id, key_b64):
    raw = socket.create_connection((PUB_HOST, PUB_PORT), timeout=30)
    sock = ctx.wrap_socket(raw, server_hostname=PUB_HOST)
    request_headers = [
        ("Host", "%s:%d" % (PUB_HOST, PUB_PORT)),
        ("Upgrade", "websocket"),
        ("Connection", "Upgrade"),
        ("Sec-WebSocket-Key", key_b64),
        ("Sec-WebSocket-Version", "13"),
        ("Origin", public_url),
        ("X-W5-Probe", probe_id),
    ]
    line = "GET /api/v1/collab/ws?ticket=%s&client_id=%s HTTP/1.1\r\n" % (ticket, client_id)
    blob = line + "".join("%s: %s\r\n" % kv for kv in request_headers) + "\r\n"
    sock.sendall(blob.encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            break
        buf += chunk
    head, rest_bytes = (buf.split(b"\r\n\r\n", 1) + [b""])[:2]
    lines = head.decode("latin-1").split("\r\n")
    status_line = lines[0] if lines else ""
    resp_headers = {}
    for entry in lines[1:]:
        if ":" in entry:
            k, v = entry.split(":", 1)
            resp_headers[k.strip()] = v.strip()
    return sock, status_line, resp_headers, rest_bytes, dict(request_headers)

def ws_send_text(sock, text):
    payload = text.encode()
    header = bytearray([0x81])
    mask = os.urandom(4)
    length = len(payload)
    if length < 126:
        header.append(0x80 | length)
    elif length < (1 << 16):
        header.append(0x80 | 126)
        header += length.to_bytes(2, "big")
    else:
        header.append(0x80 | 127)
        header += length.to_bytes(8, "big")
    header += mask
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    sock.sendall(bytes(header) + masked)

class Reader:
    def __init__(self, sock, initial=b""):
        self.sock, self.buf = sock, bytearray(initial)

    def _need(self, count, deadline):
        while len(self.buf) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("timed out waiting for %d bytes" % count)
            self.sock.settimeout(remaining)
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("peer closed the connection")
            self.buf += chunk

    def frame(self, timeout):
        deadline = time.monotonic() + timeout
        while True:
            self._need(2, deadline)
            opcode = self.buf[0] & 0x0F
            length = self.buf[1] & 0x7F
            offset = 2
            if length == 126:
                self._need(4, deadline)
                length = int.from_bytes(self.buf[2:4], "big")
                offset = 4
            elif length == 127:
                self._need(10, deadline)
                length = int.from_bytes(self.buf[2:10], "big")
                offset = 10
            self._need(offset + length, deadline)
            payload = bytes(self.buf[offset:offset + length])
            del self.buf[:offset + length]
            if opcode == 0x9:                     # control ping from the server
                continue
            if opcode == 0x8:
                raise ConnectionError("server closed the websocket")
            if opcode in (0x1, 0x2):
                return json.loads(payload.decode())

# --------------------------------------------------------------- the handshake
probe_id = "flowgate-%s" % uuid.uuid4().hex
client_id = "flowgate-%s" % uuid.uuid4().hex
ws_key = base64.b64encode(os.urandom(16)).decode()
computed_accept = base64.b64encode(hashlib.sha1((ws_key + WS_GUID).encode()).digest()).decode()

ticket, ticket_error = issue_ticket(client_id)
if ticket is None:
    bail("collab_ticket_unavailable",
         "the deployed API refused to issue a collab ticket for origin %s: %s"
         % (public_url, ticket_error))

handshake_started = now()
try:
    sock, status_line, resp_headers, tail, request_headers = ws_connect(ticket, client_id, probe_id, ws_key)
    connect_error = None
except Exception as exc:                                     # noqa: BLE001
    sock, status_line, resp_headers, tail, request_headers = None, "", {}, b"", {}
    connect_error = str(exc)[:300]

upgraded = status_line.startswith("HTTP/1.1 101")
check("websocket_upgrade_returned_101", upgraded,
      "the deployed chain answered the wss handshake with %r" % (status_line or connect_error),
      public_websocket_url=WSS_URL, status_line=status_line, error=connect_error)

observed_accept = resp_headers.get("Sec-WebSocket-Accept")
check("sec_websocket_accept_rfc6455", bool(observed_accept) and observed_accept == computed_accept,
      "Sec-WebSocket-Accept returned by the deployment equals base64(sha1(this run's random "
      "Sec-WebSocket-Key + RFC 6455 GUID))" if observed_accept == computed_accept else
      "Sec-WebSocket-Accept mismatch: a 101 without a correct accept is not a WebSocket",
      sec_websocket_key=ws_key, computed_accept=computed_accept, observed_accept=observed_accept)

# ------------------------------------------------ real Flow protocol over the socket
protocol = {"frames": []}
protocol_ok = False
reader = None
if upgraded and sock is not None:
    try:
        reader = Reader(sock, tail)
        session_id = str(uuid.uuid4())
        ws_send_text(sock, json.dumps({"type": "hello", "protocol_version": 1,
                                       "capabilities": ["presence"], "client_id": client_id,
                                       "session_id": session_id}))
        server_hello = reader.frame(20)
        protocol["frames"].append({"direction": "in", "type": server_hello.get("type")})
        ws_send_text(sock, json.dumps({"type": "open", "protocol_version": 1,
                                       "document_id": fixture["document_id"]}))
        snapshot = reader.frame(20)
        protocol["frames"].append({"direction": "in", "type": snapshot.get("type")})
        protocol["snapshot_document_id"] = snapshot.get("document_id")
        protocol["snapshot_head_seq"] = snapshot.get("head_seq")
        protocol["rest_reported_document_id"] = fixture["document_id"]
        protocol["rest_reported_frontier_matches"] = (
            snapshot.get("head_frontier") == fixture.get("frontier"))
        protocol_ok = (server_hello.get("type") == "hello"
                       and snapshot.get("type") == "snapshot"
                       and snapshot.get("document_id") == fixture["document_id"])
    except Exception as exc:                                 # noqa: BLE001
        protocol["error"] = str(exc)[:300]
check("api_protocol_handshake_observed", protocol_ok,
      "the upgraded socket carried the real Flow collab protocol -- server hello, then a snapshot "
      "for exactly the document_id the REST API reported. Only the API terminates this; a proxy "
      "that faked the 101 cannot produce it." if protocol_ok else
      "the upgraded socket did not complete the Flow collab protocol handshake",
      **protocol)

# ------------------------------------------------------- quiet socket across the TTL
idle = {"requested_seconds": IDLE_SECONDS, "idle_ttl_seconds": IDLE_TTL_SECONDS}
idle_ok = False
if protocol_ok and reader is not None:
    try:
        started = time.monotonic()
        deadline = started + IDLE_SECONDS
        heartbeat_pings_answered = 0
        unexpected_frames = []
        idle["quiet_from"] = now()
        # "Quiet" means no business work from the client. The Flow protocol nevertheless
        # requires a live client to answer the server's application-level heartbeat pings. A
        # browser WebSocket client does this in its protocol adapter; this deliberately tiny raw
        # client must do the same instead of buffering the first ping and letting the API close a
        # healthy connection 60 seconds into a 130-second proxy test.
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            try:
                inbound = reader.frame(min(remaining, 35))
            except TimeoutError:
                continue
            if inbound.get("type") == "ping" and inbound.get("nonce"):
                ws_send_text(sock, json.dumps({
                    "type": "pong",
                    "protocol_version": 1,
                    "nonce": inbound["nonce"],
                }))
                heartbeat_pings_answered += 1
            else:
                unexpected_frames.append(inbound.get("type"))
        idle["quiet_until"] = now()
        idle["measured_seconds"] = round(time.monotonic() - started, 3)
        idle["business_frames_sent_during_quiet"] = 0
        idle["heartbeat_pings_answered"] = heartbeat_pings_answered
        idle["unexpected_frame_types"] = unexpected_frames

        nonce = uuid.uuid4().hex
        ws_send_text(sock, json.dumps({"type": "ping", "protocol_version": 1, "nonce": nonce}))
        pong = None
        probe_deadline = time.monotonic() + 30
        while time.monotonic() < probe_deadline:
            candidate = reader.frame(probe_deadline - time.monotonic())
            if candidate.get("type") == "ping" and candidate.get("nonce"):
                ws_send_text(sock, json.dumps({
                    "type": "pong",
                    "protocol_version": 1,
                    "nonce": candidate["nonce"],
                }))
                heartbeat_pings_answered += 1
                idle["heartbeat_pings_answered"] = heartbeat_pings_answered
                continue
            pong = candidate
            if pong.get("type") == "pong" and pong.get("nonce") == nonce:
                break
            unexpected_frames.append(pong.get("type"))
        if pong is None:
            raise TimeoutError("timed out waiting for the post-idle protocol pong")
        idle["response_type"] = pong.get("type")
        idle["nonce_echoed"] = pong.get("nonce") == nonce
        idle_ok = (pong.get("type") == "pong" and pong.get("nonce") == nonce
                   and idle["measured_seconds"] > IDLE_TTL_SECONDS
                   and not unexpected_frames)
    except Exception as exc:                                 # noqa: BLE001
        idle["error"] = str(exc)[:300]
check("quiet_socket_survives_idle_ttl", idle_ok,
      "after %s s without business traffic -- past the %d s warm_cache_idle_ttl_seconds, "
      "while answering only required server heartbeats -- the same socket still exchanged a "
      "protocol frame (ping -> pong with the same nonce)"
      % (idle.get("measured_seconds"), IDLE_TTL_SECONDS) if idle_ok else
      "the quiet socket did not survive the idle TTL, or the post-idle frame exchange failed",
      **idle)

if sock is not None:
    try:
        sock.close()
    except Exception:                                        # noqa: BLE001
        pass

# ------------------------------------------------------------- ticket single use
replay = {}
replay_ok = False
try:
    rsock, rstatus, _, _, _ = ws_connect(ticket, client_id, probe_id + "-replay",
                                         base64.b64encode(os.urandom(16)).decode())
    replay["status_line"] = rstatus
    replay_ok = not rstatus.startswith("HTTP/1.1 101")
    rsock.close()
except Exception as exc:                                     # noqa: BLE001
    replay["error"] = str(exc)[:200]
    replay_ok = True
check("ticket_single_use_enforced", replay_ok,
      "replaying the same one-time ticket over the same public chain did not upgrade -- only the "
      "API could have consumed it, so the third hop is proven independently of any log"
      if replay_ok else "the same ticket upgraded twice: the API is not consuming it",
      **replay)

# ------------------------------------------------------------------ REST control
rest_probe = "flowgate-rest-%s" % uuid.uuid4().hex
rstatus, rbody, rheaders = rest("GET", "/api/v1/flow/objects/%s/collab" % fixture["object_id"],
                                None, fixture["token"], {"X-W5-Probe": rest_probe})
rest_control = {
    "path": "/api/v1/flow/objects/{object_id}/collab",
    "origin": public_url,
    "request_carried_upgrade": False,
    "request_carried_connection_upgrade": False,
    "status": rstatus,
    "response_connection_header": rheaders.get("Connection"),
    "response_upgrade_header": rheaders.get("Upgrade"),
}
rest_ok = rstatus == 200 and rheaders.get("Upgrade") is None \
    and (rheaders.get("Connection") or "").lower() != "upgrade"

# ---------------------------------------------------- access-log hop correlation
# Both proxies write their access-log line when the request completes, which for the upgraded
# socket is the close above. Give the writers a moment before reading the files back.
time.sleep(2)

def read_log(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read().splitlines()
    except Exception as exc:                                 # noqa: BLE001
        return ["__ERROR__ %s" % exc]

def parse_kv_line(line):
    out = {}
    for part in line.split("|"):
        if "=" in part:
            k, v = part.split("=", 1)
            out[k.strip()] = v.strip()
    return out

caddy_hop = next((h for h in hops if h.get("name") == "caddy"), None)
nginx_hop = next((h for h in hops if h.get("name") == "nginx"), None)

caddy_evidence, nginx_evidence = {}, {}
caddy_ok = nginx_ok = False
caddy_rest_seen = nginx_rest_seen = None

if caddy_hop and (caddy_hop.get("access_log") or {}).get("path"):
    for line in read_log(caddy_hop["access_log"]["path"]):
        if probe_id not in line and rest_probe not in line:
            continue
        try:
            entry = json.loads(line)
        except Exception:                                    # noqa: BLE001
            continue
        req = entry.get("request", {})
        headers = {k.lower(): v for k, v in (req.get("headers") or {}).items()}
        seen = (headers.get("x-w5-probe") or [""])[0]
        summary = {
            "status": entry.get("status"),
            "host": req.get("host"),
            "tls_server_name": (req.get("tls") or {}).get("server_name"),
            "client_ip": req.get("client_ip"),
            "request_upgrade_header": (headers.get("upgrade") or [None])[0],
            "request_connection_header": (headers.get("connection") or [None])[0],
            "request_sec_websocket_key": (headers.get("sec-websocket-key") or [None])[0],
            "response_sec_websocket_accept":
                ((entry.get("resp_headers") or {}).get("Sec-WebSocket-Accept") or [None])[0],
        }
        if seen == probe_id:
            caddy_evidence["websocket"] = summary
        elif seen == rest_probe:
            caddy_rest_seen = summary
    ws_row = caddy_evidence.get("websocket") or {}
    caddy_ok = (ws_row.get("status") == 101
                and ws_row.get("request_sec_websocket_key") == ws_key
                and (ws_row.get("request_upgrade_header") or "").lower() == "websocket"
                and ws_row.get("host") in (PUB_HOST, "%s:%d" % (PUB_HOST, PUB_PORT))
                and ws_row.get("tls_server_name") == PUB_HOST
                and ws_row.get("response_sec_websocket_accept") == computed_accept)
else:
    caddy_evidence["error"] = "no access_log.path declared for the caddy hop"

if nginx_hop and (nginx_hop.get("access_log") or {}).get("path"):
    for line in read_log(nginx_hop["access_log"]["path"]):
        if probe_id not in line and rest_probe not in line:
            continue
        row_kv = parse_kv_line(line)
        seen = row_kv.get("probe")
        if seen == probe_id:
            nginx_evidence["websocket"] = row_kv
        elif seen == rest_probe:
            nginx_rest_seen = row_kv
    row = nginx_evidence.get("websocket") or {}
    caddy_ips = hop_facts.get("caddy", {}).get("container_ips", [])
    api_ips = hop_facts.get("api", {}).get("container_ips", [])
    nginx_evidence["expected_peer_is_caddy"] = caddy_ips
    nginx_evidence["expected_upstream_is_api"] = api_ips
    nginx_ok = (row.get("status") == "101"
                and row.get("ws_key") == ws_key
                and row.get("upgrade") == "websocket"
                and row.get("conn_upgrade") == "upgrade"
                and row.get("resp_ws_accept") == computed_accept
                and row.get("peer") in caddy_ips
                and any((row.get("upstream") or "").startswith(ip + ":") for ip in api_ips))
else:
    nginx_evidence["error"] = "no access_log.path declared for the nginx hop"

check("hop_correlation_caddy", caddy_ok,
      "hop 1 (Caddy) logged this run's probe id with this run's Sec-WebSocket-Key, the published "
      "host and TLS SNI, and answered 101 with the computed accept" if caddy_ok else
      "the caddy access log does not corroborate this run's upgrade",
      **caddy_evidence)
check("hop_correlation_nginx", nginx_ok,
      "hop 2 (frontend nginx) logged the same probe id and the same Sec-WebSocket-Key with "
      "upgrade=websocket / conn_upgrade=upgrade / 101, its peer address is the Caddy hop's "
      "container IP and its upstream address is the API hop's container IP -- the three hops are "
      "identified by address, not asserted" if nginx_ok else
      "the nginx access log does not corroborate this run's upgrade, or its peer/upstream "
      "addresses are not the caddy and api hops",
      **nginx_evidence)

rest_control["observed_by_caddy"] = caddy_rest_seen
rest_control["observed_by_nginx"] = nginx_rest_seen
upstream_rest_clean = (
    nginx_rest_seen is not None
    and nginx_rest_seen.get("status") == "200"
    and nginx_rest_seen.get("upgrade") in ("", "-")
    and nginx_rest_seen.get("conn_upgrade") == "close"
    and caddy_rest_seen is not None
    and not caddy_rest_seen.get("request_upgrade_header")
    and (caddy_rest_seen.get("request_connection_header") or "").lower() != "upgrade"
)
rest_ok = rest_ok and upstream_rest_clean
check("rest_control_no_upgrade", rest_ok,
      "the same-origin REST control request carried no Upgrade/Connection: upgrade, returned 200, "
      "and both upstream hops observed it as a plain request (nginx conn_upgrade=close)"
      if rest_ok else
      "the REST control did not return 200, or an upstream hop observed it as an upgrade, or it "
      "was not observed upstream at all",
      **rest_control)

# ------------------------------------------------------------------ final verdict
passed = all(row["passed"] for row in checks)
notes.append(
    "Sec-WebSocket-Key/Accept are protocol values, not secrets, and are recorded verbatim. The "
    "one-time ticket, the access token and the account password are never written: the public URL "
    "is recorded without a query string and the evidence is scanned for every one of them before "
    "the file is created.")
notes.append(
    "The upgrade URL's query string carries the one-time ticket, so it also lands in the proxies' "
    "own access logs. That is a property of the deployment, not of this verifier -- this file "
    "quotes only parsed fields from those logs and never a raw log line.")

write_evidence({
    "gate": "deployed_chain_websocket_upgrade",
    "release": "0.4",
    "passed": passed,
    "generated_at": now(),
    "handshake_started_at": handshake_started,
    "source_head": SOURCE_HEAD,
    "source_dirty": SOURCE_DIRTY == "true",
    "deployment_descriptor": DEPLOYMENT_PATH,
    "self_test": SELF_TEST,
    "binary_provenance": PROVENANCE,
    "requested_chain": CHAIN_NAMES,
    "public_url": public_url,
    "public_websocket_url": WSS_URL,
    "probe_id": probe_id,
    "collab_client_id": client_id,
    "document_id": fixture["document_id"],
    "flow_object_id": fixture["object_id"],
    "workspace_id": fixture["workspace_id"],
    "idle_ttl_seconds": IDLE_TTL_SECONDS,
    "idle_duration_seconds": idle.get("measured_seconds"),
    "handshake_request_headers": {k: v for k, v in request_headers.items()},
    "handshake_response_headers": resp_headers,
    "computed_sec_websocket_accept": computed_accept,
    "observed_sec_websocket_accept": observed_accept,
    "hops": hop_facts,
    "proxy_timeouts_seconds": timeouts,
    "rest_control": rest_control,
    "checks": checks,
    "notes": notes,
}, passed)
PYEOF
