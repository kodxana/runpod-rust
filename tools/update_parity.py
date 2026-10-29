"""Update only reviewed mappings; leave all other entries pending."""
import json
import pathlib

root = pathlib.Path(__file__).resolve().parents[1]
path = root / "parity/api.json"
api = json.loads(path.read_text())
for entry in api["entries"]:
    file, name = entry["file"], entry["name"]
    target = None
    if file == "runpod/api/ctl_commands.py" and entry["public"]:
        target = f"Client::{name}"
    if file == "runpod/api/rest.py" and entry["public"]:
        target = "Client::" + name.removesuffix("_async")
    if file == "runpod/endpoint/helpers.py" and name == "is_completed":
        target = "endpoint::is_completed"
    if file in ("runpod/endpoint/runner.py", "runpod/endpoint/asyncio/asyncio_runner.py"):
        prefix = "Asyncio" if "/asyncio/" in file else ""
        if name in ("Endpoint", "Job") or name.startswith(("Endpoint.", "Job.")):
            parts = name.split(".")
            if len(parts) == 1 or parts[1] in ("__init__", "run", "run_sync", "health", "purge_queue", "status", "output", "stream", "cancel"):
                target = prefix + parts[0] + ("::" + ("new" if parts[1] == "__init__" else parts[1]) if len(parts) > 1 else "")
    if file == "runpod/error.py" and name in ("RunPodError", "AuthenticationError", "QueryError"):
        target = {"RunPodError":"Error", "AuthenticationError":"Error::Authentication", "QueryError":"Error::Query"}[name]
    if file == "runpod/cli/groups/config/functions.py" and entry["public"]:
        target = f"credentials::CredentialStore::{name}"
    specific = {
        ("runpod/serverless/utils/rp_validator.py", "validate"): "utils::validator::validate",
        ("runpod/serverless/utils/rp_cleanup.py", "clean"): "utils::clean",
        ("runpod/serverless/utils/rp_model_cache.py", "resolve_model_cache_path_from_hugginface_repository"): "utils::resolve_model_cache_path_from_hugginface_repository",
        ("runpod/_health/cuda.py", "is_available"): "utils::is_cuda_available",
        ("runpod/serverless/modules/rp_job.py", "run_job"): "serverless::run_job",
        ("runpod/serverless/modules/rp_job.py", "get_job"): "serverless::WorkerTransport::get_job",
        ("runpod/serverless/modules/rp_job.py", "get_stop_signals"): "serverless::WorkerTransport::get_stop_signals",
        ("runpod/serverless/modules/rp_job.py", "handle_job"): "serverless::Worker::handle_job",
        ("runpod/serverless/modules/rp_http.py", "send_result"): "serverless::WorkerTransport::send_result",
        ("runpod/serverless/modules/rp_http.py", "stream_result"): "serverless::WorkerTransport::stream_result",
        ("runpod/serverless/modules/rp_progress.py", "progress_update"): "serverless::WorkerTransport::progress_update",
    }
    target = specific.get((file, name), target)
    app_symbols = {
        "runpod/apps/model.py": {"Model":"apps::Model", "Model.path":"apps::Model::path", "Model.hf_cache_path":"apps::Model::hf_cache_path"},
        "runpod/apps/secret.py": {"Secret":"apps::Secret", "Secret.reference":"apps::Secret::reference", "render_env":"apps::render_env", "secret_names":"apps::secret_names"},
        "runpod/apps/spec.py": {"ResourceKind":"apps::ResourceKind", "RouteSpec":"apps::RouteSpec", "ResourceSpec":"apps::ResourceSpec", "normalize_workers":"apps::spec::normalize_workers", "ResourceSpec.validate":"apps::ResourceSpec::validate", "ResourceSpec.is_cpu":"apps::ResourceSpec::is_cpu", "ResourceSpec.effective_scaler_type":"apps::ResourceSpec::effective_scaler_type", "ResourceSpec.to_manifest":"apps::ResourceSpec::to_manifest"},
        "runpod/apps/gpu.py": {"GpuGroup":"apps::GpuGroup", "GpuType":"apps::GpuType", "CpuInstanceType":"apps::CpuInstanceType", "GpuGroup.all":"apps::GpuGroup::all", "GpuGroup.device_names":"apps::GpuGroup::device_names", "GpuType.all":"apps::GpuType::all", "pool_for_gpu_type":"apps::catalog::pool_for_gpu_type", "resolve_gpu_string":"apps::gpu::resolve_gpu_string", "needs_gpu_catalog":"apps::gpu::needs_gpu_catalog", "gpu_ids_value":"apps::gpu::gpu_ids_value"},
        "runpod/apps/datacenter.py": {"DataCenter":"apps::DataCenter", "DataCenter.from_string":"apps::DataCenter::from_str", "DataCenter.all":"apps::DataCenter::all"},
        "runpod/apps/volume.py": {"Volume":"apps::Volume", "NetworkVolume":"apps::NetworkVolume", "GlobalVolume":"apps::GlobalVolume", "Volume.kind":"apps::Volume::kind", "Volume.reference":"apps::Volume::reference", "Volume.path":"apps::Volume::path", "normalize_mounts":"apps::volume::normalize_mounts", "validate_mounts":"apps::volume::validate_mounts"},
        "runpod/apps/markers.py": {"validate_route":"apps::spec::validate_route"},
    }
    target = app_symbols.get(file, {}).get(name, target)
    if file == "runpod/serverless/modules/rp_prestart.py" and name in ("register_prestart_hook", "get_prestart_hooks", "has_prestart_hooks", "clear_prestart_hooks", "run_prestart_phase"):
        target = f"serverless::PrestartHooks::{name}"
    if file == "runpod/serverless/modules/worker_state.py" and name.startswith("JobsProgress.") and name.split(".")[-1] in ("clear", "add", "remove", "get", "get_job_list", "get_job_count"):
        target = "serverless::" + name.replace(".", "::")
    if target:
        entry.update(rust=target, status="implemented")
path.write_text(json.dumps(api, indent=2) + "\n")

path = root / "parity/tests.json"
tests = json.loads(path.read_text())
mapping = {
    "test_authenticated_mutation_at_custom_base_url":"transport::test_authenticated_mutation_at_custom_base_url",
    "test_missing_key_fails_before_network_request":"transport::test_missing_key_fails_before_network_request",
    "test_invalid_key_raises_authentication_error":"transport::test_invalid_key_raises_authentication_error",
    "test_empty_response_returns_none":"transport::test_empty_response_returns_none",
    "test_problem_json_preserves_validation_error":"transport::test_problem_json_preserves_validation_error",
    "test_error_body_fallbacks_do_not_mask_http_failure":"transport::test_error_body_fallbacks_do_not_mask_http_failure",
}
fixtures = json.loads((root / "tests/fixtures/control.json").read_text())
for entry in tests["entries"]:
    target = None
    if entry["file"] == "tests/test_api/test_rest.py":
        target = mapping.get(entry["name"])
    if entry["file"] == "tests/test_api/test_ctl_commands.py":
        matching = [case["name"] for case in fixtures if case["name"] == entry["name"]]
        if matching:
            target = "control::python_control_plane_fixtures [" + ", ".join(matching) + "]"
    if entry["file"] == "tests/test_endpoint/test_runner.py":
        leaf = entry["name"].split(".")[-1]
        if leaf in ("test_endpoint_run", "test_endpoint_run_sync", "test_run_sync_with_timeout", "test_job_status", "test_output_with_sleep", "test_job_stream"):
            target = "endpoint::" + leaf
    if target:
        entry.update(rust=target, status="ported")
path.write_text(json.dumps(tests, indent=2) + "\n")
print(f"Mapped {sum(e['status']=='implemented' for e in api['entries'])} symbols and {sum(e['status']=='ported' for e in tests['entries'])} tests; remaining entries retained as pending")
