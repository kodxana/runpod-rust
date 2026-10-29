"""Update only reviewed mappings; leave all other entries pending."""
import json
import pathlib

root = pathlib.Path(__file__).resolve().parents[1]
path = root / "parity/api.json"
api = json.loads(path.read_text())
for entry in api["entries"]:
    file, name = entry["file"], entry["name"]
    target = None
    if file == "runpod/api/ctl_commands.py" and entry["public"]:
        target = f"Client::{name}"
    if file == "runpod/api/rest.py" and entry["public"]:
        target = "Client::" + name.removesuffix("_async")
    if file == "runpod/endpoint/helpers.py" and name == "is_completed":
        target = "endpoint::is_completed"
    if file in ("runpod/endpoint/runner.py", "runpod/endpoint/asyncio/asyncio_runner.py"):
        prefix = "Asyncio" if "/asyncio/" in file else ""
        if name in ("Endpoint", "Job") or name.startswith(("Endpoint.", "Job.")):
            parts = name.split(".")
            if len(parts) == 1 or parts[1] in ("__init__", "run", "run_sync", "health", "purge_queue", "status", "output", "stream", "cancel"):
                target = prefix + parts[0] + ("::" + ("new" if parts[1] == "__init__" else parts[1]) if len(parts) > 1 else "")
    if file == "runpod/error.py" and name in ("RunPodError", "AuthenticationError", "QueryError"):
        target = {"RunPodError":"Error", "AuthenticationError":"Error::Authentication", "QueryError":"Error::Query"}[name]
    if target:
        entry.update(rust=target, status="implemented")
path.write_text(json.dumps(api, indent=2) + "\n")

path = root / "parity/tests.json"
tests = json.loads(path.read_text())
mapping = {
    "test_authenticated_mutation_at_custom_base_url":"transport::test_authenticated_mutation_at_custom_base_url",
    "test_missing_key_fails_before_network_request":"transport::test_missing_key_fails_before_network_request",
    "test_invalid_key_raises_authentication_error":"transport::test_invalid_key_raises_authentication_error",
    "test_empty_response_returns_none":"transport::test_empty_response_returns_none",
    "test_problem_json_preserves_validation_error":"transport::test_problem_json_preserves_validation_error",
    "test_error_body_fallbacks_do_not_mask_http_failure":"transport::test_error_body_fallbacks_do_not_mask_http_failure",
}
fixtures = json.loads((root / "tests/fixtures/control.json").read_text())
for entry in tests["entries"]:
    target = None
    if entry["file"] == "tests/test_api/test_rest.py":
        target = mapping.get(entry["name"])
    if entry["file"] == "tests/test_api/test_ctl_commands.py":
        matching = [case["name"] for case in fixtures if case["name"] == entry["name"]]
        if matching:
            target = "control::python_control_plane_fixtures [" + ", ".join(matching) + "]"
    if entry["file"] == "tests/test_endpoint/test_runner.py":
        leaf = entry["name"].split(".")[-1]
        if leaf in ("test_endpoint_run", "test_endpoint_run_sync", "test_run_sync_with_timeout", "test_job_status", "test_output_with_sleep", "test_job_stream"):
            target = "endpoint::" + leaf
    if target:
        entry.update(rust=target, status="ported")
path.write_text(json.dumps(tests, indent=2) + "\n")
print(f"Mapped {sum(e['status']=='implemented' for e in api['entries'])} symbols and {sum(e['status']=='ported' for e in tests['entries'])} tests; remaining entries retained as pending")
