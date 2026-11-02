"""Record shared native pod/endpoint log command coverage."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
mapping = {
    ("runpod/rp_cli/logs.py", "_show_logs"): "cli::logs::Options::show",
    ("runpod/rp_cli/logs.py", "show_logs"): "cli::logs::Options::execute",
    ("runpod/rp_cli/endpoint.py", "logs"): "cli::logs::command and Options::read (endpoint-id)",
    ("runpod/rp_cli/pod.py", "logs"): "cli::logs::command and Options::read (pod-id)",
}
count = 0
for entry in data["entries"]:
    key = (entry["file"], entry["name"])
    if key in mapping:
        entry.update(rust=mapping[key], status="adapted", note="Native finite/follow log commands reuse the SDK SSE reader. Preserves worker filtering/discovery, cursor retries, snapshot cutoff/raw timestamp ordering, output modes, disappearing workers and cancellation. Owned streams replace asyncio tasks; RFC3339 timestamps, native transport errors and compact UTF-8 JSON are documented adaptations. See reviews/cli-logs.md.")
        count += 1
    if "note" in entry:
        entry["note"] = entry["note"].replace("Remaining shell-completion and log workflows are tracked separately.", "Shell completion is tracked separately; log workflows are covered by cli::logs.")
assert count == 4, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))

path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
count = 0
for entry in data["entries"]:
    name = entry["name"]
    if (entry["file"] == "tests/test_cli/test_cli_logs.py"
        or (entry["file"] == "tests/test_cli/test_endpoint_command.py" and name == "test_logs_routes_options_to_shared_streamer")
        or (entry["file"] == "tests/test_cli/test_pod_command.py" and name in {"test_logs_forward_options", "test_logs_reject_tail_above_api_limit"})):
        prefix = "cli_logs::" if name == "test_logs_reject_tail_above_api_limit" else "cli::logs::tests::"
        entry.update(rust=prefix + name, status="ported", note="Named Rust counterpart; 39 unchanged parameterized source cases pass. Deterministic stream/timing tests preserve source assertions, including companion-first ordering. Fourteen captured snapshot cases replay through native processes and HTTP/SSE mocks; additional real socket tests verify cursor reconnect and stream closure.")
        count += 1
assert count == 14, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
