"""Map reviewed configuration helpers without claiming endpoint command integration."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "runpod/rp_cli/options.py": {
        "parse_env": "configuration::parse_env",
        "container_fields": "Client::container_fields / blocking::Client::container_fields",
    },
    "runpod/rp_cli/endpoint_config.py": {
        "_gpu": "configuration::gpu_config",
        "_cpu": "configuration::cpu_config",
        "scale_patch": "configuration::scale_patch",
        "config_body": "Client::{create_endpoint_config,update_endpoint_config} / blocking::Client",
        "mutable": "configuration::mutable_endpoint",
        "_kind": "configuration::validate_endpoint_document type labels",
        "validate": "configuration::validate_endpoint_document",
        "patch": "configuration::endpoint_patch",
    },
}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
changed = 0
for entry in data["entries"]:
    target = mapping.get(entry["file"], {}).get(entry["name"])
    if target:
        entry.update(rust=target, status="implemented" if entry["name"] == "parse_env" else "adapted",
                     note="Owned async/blocking clients and typed options/errors replace globals and option dictionaries. Builders only resolve catalogs/registries and return payloads. Numeric bounds and native JSON diagnostics are documented in reviews/configuration.md. Endpoint CLI integration remains pending.")
        changed += 1
assert changed == 10, changed
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))

endpoint_helpers = {
    "test_mutable_document_holds_update_fields_only",
    "test_partial_gpu_edit_preserves_omitted_exclusions",
    "test_edit_clears_cuda_floor_with_empty_string",
    "test_edit_validation",
    "test_edit_rejects_switching_cpu_to_gpu",
}
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
changed = 0
for entry in data["entries"]:
    name = entry["name"].split(".")[-1]
    if entry["file"] == "tests/test_cli/test_cli_options.py" or (
        entry["file"] == "tests/test_cli/test_endpoint_command.py" and name in endpoint_helpers
    ):
        entry.update(rust=f"configuration::{name}", status="adapted" if name == "test_edit_validation" else "ported",
                     note="Named native counterpart with actual pinned Python JSON fixtures and complete request comparison. Only invalid-JSON parser diagnostic text is native. Command-level endpoint tests remain pending.")
        changed += 1
assert changed == 15, changed
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
