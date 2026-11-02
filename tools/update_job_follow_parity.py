"""Record job wait/stream parity without implying endpoint submission is ported."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "_follow_job": "cli::endpoint::job_follow::Follow",
    "_print_job": "cli::endpoint::job_follow::print_job",
    "job_wait": "cli::endpoint::jobs::execute / wait",
    "job_stream": "cli::endpoint::jobs::execute / stream",
}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
count = 0
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/endpoint.py" and entry["name"] in mapping:
        entry.update(rust=mapping[entry["name"]], status="adapted",
            note="Native wait/stream CLI with monotonic request-bounded deadlines, terminal draining, read-timeout retries and interrupt-specific resume output. Native JSON whitespace/transport diagnostics and total HTTP deadlines replace Python spellings/read-idle timeouts. See reviews/job-follow.md. Endpoint submission/load-balancer orchestration has its own mapping.")
        count += 1
assert count == 4, count
path.write_text(json.dumps(data, indent=2)+"\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
count = 0
for entry in data["entries"]:
    name = entry["name"].split(".")[-1]
    if entry["file"] == "tests/test_cli/test_endpoint_command.py" and name in {"test_job_wait_and_stream", "test_job_stream_interrupt"}:
        entry.update(rust="cli::endpoint::job_follow::tests::"+name, status="ported",
            note="Named Rust counterpart using actual pinned Python observer fixtures; separate executable tests verify dispatch, credential-independent argument validation and in-flight HTTP deadlines. Windows terminal smoke checks cover real Ctrl-C. Remaining run-specific fault-injection tests are tracked separately.")
        count += 1
assert count == 2, count
path.write_text(json.dumps(data, indent=2)+"\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
