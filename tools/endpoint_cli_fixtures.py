"""Run pinned endpoint command tests and capture their actual CLI contracts offline."""
import copy
import json
from pathlib import Path
import tempfile
from contextlib import ExitStack
from unittest.mock import patch

import pytest
import requests
from click.testing import CliRunner
from runpod.rp_cli.main import cli
from rich.text import Text
from runpod import error
from runpod.rp_cli import endpoint as ep, compute, api, output

ROOT = Path(__file__).resolve().parents[1]
CASES = []
SELECTED = {
    "test_list_shows_ids_and_summaries", "test_list_filters_by_name_glob_and_type",
    "test_get_shows_fields", "test_get_without_id_needs_a_terminal", "test_get_without_id_picks_interactively",
    "test_api_errors_are_clean", "test_create_queue_endpoint_from_ticket", "test_create_load_balancer_endpoint_from_ticket",
    "test_create_cpu_endpoint_from_ticket", "test_create_maps_container_and_placement_flags", "test_create_exact_cuda_and_cpu_vcpu_suffix",
    "test_create_rejects_bad_flags", "test_update_merges_env_and_workers", "test_update_gpu_replaces_pools_and_exclusions_together",
    "test_update_cpu_vcpus_and_mounts", "test_update_rejects_bad_flags", "test_update_json_and_wait_follows_rollout",
    "test_wait_json_keeps_rollout_messages_on_stderr", "test_release_baseline_failure_does_not_block_update",
    "test_registry_selection_and_clearing", "test_unset_missing_environment_variable_is_noop",
    "test_invalid_container_quoting_is_usage_error", "test_nullable_configuration_maps",
    "test_edit_file_shows_patch_and_applies", "test_runtime_only_changes_skip_rollout", "test_mixed_edit_waits_for_delayed_release",
    "test_edit_without_changes", "test_edit_requires_yes_without_a_terminal", "test_edit_needs_terminal_or_file",
    "test_edit_in_editor_reopens_on_error_then_follows", "test_edit_in_editor_gives_up_or_saves_nothing",
    "test_scale_sends_workers_and_scaler", "test_scale_rejects_nothing_and_lb_queue_delay", "test_delete_confirms",
    "test_workers_table_and_json", "test_workers_watch", "test_releases",
}


class Capture:
    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.originalname in SELECTED]

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item):
        self.source = item.module
        patcher = pytest.MonkeyPatch()
        invoke = item.module.invoke
        def wrapped(*args, input=None):
            case = {"name": item.originalname, "args": list(args), "input": input,
                    "requests": [], "fields": [], "tables": [], "success": [], "picks": [], "edits": [], "sleeps": []}
            if "--file" in args:
                index = args.index("--file") + 1
                case["document"] = Path(args[index]).read_text()
                case["args"][index] = "{document}"
            active = pytest.MonkeyPatch()
            request = ep.run_rest_request
            def record(method, path, **kwargs):
                call = {"method":method, "path":path, "params":{str(k): str(v) for k,v in kwargs.get("params", {}).items()},
                        "body":copy.deepcopy(kwargs.get("json")), "status":200}
                case["requests"].append(call)
                try:
                    value = request(method, path, **kwargs)
                except (error.QueryError, error.AuthenticationError, requests.RequestException) as exc:
                    call.update(status=400, response={"error":str(exc)}, failure=type(exc).__name__)
                    raise
                call["response"] = copy.deepcopy(value)
                return value
            for module in [ep, compute, api]: active.setattr(module, "run_rest_request", record)
            fields, table, success, pick, edit, sleep, worker_view = ep.print_fields, ep.print_table, ep.ui.success, ep.pick, ep.click.edit, ep.time.sleep, ep._workers_view
            def record_fields(row, columns):
                case["fields"].append([[name, output.cell(get(row))] for name, get in columns])
                return fields(row, columns)
            def record_table(rows, columns, empty):
                case["tables"].append({"headers":[name for name, _ in columns], "cells":[[output.cell(get(row)) for _, get in columns] for row in rows], "empty":empty})
                return table(rows, columns, empty)
            def record_success(message):
                case["success"].append(Text.from_markup(message).plain)
                return success(message)
            def record_workers(data):
                view = worker_view(data)
                header = view.renderables[0] if data["workers"] else view
                case["worker_summary"] = str(header)
                if data["workers"]:
                    case["tables"].append({"headers":[name for name, _ in ep._WORKER_COLUMNS], "cells":[[output.cell(get(row)) for _,get in ep._WORKER_COLUMNS] for row in data["workers"]], "empty":""})
                return view
            def record_pick(message, choices, missing):
                selected = pick(message, choices, missing)
                case["picks"].append({"message":message, "choices":choices, "missing":missing, "selected":selected})
                return selected
            def record_edit(text, **kwargs):
                edited = edit(text, **kwargs)
                case["edits"].append({"text":text, "result":edited})
                return edited
            def record_sleep(seconds):
                case["sleeps"].append(seconds)
                try: return sleep(seconds)
                except KeyboardInterrupt:
                    case["interrupt_at_sleep"] = len(case["sleeps"])
                    raise
            for attr, fn in [("print_fields",record_fields), ("print_table",record_table), ("pick",record_pick)]: active.setattr(ep,attr,fn)
            active.setattr(ep.ui,"success",record_success)
            active.setattr(ep,"_workers_view",record_workers)
            active.setattr(ep.click,"edit",record_edit)
            active.setattr(ep.time,"sleep",record_sleep)
            case["interactive"] = ep.interactive()
            case["confirm_interactive"] = output.interactive()
            try: result = invoke(*args, input=input)
            finally: active.undo()
            case.update(code=result.exit_code, stdout=result.stdout, stderr=result.stderr)
            if result.exit_code and not isinstance(result.exception, SystemExit):
                raise AssertionError((case, result.exception))
            CASES.append(case)
            return result
        patcher.setattr(item.module, "invoke", wrapped)
        try: yield
        finally: patcher.undo()


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="endpoint-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    capture = Capture()
    status = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_endpoint_command.py"),
                          "--basetemp", str(Path(scratch) / "pytest"), "--no-cov", "-q"], plugins=[capture])
if status: raise SystemExit(status)

def extra(name, args, *, names=None, fail_after_update=False, input=None):
    case = {"name":name,"args":args,"input":input,"requests":[],"fields":[],"tables":[],"success":[],"picks":[],"edits":[],"sleeps":[],"interactive":False,"confirm_interactive":False}
    release_calls = [0]
    def request(method,path,**kwargs):
        call = {"method":method,"path":path,"params":{str(k):str(v) for k,v in kwargs.get("params",{}).items()},"body":copy.deepcopy(kwargs.get("json")),"status":200}
        case["requests"].append(call)
        if path == "/v2/serverless":
            value={"endpoints":[dict(capture.source.GPU_ENDPOINT,id=str(i),name=name) for i,name in enumerate(names)],"pagination":{"hasNextPage":False}}
        elif path.endswith("/releases"):
            release_calls[0] += 1
            if release_calls[0] > 1 and fail_after_update:
                call.update(status=503,response={"detail":"rollout read failed"})
                raise error.QueryError("rollout read failed")
            value={"endpointVersion":3,"rollout":{}}
        elif method == "PATCH": value=dict(capture.source.GPU_ENDPOINT,**kwargs["json"])
        else: value=capture.source.GPU_ENDPOINT
        call["response"] = copy.deepcopy(value)
        return copy.deepcopy(value)
    with ExitStack() as stack:
        for module in [ep,compute,api]: stack.enter_context(patch.object(module,"run_rest_request",request))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result=CliRunner().invoke(cli,["endpoint",*args],input=input)
    case.update(code=result.exit_code,stdout=result.stdout,stderr=result.stderr)
    if result.exit_code and not isinstance(result.exception,SystemExit): raise AssertionError(result.exception)
    CASES.append(case)

for pattern in ["LLaMA**", "***", "model[", "model[[]", "model?", "model[!b]", "[!a-c]*", "a{b,c}", "model[]", "model[!]", "model[]a]", "model[z-a]"]:
    extra("additional_glob_edges",["list","--name",pattern,"--json"],names=["llama-prod","model[","modela","modelb","a{b,c}","model[]","model[!]","model]","modelz"])
extra("partial_output_survives_rollout_failure",["update","ep1","--image","new","--wait","--json"],fail_after_update=True)
extra("rollout_timeout_is_successful_detachment",["update","ep1","--image","new","--wait","--timeout","0","--json"])
extra("stdin_edit_document",["edit","ep1","--file","-","--yes","--json"],input='{"name":"from-stdin"}')
extra("encoded_endpoint_id",["get","ep /?#%é","--json"])
assert not Path("{missing-document}").exists()
extra("unreadable_edit_document",["edit","ep1","--file","{missing-document}","--yes"])

def rollout(name, *, previous=3, timeout=0, delayed=False, progressing=False, interrupt=False):
    clock = [0.0]
    case = {"name":name, "helper":"follow", "previous":previous, "timeout":timeout,
            "requests":[], "sleeps":[]}
    def request(method, path, **kwargs):
        state = {"endpointVersion":4 if progressing or (delayed and clock[0] >= 12) else 3,
                 "rollout":{"inProgress":progressing}}
        case["requests"].append({"method":method, "path":path, "params":{"limit":"1"}, "body":None, "status":200, "response":state})
        return state
    def sleep(seconds):
        case["sleeps"].append(seconds)
        if interrupt:
            case["interrupt_at_sleep"] = len(case["sleeps"])
            raise KeyboardInterrupt
        clock[0] += seconds
    with ExitStack() as stack:
        stack.enter_context(patch.object(ep,"run_rest_request",request))
        stack.enter_context(patch.object(ep.time,"monotonic",lambda:clock[0]))
        stack.enter_context(patch.object(ep.time,"sleep",sleep))
        with ep.ui.console.capture() as captured:
            ep._follow_rollout("ep1",previous,timeout)
    case["stdout"] = captured.get()
    CASES.append(case)

rollout("test_follow_rollout_without_new_release")
rollout("test_follow_rollout_waits_for_delayed_release",timeout=30,delayed=True)
rollout("test_follow_rollout_reports_unobserved_release_at_deadline",timeout=20)
rollout("test_follow_rollout_times_out_and_detaches",progressing=True)
rollout("test_follow_rollout_times_out_and_detaches",progressing=True,timeout=60,interrupt=True)
(ROOT / "tests/fixtures/endpoint_cli.json").write_text(json.dumps(CASES, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(CASES)} invocations in {len({case['name'] for case in CASES})} named command/helper groups")
