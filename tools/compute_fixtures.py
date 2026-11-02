"""Characterize the pinned CLI compute helpers without contacting Runpod."""
import json
from pathlib import Path
import runpy
from unittest.mock import patch

from runpod.rp_cli import compute

ROOT = Path(__file__).resolve().parents[1]
source = runpy.run_path(str(ROOT.parent / "runpod-python/tests/test_cli/test_compute.py"))
gpus, cpus, datacenters = source["GPUS"], source["TestCpus"].CPUS, source["DATACENTERS"]
cases = []


def capture(operation, *args, **kwargs):
    case = {"operation": operation, "args": args, "kwargs": kwargs}
    def request(method, path, **options):
        case["request"] = {"method": method, "path": path, **options}
        return {"gpus": gpus, "cpus": cpus, "dataCenters": datacenters}
    try:
        with patch.object(compute, "run_rest_request", request):
            case["output"] = getattr(compute, operation)(*args, **kwargs)
    except Exception as error:
        case["error"] = str(error)
    cases.append(case)


for query in ["4090", "rtx 4090", "rtx4090", "NVIDIA L4", "l4", "h100 sxm", "ADA_80_PRO",
              "a100-sxm4", "h100", "nvidia", "mi300x", "", "  H100_NVL  ", "L40"]:
    capture("match_gpus", query, gpus)
    capture("resolve_gpus", query, gpus)
for query in ["", "nvidia", "custom"]:
    capture("match_gpus", query, [{"id": "custom"}, {"id": "other", "name": None, "pool": None}])
for value in ["h100:2", " 4090 ", "h100:0", "h100:x", ":2", "", "h100:0002", "h100 :2", "x:y:3", "h100: 2"]:
    capture("parse_gpu_spec", value)
for values in [[], [">=12.4"], ["12.4,12.8", "13.0", ""], ["twelve"], [">=12.4", "12.8"],
               [">=12.4", ">=12.10"], [">=12.10", ">=12.4"], [">=0012.04", ">=12.4"],
               ["12.4", "12.4"], [" , , "], [">= 12.4"], ["12"], ["12.4.1"]]:
    capture("parse_cuda", values)
for query in ["h100 nvl", "h100", "a100-sxm4", "4090", "mi300x"]:
    capture("endpoint_gpu_selection", compute.match_gpus(query, gpus), gpus)
for values in [["eu"], ["Europe"], ["north-america"], ["us"], ["us-tx"], ["eu_ro_1"],
               ["ap", "EU-RO-1", "eu-ro"], ["mars"], [], ["na", "us", "asia"], [""], ["apac"]]:
    capture("resolve_regions", values, datacenters)
for query in ["CPU3", "general purpose", "cpu5c", "cpu9", "Compute Optimized", "", "  cpu3g  "]:
    capture("match_cpus", query, cpus)
    capture("resolve_cpu", query, cpus)
for value in [None, "NONE", "LOW", "MEDIUM", "HIGH", "unknown", "high", ""]:
    capture("availability_rank", value)
for values in [[], ["LOW", None, "HIGH", "MEDIUM"], ["unknown", "NONE"], [None, "unknown"], ["MEDIUM", "MEDIUM"]]:
    capture("best_availability", values)
for kwargs in [{}, {"count": 2, "min_cuda_version": "12.4"},
               {"product": "POD", "count": 2, "cloud": "SECURE", "cuda_versions": ["12.4", "12.8"]},
               {"product": "SERVERLESS", "cuda_versions": [], "min_cuda_version": "12.4"},
               {"product": "", "count": 2}, {"product": "POD", "count": 0, "cloud": ""}]:
    capture("list_gpus", **kwargs)
for kwargs in [{}, {"product": "SERVERLESS", "vcpu_count": 4}, {"vcpu_count": 4}, {"product": ""}]:
    capture("list_cpus", **kwargs)
for include in [None, [], ["STOCK", "PRICES"]]:
    capture("list_datacenters", include)
(ROOT / "tests/fixtures/compute.json").write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} compute cases")
