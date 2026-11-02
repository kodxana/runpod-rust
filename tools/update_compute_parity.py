"""Update reviewed compute-module mappings without touching unrelated inventory."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    "ResolutionError": "compute::ResolutionError / Error::Compute",
    "_query_params": "compute::product_params / Client::list_catalog_*",
    "list_gpus": "Client::list_catalog_gpus / blocking::Client::list_catalog_gpus",
    "list_cpus": "Client::list_catalog_cpus / blocking::Client::list_catalog_cpus",
    "list_datacenters": "Client::list_catalog_datacenters / blocking::Client::list_catalog_datacenters",
    "_tokens": "compute::tokens", "_compact": "compute::compact",
}
adapted = set(mapping) | {"parse_gpu_spec", "parse_cuda"}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/compute.py":
        name = entry["name"]
        entry.update(rust=mapping.get(name, f"compute::{name}"), status="adapted" if name in adapted else "implemented",
                     note="Owned clients and typed errors replace globals/exceptions; JSON catalog fields remain intact. ASCII counts/version components are checked u64 values. Native Unicode whitespace follows Rust. See reviews/compute.md and PORTING.md.")
path.write_text(json.dumps(data, indent=2) + "\n")
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] == "tests/test_cli/test_compute.py":
        entry.update(rust="compute::" + entry["name"].split(".")[-1], status="ported",
                     note="Named Rust counterpart using pinned Python differential fixtures; catalog request tests compare complete query maps and raw response records.")
path.write_text(json.dumps(data, indent=2) + "\n")
