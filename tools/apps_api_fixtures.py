"""Capture Apps API contracts from pinned Python; no external requests are made."""
import asyncio
import json
from pathlib import Path
from unittest.mock import patch

from runpod.apps import api
from runpod.error import QueryError

CASES = []


def case(name, operation, kwargs, **scenario):
    CASES.append(dict(name=name, operation=operation, kwargs=kwargs, **scenario))


for operation, kwargs in [
    ("delete_endpoint", {"endpoint_id": "ep/id"}),
    ("list_my_endpoints", {}),
    ("endpoint_workers", {"endpoint_id": "ep/id"}),
    ("terminate_pod", {"pod_id": "pod/id"}),
    ("get_app_by_name", {"name": "app"}),
    ("create_app", {"name": "app"}),
    ("create_environment", {"app_id": "app1", "name": "prod"}),
    ("list_global_volumes", {}),
    ("create_global_volume", {"name": "data"}),
    ("list_network_volumes", {}),
    ("create_network_volume", {"name": "data", "size": 50, "data_center_id": "US-CA-2"}),
    ("list_registry_auths", {}),
    ("create_registry_auth", {"name": "reg", "username": "user", "password": "dummy"}),
    ("delete_registry_auth", {"auth_id": "reg/id"}),
    ("list_secrets", {}),
    ("create_secret", {"name": "token", "value": "dummy", "description": "desc"}),
    ("delete_secret", {"secret_id": "secret1"}),
    ("list_apps", {}),
    ("get_environment_by_name", {"app_name": "app", "env_name": "prod"}),
    ("delete_app", {"app_id": "app1"}),
    ("delete_environment", {"environment_id": "env1"}),
    ("create_auth_request", {}),
    ("get_auth_request_status", {"request_id": "request1"}),
    ("prepare_artifact_upload", {"app_id": "app1", "tarball_size": 12345}),
    ("finalize_artifact_upload", {"app_id": "app1", "object_key": "key", "manifest": {"resources": []}}),
    ("deploy_build", {"environment_id": "env1", "build_id": "build1"}),
    ("network_volume_datacenters", {}),
]:
    case(operation, operation, kwargs)

base = {"name": "chat", "template": {"imageName": "image"}, "scalerType": "QUEUE_DELAY", "scalerValue": 4}
case("dev_endpoint_preserves_runtime_and_placement", "save_endpoint", {"endpoint_input": {
    **base, "template": {"imageName": "image", "containerDiskInGb": 50, "dockerArgs": "serve",
        "containerRegistryAuthId": "reg", "env": [{"key": "A", "value": "B"}], "ports": "80/http, 22/tcp"},
    "gpuIds": "ADA_48_PRO,-NVIDIA L40", "gpuCount": 2, "locations": "US-CA-2, EU-RO-1",
    "minCudaVersion": "12.8", "allowedCudaVersions": ["12.8", "13.0"],
    "networkVolumeIds": [{"networkVolumeId": "volume1"}], "workersMin": 0, "workersMax": 2,
    "idleTimeout": 60, "executionTimeoutMs": 90000, "flashBootType": True,
}})
for routing in ["QB", "LB"]:
    case("request_count_" + routing, "save_endpoint", {"endpoint_input": {**base, "type": routing, "scalerType": "REQUEST_COUNT", "scalerValue": 7, "idleTimeout": 60}})
for extra in [{"flashEnvironmentId": "env1"}, {"model": "owner/model"}, {"instanceIds": ["cpu3c-1-2"]}, {"networkVolumeIds": [{"networkVolumeId": "v", "dataCenterId": "dc"}]}, {"scalerType": "OTHER"}, {"template": {"imageName": "image", "name": "named"}}]:
    case("atomic_gql_" + str(len(CASES)), "save_endpoint", {"endpoint_input": {**base, **extra}})
case("rest_cpu_update", "save_endpoint", {"endpoint_input": {"id": "ep/id", "type": "LB", "instanceIds": ["cpu5c-2-4", "cpu5g-4-16"], "scalerType": "REQUEST_COUNT", "scalerValue": 7, "idleTimeout": 60}}, current={"type": "LOAD_BALANCER", "cpu": [{"id": "cpu5c"}]})
case("routing_change", "save_endpoint", {"endpoint_input": {"id": "ep/id", "type": "LB"}}, current={"type": "QUEUE"})
case("compute_change", "save_endpoint", {"endpoint_input": {"id": "ep/id", "instanceIds": ["cpu3c-2-4"]}}, current={"type": "QUEUE", "gpu": {}})
case("current_request_scaling", "save_endpoint", {"endpoint_input": {"id": "ep/id", "idleTimeout": 60}}, current={"type": "QUEUE", "scaling": {"type": "REQUEST_COUNT"}})
for empty in [None, {}]:
    case("empty_endpoint_" + str(len(CASES)), "save_endpoint", {"endpoint_input": {"id": "ep1", "name": "chat"}}, current=empty)
case("gpu_pod", "deploy_task_pod", {"pod_input": {"gpuTypeIdList": ["NVIDIA GeForce RTX 4090"], "supportPublicIp": True, "terminateAfter": "2026-09-15T12:30:00Z"}, "is_cpu": False})
for scenario in [{}, {"capacity": True}, {"failure": "ambiguous failure"}]:
    case("cpu_alternatives_" + str(len(CASES)), "deploy_task_pod", {"pod_input": {"instanceIds": ["cpu3c-2-4", "cpu3g-2-8"], "dataCenterIds": ["US-CA-2"], "networkVolumeId": "v"}, "is_cpu": True}, **scenario)
for ids in [None, [], "cpu3c-2-4", [None], [" "], [42]]:
    case("invalid_cpu_" + str(len(CASES)), "deploy_task_pod", {"pod_input": {"instanceIds": ids}, "is_cpu": True})
for pods in [False, True]:
    case("gpu_stock_" + str(pods), "gpu_stock_status", {"gpu_id": "NVIDIA H100", "data_center_id": "US-CA-2", "gpu_count": 2, "pods": pods})
    for instance in ["cpu3c-4-8", "cpu3c-1-2", "cpu3c-3-6"]:
        case("cpu_stock_" + instance + str(pods), "cpu_stock_status", {"instance_id": instance, "data_center_id": "US-CA-2", "pods": pods})
case("missing_stock", "gpu_stock_status", {"gpu_id": "gpu", "data_center_id": "missing"})
for operation, kwargs in [("get_app_by_name", {"name": "app"}), ("get_environment_by_name", {"app_name": "app", "env_name": "prod"})]:
    case("not_found_" + operation, operation, kwargs, failure="not found")
case("environment_not_found", "get_environment_by_name", {"app_name": "app", "env_name": "prod"}, environment_missing=True)
for response in [None, {}, {"id": ""}, {"id": 42}]:
    case("invalid_global_volume_" + str(len(CASES)), "create_global_volume", {"name": "data"}, rest_response=response)
for response in [{}, {"dataCenters": [{}]}, {"dataCenters": [{"id": "dc", "networkVolumeTypes": [None]}]}]:
    case("invalid_datacenter_" + str(len(CASES)), "network_volume_datacenters", {}, rest_response=response)


async def capture(case):
    requests = []
    gql_data = {k: {"id": k} for k in ["saveEndpoint", "deployCpuPod", "podFindAndDeployOnDemand", "flashAppByName", "createFlashApp", "createFlashEnvironment", "secretCreate", "flashEnvironmentByName", "createFlashAuthRequest", "flashAuthRequestStatus", "prepareFlashArtifactUpload", "finalizeFlashArtifactUpload", "deployBuildToEnvironment"]}
    gql_data.update(myself={"secrets": [{"id": "s"}], "flashApps": [{"id": "a"}]}, secretDelete=True, deleteFlashApp=True, deleteFlashEnvironment=True, cpuFlavors=[{"specifics": {"stockStatus": "High"}}])

    async def rest(method, path, **kw):
        response = case.get("rest_response", {"id": "result", "endpoints": [{"id": "ep"}], "registries": [{"id": "reg"}], "dataCenters": [{"id": "US-CA-2", "availability": "HIGH", "networkVolumeTypes": ["STANDARD"]}, {"id": "EU-RO-1", "networkVolumeTypes": []}]})
        if method == "GET" and path.startswith("/v2/serverless/"):
            response = case.get("current", response)
        requests.append(dict(method=method, path=path, body=kw.get("json"), params=kw.get("params", {}), response=response, status=200, anonymous=False))
        return response

    async def paginated(path, key, **kw):
        response = {key: [{"id": "v1"}], "pagination": {"hasNextPage": False}}
        requests.append(dict(method="GET", path=path, body=None, params={}, response=response, status=200, anonymous=False))
        return response[key]

    async def graphql(query, **kw):
        failure = case.get("failure")
        if case.get("capacity") and not requests:
            failure = "insufficient capacity"
        if case.get("environment_missing") and requests:
            failure = "not found"
        response = {"errors": [{"message": failure}]} if failure else {"data": gql_data}
        body = {"query": query}
        if kw.get("variables"):
            body["variables"] = kw["variables"]
        requests.append(dict(method="POST", path="/graphql", body=body, params={}, response=response, status=200, anonymous=kw.get("anonymous", False)))
        if failure:
            raise QueryError(failure)
        return response

    output = {k: case[k] for k in ["name", "operation", "kwargs"]}
    with patch.object(api, "run_rest_request_async", rest), patch.object(api, "run_rest_paginated_request_async", paginated), patch.object(api, "run_graphql_query_async", graphql):
        try:
            result = await getattr(api.AppsApiClient("test-key"), case["operation"])(**case["kwargs"])
            output["output"] = sorted(result) if isinstance(result, set) else result
        except (ValueError, QueryError) as error:
            output["error"] = str(error)
    output["requests"] = requests
    return output


async def main():
    fixtures = [await capture(item) for item in CASES]
    path = Path(__file__).resolve().parents[1] / "tests/fixtures/apps_api.json"
    path.write_text(json.dumps(fixtures, indent=2) + "\n", encoding="utf-8")
    print(f"Captured {len(fixtures)} Apps API cases")


asyncio.run(main())
