"""Capture actual Python control-plane calls as offline differential-test fixtures.

Run with the pinned upstream SDK installed in an isolated Python environment.
No Runpod service is contacted; both transports are intercepted.
"""
import json
import pathlib
from unittest.mock import patch
from runpod.api import ctl_commands as ctl
from runpod.error import QueryError

CASES = [
    ("test_get_user_uses_graphql", "get_user", {}),
    ("test_update_user_settings_replaces_ssh_keys", "update_user_settings", {"pubkey": "\n ssh-ed25519 key user \n\nssh-rsa second\n"}),
    ("test_get_gpus_unwraps_response", "get_gpus", {}),
    ("test_get_gpu_requests_pod_availability", "get_gpu", {"gpu_id": "NVIDIA A100", "gpu_quantity": 2}),
    ("test_get_gpu_converts_not_found_to_value_error", "get_gpu", {"gpu_id": "missing"}),
    ("test_get_gpu_propagates_other_api_errors", "get_gpu", {"gpu_id": "forbidden"}),
    ("test_get_pods_unwraps_response", "get_pods", {}),
    ("test_get_pod_escapes_id", "get_pod", {"pod_id": "pod/id"}),
    ("test_get_pod_returns_none_when_not_found", "get_pod", {"pod_id": "missing"}),
    ("test_get_pod_propagates_other_api_errors", "get_pod", {"pod_id": "forbidden"}),
    ("test_create_cpu_pod_requires_instance_id", "create_pod", {"name":"cpu-pod", "image_name":"python:3.11"}),
    ("test_create_pod_validates_image_and_cloud", "create_pod", {"name":"pod", "gpu_type_id":"NVIDIA A100"}),
    ("invalid_cloud", "create_pod", {"name":"pod", "image_name":"image", "gpu_type_id":"NVIDIA A100", "cloud_type":"INVALID"}),
    ("test_create_gpu_pod_enforces_per_gpu_host_minima", "create_pod", {"name":"training", "image_name":"image", "gpu_type_id":"NVIDIA A100", "gpu_count":2, "min_memory_in_gb":32, "min_vcpu_count":8}),
    ("full_gpu_pod", "create_pod", {"name":"gpu", "image_name":"image", "gpu_type_id":"NVIDIA A100", "cloud_type":"SECURE", "start_ssh":False,"data_center_id":"US-KS-2","container_disk_in_gb":50,"docker_args":"echo ok", "ports":"80/http, 22/tcp, ","env":{"NUMBER":1,"BOOL":True},"volume_in_gb":20,"allowed_cuda_versions":["12.8"," 13.0 "]}),
    ("cpu_pod", "create_pod", {"name":"cpu", "image_name":"image", "instance_id":"cpu3c-4-8"}),
    ("test_create_cpu_pod_rejects_unrepresentable_template_mount_path", "create_pod", {"name":"training","template_id":"template/id","instance_id":"cpu3c-4-8","volume_mount_path":"/custom"}),
    ("test_resume_pod_rejects_gpu_count", "resume_pod", {"pod_id":"pod", "gpu_count":8}),
    ("stop_pod", "stop_pod", {"pod_id":"pod/id"}),
    ("resume_pod", "resume_pod", {"pod_id":"pod/id"}),
    ("test_terminate_pod_deletes_resource", "terminate_pod", {"pod_id":"pod/id"}),
    ("test_get_endpoints_unwraps_response", "get_endpoints", {}),
    ("create_endpoint_defaults", "create_endpoint", {"name":"ep", "template_id":"t"}),
    ("endpoint_request_scaling", "create_endpoint", {"name":"ep", "template_id":"t", "scaler_type":"REQUEST_COUNT", "scaler_value":10}),
    ("endpoint_invalid_idle", "create_endpoint", {"name":"ep", "template_id":"t", "scaler_type":"REQUEST_COUNT", "idle_timeout":5}),
    ("endpoint_invalid_scaler", "create_endpoint", {"name":"ep", "template_id":"t", "scaler_type":"OTHER"}),
    ("test_create_endpoint_filters_gpu_pools_and_datacenters", "create_endpoint", {"name":"ep", "template_id":"t","gpu_ids":"ADA_48_PRO,-NVIDIA L40,-NVIDIA L40", "locations":"EU-RO-1, US-KS-2","network_volume_id":"volume","flashboot":True,"allowed_cuda_versions":"12.8, 13.0"}),
    ("update_endpoint_template", "update_endpoint_template", {"endpoint_id":"ep/id", "template_id":"template"}),
    ("create_template_defaults", "create_template", {"name":"t","image_name":"image"}),
    ("full_template", "create_template", {"name":"t","image_name":"image","docker_start_cmd":"", "volume_in_gb":10,"ports":"80/http,22/tcp","env":{"BOOL":False,"NUMBER":42},"registry_auth_id":"registry","is_serverless":True}),
    ("create_container_registry_auth", "create_container_registry_auth", {"name":"r","username":"u","password":"p\"\\\n"}),
    ("update_container_registry_auth", "update_container_registry_auth", {"registry_auth_id":"r","username":"u","password":"p\"\\\n"}),
    ("test_delete_container_registry_auth_uses_rest", "delete_container_registry_auth", {"registry_auth_id":"registry/id"}),
]
for kwargs in [{}, {"docker_args":""}, {"volume_mount_path":"/custom"}, {"network_volume_id":"volume"}, {"volume_in_gb":20}]:
    CASES.append(("template_precedence_" + str(len(CASES)), "create_pod", {"name":"training","template_id":"template/id","gpu_type_id":"NVIDIA A100",**kwargs}))
for instance in ["cpu3c-invalid", "cpu3c", "cpu3c-4-not-a-number", "cpu3c-0-4", "cpu3c-01-4"]:
    CASES.append(("invalid_cpu_" + instance, "create_pod", {"name":"cpu", "image_name":"image", "instance_id":instance}))
for key, value in [("support_public_ip",True),("country_code","US"),("min_download",100),("min_upload",100)]:
    CASES.append(("unsupported_" + key, "create_pod", {"name":"pod","image_name":"image","gpu_type_id":"NVIDIA A100",key:value}))

fixtures = []
for name, operation, kwargs in CASES:
    requests = []
    def rest(method, path, **kw):
        status = 404 if path.endswith("/missing") else 403 if path.endswith("/forbidden") else 200
        response = {"id":"result", "pods":[{"id":"p"}], "gpus":[{"id":"g"}],"endpoints":[{"id":"e"}]}
        if path.startswith("/v2/templates/"):
            response = {"mounts":{"persistent":{"size":30,"path":"/training"}}}
        if status != 200:
            response = {"detail":"not found" if status == 404 else "forbidden"}
        requests.append({"method":method,"path":path,"body":kw.get("json"),"params":kw.get("params",{}),"response":response,"status":status})
        if status != 200:
            raise QueryError(response["detail"], status_code=status)
        return response
    def graphql(query, **kw):
        response = {"data":{"myself":{"id":"user"},"updateRegistryAuth":{"id":"registry"}}}
        requests.append({"method":"POST","path":"/graphql","body":{"query":query},"params":{},"response":response,"status":200})
        return response
    case = {"name":name,"operation":operation,"kwargs":kwargs,"requests":requests}
    with patch.object(ctl,"run_rest_request",side_effect=rest), patch.object(ctl,"run_graphql_query",side_effect=graphql):
        try:
            case["output"] = getattr(ctl,operation)(**kwargs)
        except (ValueError,QueryError) as error:
            case["error"] = str(error)
    fixtures.append(case)
path = pathlib.Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "control.json"
path.parent.mkdir(exist_ok=True)
path.write_text(json.dumps(fixtures, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(fixtures)} Python control-plane fixtures")
