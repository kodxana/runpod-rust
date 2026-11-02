"""Capture actual catalog command behavior with both network and update checks intercepted."""
import copy
import json
from pathlib import Path
import runpy
from unittest.mock import patch
from click.testing import CliRunner
from runpod.rp_cli.main import cli
from runpod.rp_cli.output import cell
from runpod.error import AuthenticationError

ROOT = Path(__file__).resolve().parents[1]
source = runpy.run_path(str(ROOT.parent / "runpod-python/tests/test_cli/test_catalog_command.py"))
data = {"gpus": source["GPUS"], "cpus": source["CPUS"], "dataCenters": source["DATACENTERS"]}
cases = []


def capture(name, args, records=None, denied=False):
    records = copy.deepcopy(records or data)
    case = {"name": name, "args": args, "records": records, "requests": []}
    def request(method, path, **options):
        case["requests"].append({"method": method, "path": path, **options})
        if denied:
            raise AuthenticationError("Unauthorized request")
        return records
    def table(rows, columns, empty):
        case["table"] = {"headers": [header for header, _ in columns],
                         "cells": [[cell(value(row)) for _, value in columns] for row in rows], "empty": empty}
    with patch("runpod.rp_cli.compute.run_rest_request", request), patch("runpod.rp_cli.catalog.print_table", table), patch("runpod.rp_cli.update.start_background_check"):
        result = CliRunner().invoke(cli, ["catalog", *args])
    case["code"] = result.exit_code
    if denied: case["denied"] = True
    if result.exit_code:
        case["error"] = result.output.split("Error: ")[-1].strip()
    elif "--json" in args:
        case["json"] = json.loads(result.output)
    elif "table" not in case:
        raise AssertionError(f"missing output for {args}: {result.exception!r}")
    cases.append(case)


for name, args in [
    ("test_pod_table_sorted_by_stock_then_price", ["h100", "--count", "2"]),
    ("test_region_narrows_stock", ["h100", "--region", "eu", "--json"]),
    ("test_cuda_and_cloud_filters", ["--cloud", "COMMUNITY", "--cuda", ">=12.4", "--max-price", "1"]),
    ("test_available_and_vram_filters", ["--available", "--min-vram", "90", "--json"]),
    ("test_json_totals", ["h100 sxm", "--count", "2", "--json"]),
    ("test_serverless_shows_pools_only", ["--serverless"]),
    ("test_unknown_gpu", ["b9000"]), ("test_empty", ["--max-price", "0.01"]),
    ("test_cpu_community_rejected_before_fetch", ["--cpu", "--cloud", "community"]),
    ("test_table", ["--cpu", "--vcpu", "8", "--serverless"]),
    ("test_query_region_and_available", ["cpu3", "--cpu", "--region", "us", "--available", "--json"]),
    ("test_unknown", ["cpu9", "--cpu"]), ("test_all", ["--datacenters"]),
    ("test_query_is_a_region", ["--datacenters", "eu", "--json"]),
    ("test_conflicting_flags", ["--cpu", "--datacenters"]),
    ("test_conflicting_flags", ["--serverless", "--cloud", "secure"]),
]: capture(name, args)
capture("test_api_error_is_clean", [], denied=True)
for cloud in [None, "secure", "community"]:
    capture("test_max_count_matches_cloud", [] if cloud is None else ["--cloud", cloud])
for floors in [("12.4", "12.10"), ("12.10", "12.4")]:
    capture("test_strongest_cuda_floor_is_sent", ["--cuda", ">=" + floors[0], "--cuda", ">=" + floors[1]])
no_stock = copy.deepcopy(data)
no_stock["gpus"] = [{k: v for k, v in data["gpus"][0].items() if k not in ("dataCenters", "availability")}]
capture("test_unavailable_gpu_omits_stock_fields", ["--region", "eu", "--available", "--json"], no_stock)
for product, tier in [("pod", "securePerVcpu"), ("serverless", "serverlessPerVcpu")]:
    for price in [0.036, 0.004, 0.0001234, 0.012345, 0, None, 0.00001, 12345.6, 9999.9]:
        records = copy.deepcopy(data)
        records["cpus"] = [dict(data["cpus"][0], price={tier: price})]
        for extra in [[], ["--json"]]:
            capture("test_per_vcpu_price_precision", ["--cpu", "--" + product, *extra], records)
invalid = [
    (["--cpu"], ["--max-price", "0.02"]), (["--cpu"], ["--max-price", "0"]),
    (["--cpu"], ["--min-vram", "0"]), (["--cpu"], ["--cuda", "12.4"]),
    (["--cpu"], ["--count", "1"]), (["--cpu", "--serverless"], ["--count", "2"]),
    *[(["--datacenters"], flag) for flag in [["--available"], ["--serverless"], ["--pod"], ["--cloud", "secure"], ["--cuda", "12.4"], ["--max-price", "0"], ["--min-vram", "0"], ["--count", "1"], ["--vcpu", "8"]]],
    ([], ["--vcpu", "8"]), (["--serverless"], ["--vcpu", "8"]),
]
for mode, flags in invalid:
    for extra in [[], ["--json"]]: capture("test_inapplicable_flags_fail_before_fetch", [*mode, *flags, *extra])
for args in [["--serverless", "--pod"], ["--pod", "--serverless"], ["--count", "1", "--count", "2"],
             ["--region", "eu", "--region", "us", "--json"], ["--cuda", "wrong"], ["--region", "mars"],
             ["--min-vram", "-1", "--json"], ["", "--json"]]:
    capture("additional_cli_edges", args)
for args in [["--max-price", "-1", "--json"], ["--max-price", "nan", "--json"],
             ["--max-price", "inf", "--json"], ["--datacenters", "--pod", "--serverless"],
             ["--datacenters", "--serverless", "--pod"]]:
    capture("additional_cli_edges", args)
for price in [0.00005, 0.00015, 1.23445, -0.00001]:
    records = copy.deepcopy(data)
    records["gpus"][0]["price"]["futureTier"] = price
    capture("additional_cli_edges", ["--count", "3", "--json"], records)
records = copy.deepcopy(data)
records["gpus"][0]["memory"] = 24.5
for min_vram in ["24", "25"]:
    capture("additional_cli_edges", ["--min-vram", min_vram, "--json"], records)
records = copy.deepcopy(data)
records["gpus"][0]["name"] = "GPU [red] 雪 🌍"
records["gpus"][0]["future"] = {"label": "café"}
for args in [[], ["--json"]]:
    capture("additional_cli_edges", args, records)
for mode in [[], ["--cpu"], ["--datacenters"]]:
    for extra in [[], ["--json"]]:
        capture("additional_cli_edges", mode + extra, {"gpus": [], "cpus": [], "dataCenters": []})
(ROOT / "tests/fixtures/catalog.json").write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} catalog command cases")
