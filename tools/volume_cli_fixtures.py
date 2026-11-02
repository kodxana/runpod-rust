"""Capture native volume CLI contracts from the pinned Python commands, offline."""
import copy
import json
from pathlib import Path
import runpy
from unittest.mock import patch
from contextlib import ExitStack

import click
from click.testing import CliRunner
from rich.text import Text
from runpod.rp_cli.main import cli
from runpod.rp_cli import output
from runpod.api.rest import _raise_for_error

ROOT = Path(__file__).resolve().parents[1]
source = runpy.run_path(str(ROOT.parent / "runpod-python/tests/test_cli/test_volume_command.py"))
NETWORK, GLOBAL, CENTERS = (source[key] for key in ("VOLUME", "GLOBAL_VOLUME", "DATACENTERS"))
cases = []


def capture(name, args, *, record=None, listing=None, selected=None, answer=None, failure=None, pages=None):
    base = GLOBAL if "--global" in args else NETWORK
    record = copy.deepcopy(base if record is None else record)
    listing = copy.deepcopy([base] if listing is None else listing)
    case = {"name": name, "args": args, "requests": [], "picks": [], "confirms": [], "success": []}
    if selected is not None: case["selected"] = selected
    if answer is not None: case["answer"] = answer
    page_index = 0
    def request(method, path, **options):
        nonlocal page_index
        item = {"method": method, "path": path, "params": copy.deepcopy(options.get("params", {})), "body": options.get("json"), "status": 200}
        case["requests"].append(item)
        if failure:
            item["status"], message = failure
            item["response"] = {"detail": message}
            _raise_for_error(item["status"], method, path, json.dumps(item["response"]))
        if path == "/v2/catalog/datacenters":
            result = {"dataCenters": CENTERS}
        elif path in ("/v2/network-volumes", "/v2/global-volumes") and method == "GET":
            key = "globalVolumes" if "global" in path else "networkVolumes"
            if pages is not None:
                result = pages[page_index]
                page_index += 1
            else:
                result = {key: listing, "pagination": {"hasNextPage": False, "nextCursor": None}}
        elif method == "GET": result = base
        else: result = record
        item["response"] = copy.deepcopy(result)
        return copy.deepcopy(result)
    def table(rows, columns, empty):
        case["table"] = {"headers": [h for h, _ in columns], "cells": [[output.cell(f(row)) for _, f in columns] for row in rows], "empty": empty}
    def fields(row, columns):
        case["fields"] = [[h, output.cell(f(row))] for h, f in columns]
    def picker(message, choices, missing):
        case["picks"].append({"message": message, "choices": choices, "missing": missing})
        if selected is not None: return selected
        return output.pick(message, choices, missing)
    def confirm(message, yes):
        case["confirms"].append(message)
        return output.confirm(message, yes)
    with ExitStack() as stack:
        for target in ["runpod.rp_cli.volume.run_rest_request", "runpod.rp_cli.api.run_rest_request", "runpod.rp_cli.compute.run_rest_request"]:
            stack.enter_context(patch(target, request))
        for target, replacement in [("volume.print_table", table), ("volume.print_fields", fields),
                                    ("volume.pick", picker), ("volume.confirm", confirm),
                                    ("volume.ui.success", lambda msg: case["success"].append(Text.from_markup(msg).plain))]:
            stack.enter_context(patch("runpod.rp_cli." + target, replacement))
        stack.enter_context(patch("runpod.rp_cli.output.interactive", return_value=answer is not None))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result = CliRunner().invoke(cli, ["volume", *args], input=(answer + "\n") if answer else None)
    case["code"] = result.exit_code
    if result.exit_code:
        if isinstance(result.exception, SystemExit):
            case["error"] = "Aborted!" if "Aborted!" in result.output else result.output.split("Error: ")[-1].strip()
        else: raise AssertionError((args, result.exception, result.output))
    elif "--json" in args: case["json"] = json.loads(result.output)
    cases.append(case)


for kind, base, flags in [("network", NETWORK, []), ("global", GLOBAL, ["--global"])]:
    for extra in [[], ["--json"]]:
        capture("test_list_table_and_json", ["list", *flags, *extra])
        capture("test_get_fields_and_json", ["get", base["id"], *flags, *extra])
    capture("test_rename_without_fetching", ["update", base["id"], *flags, "--name", "renamed"])
    capture("test_delete_yes", ["delete", base["id"], *flags, "--yes"])
capture("test_list_empty", ["list"], listing=[])
capture("test_get_picker", ["get"], selected="vol1")
capture("test_missing_id_requires_terminal", ["get"])
for args, failure in [(["get", "vol1"], (404, "not found")), (["get", "vol1"], (401, "bad key")),
                      (["delete", "vol1", "--global", "--yes"], (409, "still mounted"))]:
    capture("test_api_errors_are_clean", args, failure=failure)
capture("test_create_exact_data_center", ["create", "--name", "models", "--size", "100", "--data-center", "EU-RO-1", "--type", "standard"])
for args, selected in [(["--region", "eu", "--type", "high_performance"], "EU-FR-1"),
                       (["--type", "standard"], "EU-RO-1"), (["--region", "eu", "--region", "na"], "EU-RO-1")]:
    capture("test_create_filters_picker_candidates", ["create", "--name", "m", "--size", "10", *args, "--json"], selected=selected)
capture("test_create_single_region_match_uses_default_tier", ["create", "--name", "m", "--size", "10", "--region", "na"])
for args in [["--region", "eu"], ["--region", "na", "--type", "high_performance"], ["--region", "mars"],
             ["--region", "eu", "--data-center", "EU-RO-1"], ["--size", "5000"]]:
    capture("test_create_rejects_invalid_placement_and_size", ["create", "--name", "m", "--size", "10", *args])
capture("test_create_requires_network_size", ["create", "--name", "m", "--data-center", "EU-RO-1"])
capture("test_create_global", ["create", "--name", "shared", "--global"])
for args in [["create", "--name", "s", "--global", "--size", "10", "--region", "eu"],
             ["update", "vol1", "--global", "--size", "20"], ["update", "vol1"]]:
    capture("test_invalid_updates_and_global_options", args)
for args, target in [(["update", "vol1", "--size", "200", "--name", "renamed"], 200),
                     (["update", "vol1", "--size", "100"], 100), (["expand", "vol1", "--size", "250"], 250),
                     (["expand", "vol1", "--size", "+50"], 150)]:
    capture("test_resize_requests", [*args, "--json"], record=dict(NETWORK, size=target))
capture("test_expand_human_output", ["expand", "vol1", "--size", "+1"], record=dict(NETWORK, size=101))
for args in [["update", "vol1", "--size", "50"], ["expand", "vol1", "--size", "50"],
             ["expand", "vol1", "--size", "+0"], ["expand", "vol1", "--size", "+4000"]]:
    capture("test_resize_rejects_shrink_no_growth_and_overflow", args)
for size in ["abc", "-5", "1.5"]:
    capture("test_expand_rejects_malformed_size_without_fetching", ["expand", "vol1", "--size", size])
for answer in ["y", "n"]:
    capture("test_delete_confirmation", ["delete", "vol1"], answer=answer)
capture("test_delete_requires_yes_without_terminal", ["delete", "vol1"])
for args in [["create", "--name", "", "--global", "--json"], ["update", "vol1", "--name", "", "--json"],
             ["create", "--name", "m", "--size", "10", "--region", "eu-se"],
             ["create", "--name", "m", "--size", "10", "--data-center", ""],
             ["expand", "vol1", "--size", " +0001 ", "--json"], ["expand", "vol1", "--size", "4097"],
             ["expand", "vol1", "--size", "+"], ["delete", "vol1", "-y"]]:
    capture("additional_volume_edges", args)
capture("additional_volume_edges", ["list", "--json"], pages=[
    {"networkVolumes": [NETWORK], "pagination": {"hasNextPage": True, "nextCursor": "page-2"}},
    {"networkVolumes": [dict(NETWORK, id="vol2")], "pagination": {"hasNextPage": False, "nextCursor": None}},
])
capture("additional_volume_edges", ["list", "--global"], listing=[{k: v for k, v in GLOBAL.items() if k != "createdByUserId"}])
(ROOT / "tests/fixtures/volume_cli.json").write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} volume command cases")
