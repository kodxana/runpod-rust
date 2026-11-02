"""Record native endpoint submission and load-balancer command coverage."""
import json
from collections import Counter
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
mapping={"_request_body":"cli::endpoint::run::RunOptions::read",
    "_call_load_balancer":"cli::endpoint::run::RunOptions::call_load_balancer",
    "run_endpoint":"cli::endpoint::run::RunOptions::execute"}
path=ROOT/"parity/api.json"
data=json.loads(path.read_text())
count=0
for entry in data["entries"]:
    if entry["file"]=="runpod/rp_cli/endpoint.py" and entry["name"] in mapping:
        entry.update(rust=mapping[entry["name"]],status="adapted",
            note="Native queue submission/async/wait/stream and load-balancer calls with file/stdin/inline JSON, ordered validation, policy merge and raw/JSON output. Owned client credentials, UTF-8 files, u64 checked durations, object-shaped policies, native HTTP deadlines and no implicit redirects/replay are documented adaptations. See reviews/endpoint-run.md.")
        count+=1
assert count==3,count
path.write_text(json.dumps(data,indent=2)+"\n")
print("API",dict(Counter(e["status"] for e in data["entries"])))
path=ROOT/"parity/tests.json"
data=json.loads(path.read_text())
names={c["name"] for c in json.loads((ROOT/"tests/fixtures/endpoint_run.json").read_text()) if c["name"].startswith("test_")}
count=0
for entry in data["entries"]:
    name=entry["name"].split(".")[-1]
    if entry["file"]=="tests/test_cli/test_endpoint_command.py" and name in names:
        entry.update(rust="cli_endpoint_run::"+name,status="ported",
            note="Named native process counterpart using unchanged source tests and captured request/response sequences. Loopback tests preserve payloads, routing, credentials, headers, output and errors. Separate URL tests verify original/default load-balancer bases; process mocks substitute only the HTTP origin. Additional timing/fault-injection source tests remain pending separately.")
        count+=1
assert count==11,count
path.write_text(json.dumps(data,indent=2)+"\n")
print("Tests",dict(Counter(e["status"] for e in data["entries"])))
