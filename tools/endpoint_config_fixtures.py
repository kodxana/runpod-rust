"""Capture endpoint configuration and shared option behavior from pinned Python."""
import copy
import json
from pathlib import Path
import runpy
from unittest.mock import patch

import click
from runpod.rp_cli import endpoint_config as config, options

ROOT = Path(__file__).resolve().parents[1]
source = runpy.run_path(str(ROOT.parent / "runpod-python/tests/test_cli/test_endpoint_command.py"))
GPU, CPU, LB = (source[name] for name in ("GPU_ENDPOINT", "CPU_ENDPOINT", "LB_ENDPOINT"))
records = {"gpus": source["GPUS"], "cpus": source["CPUS"], "dataCenters": source["DATACENTERS"],
           "registries": [{"id": "reg1", "name": "private"}], "pagination": {"hasNextPage": False, "nextCursor": None}}
cases = []


def capture(name, operation, *args, registry_records=None):
    case = {"name": name, "operation": operation, "args": copy.deepcopy(args), "requests": []}
    def request(method, path, **kwargs):
        response = copy.deepcopy(records)
        if registry_records is not None: response["registries"] = registry_records
        case["requests"].append({"method": method, "path": path, "params": copy.deepcopy(kwargs.get("params", {})), "response": response})
        return response
    with patch("runpod.rp_cli.compute.run_rest_request", request), patch("runpod.rp_cli.api.run_rest_request", request):
        try:
            module = options if operation in ("parse_env", "container_fields") else config
            case["result"] = getattr(module, operation)(*copy.deepcopy(args))
        except click.ClickException as error:
            case["error"] = error.format_message()
            case["code"] = error.exit_code
        except ValueError as error:
            case["error"] = str(error)
            case["code"] = 1
    cases.append(case)


capture("test_env_values_preserve_equals_and_allow_empty_values", "parse_env", ["TOKEN=a=b", "EMPTY=", "TOKEN=c=d"])
for value in ["secret-without-equals", "=secret"]:
    capture("test_env_error_does_not_echo_the_value", "parse_env", [value])
for opts in [{}, {"env": [], "ports": [], "registry": None}]:
    capture("test_container_fields_preserve_omitted_values", "container_fields", opts)
capture("test_container_fields_map_options_and_parse_commands", "container_fields", {"image": "image:tag", "template": "tpl1", "args": "bash start.sh", "disk": 0, "global_networking": False, "cmd": "python -c 'print(1)'", "entrypoint": "", "ports": ["8000/http", "22/tcp"]})
for key in ["cmd", "entrypoint"]:
    capture("test_container_fields_reject_malformed_shell_quoting", "container_fields", {key: "python 'unterminated"})
for registry in ["reg1", "private"]:
    capture("test_container_fields_resolve_registry_id_or_name", "container_fields", {"registry": registry})
capture("test_container_fields_clear_registry_without_lookup", "container_fields", {"registry": ""})
capture("test_container_fields_reject_unknown_registry", "container_fields", {"registry": "missing"}, registry_records=[])
capture("test_container_fields_merge_then_unset_environment", "container_fields", {"env": ["EDIT=new", "DROP=new", "EMPTY="]}, {"env": {"KEEP": "old", "EDIT": "old", "DROP": "old"}}, ["DROP", "MISSING"])
capture("test_container_fields_merge_then_unset_environment", "container_fields", {}, {"env": {"DROP": "old"}}, ["DROP"])
for current in [None, {}, {"env": None}]:
    capture("test_container_fields_allow_empty_current_environment", "container_fields", {"env": ["KEY=value"]}, current)
for command in ["echo # literal", "echo a\rb", "a\\\nb", 'echo "\\$x \\`x \\q"', "'' a\"b\"c 'x y'", "echo trailing\\", '"unterminated', "echo \"a\\\"b\"", "echo a\\ b"]:
    capture("command_quoting_edges", "container_fields", {"cmd": command})

for endpoint in [GPU, CPU, LB, {}, {"gpu": {"count": 0}, "mounts": {"network": [{"volumeId":"v", "extra":1}]}, "env": None}]:
    capture("test_mutable_document_holds_update_fields_only", "mutable", endpoint)
for excluded in [None, []]:
    edited = {"pools": ["ADA_24"]}
    if excluded is not None: edited["excludedTypes"] = excluded
    capture("test_partial_gpu_edit_preserves_omitted_exclusions", "patch", {"gpu": {"pools": ["ADA_80_PRO"], "excludedTypes": ["excluded"]}}, {"gpu": edited})
before = config.mutable(LB)
after = copy.deepcopy(before)
after["gpu"].update(minCudaVersion=None, count=1)
capture("test_edit_clears_cuda_floor_with_empty_string", "patch", before, after)
for text in ["{not json", "[]", *[json.dumps(value) for value in [
    {"bogus": 1}, {"disk": "big"}, {"disk": True}, {"registry": 1}, {"workers": {"maximum": 1}},
    {"env": {"A": 1}}, {"flashboot": "ON"}, {"cpu": [{"id":"cpu3c", "vcpuCount":2}]},
    {"gpu": {"unknown": 1}, "mounts": {"extra": []}, "timeout": False},
]]]: capture("test_edit_validation", "validate", text, config.mutable(GPU))
capture("test_edit_rejects_switching_cpu_to_gpu", "validate", '{"gpu": {}}', config.mutable(CPU))
for doc in [{}, {"image": "i", "env": {}, "registry": None, "gpu": {"minCudaVersion": None}}, config.mutable(GPU)]:
    capture("valid_edit_documents", "validate", json.dumps(doc), config.mutable(GPU))
for after in [{}, {"registry": None}, {"gpu": {"excludedTypes": []}}, {"gpu": {"count": 3}}, {"gpu": {}}, {"env": {}}]:
    capture("patch_edges", "patch", config.mutable(GPU), after)
for before, after in [
    ({"scaling":{"queueDelay":4.0}}, {"scaling":{"queueDelay":4}}),
    ({"gpu":{"count":1}}, {"gpu":{"count":True}}),
    ({"timeout":9007199254740993}, {"timeout":9007199254740992.0}),
    ({"timeout":18446744073709551615}, {"timeout":18446744073709551616.0}),
]: capture("patch_edges", "patch", before, after)

base = {"endpoint_type":"QUEUE", "name":"x", "image":"i"}
for extra in [
    {"gpu":["H100"], "gpu_count":1, "min_workers":0, "max_workers":4, "queue_delay_seconds":4.0,
     "idle_timeout_seconds":10, "request_timeout_seconds":300, "data_center":["EU-RO-1"], "network_volume":["vol1"], "env":["MODEL_ID=m"], "flashboot":"standard"},
    {"endpoint_type":"LOAD_BALANCER", "gpu":["h100", "4090"], "requests_per_worker":4, "env":["PORT=8000"]},
    {"cpu":["cpu3c"], "vcpu_count":8, "max_workers":3, "queue_delay_seconds":2.0},
    {"gpu":["h100:2"], "cuda":[">=12.4"], "region":["eu"], "global_volume":["gv1"], "port":["8000/http"], "disk":20, "template":"tpl1", "args":"python main.py", "entrypoint":"python -m 'my app'", "cmd":"--port 1", "registry":"reg1"},
    {"gpu":["4090"], "cuda":["12.4,12.8"]}, {"cpu":["cpu5g:4"]}, {}, {"cuda":["any"]},
]: capture("configuration_create", "config_body", {**base, **extra})
for extra in [{"gpu":["h100"], "cpu":["cpu3c"]}, {"gpu":["h100:2"], "gpu_count":1}, {"endpoint_type":"LOAD_BALANCER"},
    {"endpoint_type":"LOAD_BALANCER", "queue_delay_seconds":2.0}, {"queue_delay_seconds":2.0, "requests_per_worker":2},
    {"env":["NOVALUE"]}, {"cpu":["cpu3c"]}, {"cpu":["cpu3c:x"]}, {"vcpu_count":4}, {"cpu":["cpu3c:4"], "cuda":["12.4"]},
    {"gpu":["b200"]}, {"region":["mars"]}, {"cpu":["cpu3c:0"]}, {"gpu":["h100", "h100"]}, {"gpu_count":0}]:
    capture("configuration_create_errors", "config_body", {**base, **extra})
for opts, current in [
    ({"env":["C=3"], "unset_env":["A"], "max_workers":8}, GPU), ({"gpu":["4090"], "cuda":["any"]}, GPU),
    ({"cuda":[">=12.8"]}, GPU), ({"cuda":["12.8"]}, GPU), ({"vcpu_count":8, "network_volume":["vol9"]}, CPU),
    ({"cpu":["cpu3c:4"]}, GPU), ({"gpu":["h100"]}, CPU), ({"cuda":["12.4"]}, CPU), ({"gpu_count":1}, CPU),
    ({"queue_delay_seconds":3.0}, LB), ({}, GPU), ({"unset_env":["MISSING"]}, GPU),
    ({"env":["A=1"]}, dict(GPU, env=None)), ({"max_workers":2}, dict(GPU, workers=None)), ({"network_volume":["vol1"]}, dict(GPU, mounts=None)),
]: capture("configuration_update", "config_body", opts, current)
for opts, current in [({"min_workers":1, "requests_per_worker":3}, GPU), ({}, GPU), ({"queue_delay_seconds":2.0}, LB),
    ({"queue_delay_seconds":2.0, "requests_per_worker":3}, GPU), ({"min_workers":0,"max_workers":0,"idle_timeout_seconds":1}, {"workers":None})]:
    capture("scale_changes", "scale_patch", opts, current)
(ROOT / "tests/fixtures/endpoint_config.json").write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} endpoint configuration cases")
