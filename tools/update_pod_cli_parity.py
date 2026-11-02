"""Map native pod resource commands and their unchanged source tests."""
import json
import re
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "_pod_choices": "cli::pod::resolve_id (action-filtered choices)",
    "_resolve_pod_id": "cli::pod::resolve_id", "_say": "cli::pod::say",
    "_status": "cli::pod::view::status", "_summary": "cli::pod::view::summary",
    "_show": "cli::pod::view::show", "_wait_running": "cli::pod::wait::Waiter::run",
    "_container_body": "cli::pod::config::Container::body",
    "_container_options": "cli::pod::options::container",
    "_gpu_body": "cli::pod::options::Create::gpu",
    "_datacenter_ids": "cli::pod::options::Create::datacenters",
    "_create_body": "cli::pod::options::Create::body", "pod": "cli::pod::command",
    **{name: "cli::pod::options::Options::execute" for name in
       ["list_pods", "get", "create", "update", "_act", "start", "stop", "restart", "terminate"]},
}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
count = 0
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/pod.py" and entry["name"] in mapping:
        entry.update(rust=mapping[entry["name"]], status="adapted", note="Native resource CLI with typed GPU/CPU selection, merged container/mount payloads, displayed-status filtering, action-aware pickers, restart readiness checks and confirmation before deletion. Owned credentials, bounded integers, async polling, plain terminal messages and typed errors are native adaptations. SSH connect/sync, completion and Rich-style console UI remain separately pending. See reviews/pod-cli.md.")
        count += 1
    if entry["file"] == "runpod/rp_cli/pod.py" and entry["name"] == "logs":
        entry.update(rust="cli::pod::show_logs and cli::logs::Options::execute_with_client",
            note="Shared native SSE log command, now resolving omitted pod IDs through the same resource picker before streaming. Snapshot/reconnect adaptations remain documented in reviews/cli-logs.md; optional-ID parity is verified in reviews/pod-cli.md.")
assert count == 22, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))

path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
names = {case["name"] for case in json.loads((ROOT / "tests/fixtures/pod_cli.json").read_text())}
count = 0
for entry in data["entries"]:
    if entry["file"] != "tests/test_cli/test_pod_command.py" or entry["name"] not in names: continue
    cls, name = entry["name"].split(".")
    cls = re.sub(r"(?<!^)(?=[A-Z])", "_", cls.removeprefix("Test")).lower()
    entry.update(rust=f"cli::pod::tests::test_{cls}_{name.removeprefix('test_')}", status="ported",
        note="Named native counterpart replaying captured REST/catalog sequences, JSON/human fields, notices, errors and interaction. 82 unchanged parameterized source cases yield 89 invocations; 71 also replay through native executables. Fake monotonic time preserves source no-op sleeps; zero Duration represents the source's injected negative timeout. Additional regressions verify optional log selection and rejection of malformed current settings before PATCH.")
    count += 1
assert count == 48, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
