"""Map endpoint resource commands and observation helpers independently of other CLI work."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "_path":"cli::endpoint::path", "endpoint_argument":"cli::endpoint::options::id",
    "_pick_endpoint":"cli::endpoint::pick", "_type":"cli::endpoint::view::kind",
    "_flashboot":"cli::endpoint::view::flashboot", "_compute":"cli::endpoint::view::compute",
    "_cuda":"cli::endpoint::view::cuda", "_scaling":"cli::endpoint::view::scaling",
    "_workers":"cli::endpoint::view::workers", "_seconds":"cli::endpoint::view::seconds",
    "_volumes":"cli::endpoint::view::volumes", "_url":"cli::endpoint::view::detail URL selection",
    "_scale_options":"cli::endpoint::options::scale", "_config_options":"cli::endpoint::options::config",
    "_rollout_line":"cli::endpoint::view::rollout", "_follow_rollout":"cli::endpoint::watch::follow",
    "_apply":"cli::endpoint::Options::apply", "_edit_document":"cli::endpoint::edit_document",
    "_uptime":"cli::endpoint::view::uptime", "_worker_version":"cli::endpoint::view::worker_version",
    "_workers_view":"cli::endpoint::view::worker_view", "endpoint":"cli::endpoint::command",
    **{name:"cli::endpoint::Options::execute_with_clock / "+command for name,command in [
        ("list_endpoints","list"),("get_endpoint","get"),("create_endpoint","create"),("update_endpoint","update"),
        ("edit_endpoint","edit"),("scale_endpoint","scale"),("delete_endpoint","delete"),("list_releases","releases"),
    ]},
    "list_workers":"cli::endpoint::watch::workers", "_release_changes":"cli::endpoint::view::releases diff selection",
}
path=ROOT / "parity/api.json"
data=json.loads(path.read_text())
count=0
for entry in data["entries"]:
    if entry["file"]=="runpod/rp_cli/endpoint.py" and entry["name"] in mapping:
        entry.update(rust=mapping[entry["name"]],status="adapted",
                     note="Native resource CLI, typed options, owned editor files, incremental stdout/stderr, interruptible rollout/worker observation and native terminal rendering. Shell completion is tracked separately; log workflows are covered by cli::logs. See reviews/endpoint-cli.md.")
        count+=1
assert count==32,count
path.write_text(json.dumps(data,indent=2)+"\n")
print("API",dict(Counter(e["status"] for e in data["entries"])))

cases=json.loads((ROOT / "tests/fixtures/endpoint_cli.json").read_text())
names={c["name"] for c in cases if c["name"].startswith("test_")} | {"test_workers_empty_and_uptime"}
interactive={"test_get_without_id_picks_interactively","test_edit_in_editor_reopens_on_error_then_follows",
             "test_edit_in_editor_gives_up_or_saves_nothing","test_delete_confirms","test_workers_watch"}
adapted={"test_api_errors_are_clean","test_release_baseline_failure_does_not_block_update"}
path=ROOT / "parity/tests.json"
data=json.loads(path.read_text())
count=0
for entry in data["entries"]:
    name=entry["name"].split(".")[-1]
    if entry["file"]=="tests/test_cli/test_endpoint_command.py" and name in names:
        prefix="cli::endpoint::view::tests::" if name=="test_workers_empty_and_uptime" else "cli::endpoint::tests::" if name in interactive or name.startswith("test_follow_rollout_") else "cli_endpoint::"
        entry.update(rust=prefix+name,status="adapted" if name in adapted else "ported",
                     note="Named native counterpart backed by pinned Python command/helper fixtures. API exception injection uses equivalent HTTP response fixtures; an additional real transport-timeout test verifies update-after-baseline-failure. Interactive and clock behavior uses native fakes; editor subprocess behavior has a real child-process test.")
        count+=1
assert count==42,count
path.write_text(json.dumps(data,indent=2)+"\n")
print("Tests",dict(Counter(e["status"] for e in data["entries"])))
