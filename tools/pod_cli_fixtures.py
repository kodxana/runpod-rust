"""Capture pod resource commands while running the pinned Python assertions."""
import copy
import json
import tempfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch

import pytest
from runpod import error
from runpod.rp_cli import compute, output, pod

ROOT = Path(__file__).resolve().parents[1]
CASES = []
CLASSES = {"TestList", "TestGet", "TestCreateGpu", "TestCreateCpu", "TestCreateContainer",
           "TestCreateValidation", "TestWait", "TestUpdate", "TestActions", "TestTerminate"}


def capture(module, name, invoke, args, **kwargs):
    case = dict(name=name, args=list(args), interactive=kwargs.get("interactive", False),
                input=kwargs.get("input"), requests=[], messages=[], fields=[], rows=[],
                wait_timeout=pod.WAIT_TIMEOUT)
    api_call = module.FakeApi.__call__
    resolve_gpu, resolve_cpu, resolve_regions = compute.resolve_gpus, compute.resolve_cpu, compute.resolve_regions
    fields, table, say, resolve_id, choices = pod.print_fields, pod.print_table, pod._say, pod._resolve_pod_id, pod._pod_choices

    def rest(api, method, path, params=None, json=None):
        call = dict(method=method, path=path, params=params or {}, body=json, status=200)
        try:
            result = api_call(api, method, path, params=params, json=json)
        except (error.QueryError, error.AuthenticationError) as exc:
            call.update(status=401 if isinstance(exc, error.AuthenticationError) else 400,
                        response={"error": str(exc)})
            case["requests"].append(copy.deepcopy(call))
            raise
        call["response"] = result
        case["requests"].append(copy.deepcopy(call))
        return result

    def catalog_gpu(query, gpus):
        options = compute.list_gpus.call_args.kwargs
        params = dict(include="AVAILABILITY", product="POD", count=str(options["count"]), cloud=options["cloud"])
        if options["cuda_versions"]: params["cudaVersions"] = ",".join(options["cuda_versions"])
        if options["min_cuda_version"]: params["minCudaVersion"] = options["min_cuda_version"]
        case["requests"].append(dict(method="GET", path="/v2/catalog/gpus", params=params,
            body=None, status=200, response={"gpus": copy.deepcopy(gpus)}))
        return resolve_gpu(query, gpus)

    def catalog_cpu(query, cpus):
        options = compute.list_cpus.call_args.kwargs
        params = dict(include="AVAILABILITY", product="POD")
        if options["vcpu_count"] is not None: params["vcpuCount"] = str(options["vcpu_count"])
        case["requests"].append(dict(method="GET", path="/v2/catalog/cpus", params=params,
            body=None, status=200, response={"cpus": copy.deepcopy(cpus)}))
        return resolve_cpu(query, cpus)

    def catalog_regions(regions, datacenters):
        case["requests"].append(dict(method="GET", path="/v2/catalog/datacenters", params={},
            body=None, status=200, response={"dataCenters": copy.deepcopy(datacenters)}))
        return resolve_regions(regions, datacenters)

    def print_fields(record, columns):
        case["fields"] = [[label, output.cell(getter(record))] for label, getter in columns]
        return fields(record, columns)

    def print_table(records, columns, empty):
        case["headers"] = [label for label, _ in columns]
        case["rows"] = [[output.cell(getter(record)) for _, getter in columns] for record in records]
        case["empty"] = empty
        return table(records, columns, empty)

    def record_say(message, as_json, warning=False):
        case["messages"].append(dict(message=message, stderr=as_json, warning=warning))
        return say(message, as_json, warning)

    def record_choices(action=None):
        result = choices(action)
        case["choices"] = result
        return result

    def record_id(pod_id, action=None):
        result = resolve_id(pod_id, action)
        if not pod_id: case["picked"] = result
        return result

    with ExitStack() as stack:
        for owner, field, value in [(module.FakeApi,"__call__",rest),
            (compute,"resolve_gpus",catalog_gpu),(compute,"resolve_cpu",catalog_cpu),
            (compute,"resolve_regions",catalog_regions),(pod,"print_fields",print_fields),
            (pod,"print_table",print_table),(pod,"_say",record_say),
            (pod,"_pod_choices",record_choices),(pod,"_resolve_pod_id",record_id)]:
            stack.enter_context(patch.object(owner,field,value))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result = invoke(args, **kwargs)
    case.update(code=result.exit_code, stdout=result.stdout, stderr=result.stderr)
    assert not result.exit_code or isinstance(result.exception, SystemExit), result.exception
    CASES.append(case)
    return result


class Capture:
    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.cls and item.cls.__name__ in CLASSES]
        self.module = items[0].module

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item):
        invoke = item.module._invoke
        name = f"{item.cls.__name__}.{item.originalname}"
        with patch.object(item.module, "_invoke", lambda args, **kwargs: capture(item.module, name, invoke, args, **kwargs)):
            yield


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
capture_tests = Capture()
with tempfile.TemporaryDirectory(prefix="pod-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_pod_command.py"),
        "--basetemp", str(Path(scratch) / "pytest"), "--no-cov", "-q"], plugins=[capture_tests])
if status: raise SystemExit(status)
for value in ["bad", [{}], 1]:
    # FakeApi uses lists as response queues, so wrap a list-shaped payload once.
    response = [value] if isinstance(value, list) else value
    api = capture_tests.module.FakeApi({("GET", "/v2/pods/pod123"): response})
    with patch("runpod.rp_cli.update.start_background_check"):
        result = capture_tests.module._invoke(["update", "pod123", "--env", "K=V"], api=api)
    assert result.exit_code == 1
    assert [call[:2] for call in api.calls] == [("GET", "/v2/pods/pod123")]
print("Verified three malformed-current responses fail before PATCH")
(ROOT / "tests/fixtures/pod_cli.json").write_text(json.dumps(CASES, indent=2) + "\n")
print(f"Captured {len(CASES)} invocations across {len({case['name'] for case in CASES})} source functions")
