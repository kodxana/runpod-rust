"""Map job inspection/control commands independently of submission and observation."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "job": "cli::endpoint::options::job_command",
    "_job_command": "cli::endpoint::options::job_command",
    "_job_path": "cli::endpoint::jobs::request path assembly",
    "_data_plane_errors": "cli::endpoint::jobs::request and cli::CliError",
    "_job_output": "cli::endpoint::view::job output selection",
    "job_get": "cli::endpoint::jobs::execute / Action::Get",
    "job_cancel": "cli::endpoint::jobs::execute / Action::Cancel",
    "job_retry": "cli::endpoint::jobs::execute / Action::Retry",
    "purge_queue": "cli::endpoint::jobs::execute / Action::Purge",
}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
count = 0
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/endpoint.py" and entry["name"] in mapping:
        entry.update(status="adapted", rust=mapping[entry["name"]],
                     note="Native job get/cancel/retry and queue purge; encoded data-plane paths, typed transport failures, JSON/field output and confirmation before credentials. JSON cell spacing, native transport diagnostics and credential lookup are documented adaptations. Log/completion work remains pending separately. See reviews/endpoint-jobs.md.")
        count += 1
assert count == 9, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))

path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
names = {"test_job_get", "test_job_cancel_and_retry", "test_purge"}
count = 0
for entry in data["entries"]:
    name = entry["name"].split(".")[-1]
    if entry["file"] == "tests/test_cli/test_endpoint_command.py" and name in names:
        entry.update(status="ported", rust="cli_endpoint_jobs::" + name,
                     note="Named native process counterpart using captured unchanged upstream invocations. Compares HTTP path/method/headers/body, exit codes, fields, JSON values and success messages. Five additional groups cover realistic output/error/ID/confirmation edges.")
        count += 1
assert count == 3, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
