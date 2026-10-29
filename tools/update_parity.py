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
    if file == "runpod/apps/api.py":
        if name == "AppsApiClient":
            target = "apps::AppsApiClient"
        elif name.startswith("AppsApiClient."):
            method = name.split(".")[-1]
            method = {"__init__":"new", "_execute":"execute", "_put_tarball":"put_tarball"}.get(method, method)
            target = "apps::AppsApiClient::" + method
        elif name == "is_capacity_error":
            target = "apps::api::is_capacity_error"
        elif name == "_rest_endpoint_input":
            target = "apps::endpoint_input::rest_endpoint_input"
        elif name == "_stock_in_datacenter":
            target = "apps::api::stock_in_datacenter"
    if file == "runpod/apps/gpu.py" and name == "live_pool_members_async":
        target = "apps::gpu::GpuCatalog::live_pool_members"
    more_app_symbols = {
        "runpod/apps/placement.py": {"StockMap":"apps::placement::StockMap", "StockMap.__init__":"apps::placement::StockMap::new", "StockMap.fetch":"apps::placement::StockMap::fetch", "StockMap.score":"apps::placement::StockMap::score", "_hardware_keys":"apps::placement::hardware_keys", "_score":"apps::placement::score_status", "candidates":"apps::placement::candidates", "solve_placement":"apps::placement::solve_placement"},
        "runpod/apps/volume.py": {"VolumeResolver":"apps::volume::VolumeResolver", "VolumeResolver.__init__":"apps::volume::VolumeResolver::new", "VolumeResolver.resolve":"apps::volume::VolumeResolver::resolve", "VolumeResolver.resolve_mounts":"apps::volume::VolumeResolver::resolve_mounts", "attach_pod_mounts":"apps::volume::attach_pod_mounts", "attach_endpoint_volumes":"apps::volume::attach_endpoint_volumes", "_bind_worker_mounts":"apps::resolution::bind_worker_mounts"},
        "runpod/apps/utils/lookup.py": {"find_by_id_or_name":"apps::lookup::find_by_id_or_name"},
        "runpod/apps/utils/events.py": {"emit":"apps::events::emit"},
        "runpod/apps/registry.py": {"resolve_registry_auth":"apps::registry::resolve_registry_auth"},
        "runpod/apps/secret.py": {"validate_secrets":"apps::validate_secrets"},
    }
    target = more_app_symbols.get(file, {}).get(name, target)
    invocation_symbols = {
        "runpod/apps/errors.py": {"AppError":"apps::AppError", "EndpointNotFound":"apps::AppError::EndpointNotFound", "EndpointNotFound.__init__":"apps::AppError::EndpointNotFound", "RemoteExecutionError":"apps::AppError::RemoteExecution", "ScheduleNotSupported":"apps::AppError::ScheduleNotSupported", "ScheduleNotSupported.__init__":"apps::AppError::ScheduleNotSupported", "InvalidResourceError":"apps::AppError::InvalidResource"},
        "runpod/apps/protocol.py": {"FunctionRequest":"apps::protocol::FunctionRequest", "FunctionRequest.to_input":"apps::protocol::FunctionRequest::to_input", "FunctionResponse":"apps::protocol::FunctionResponse", "FunctionResponse.from_output":"apps::protocol::FunctionResponse::from_output"},
        "runpod/apps/job.py": {"Job":"apps::Job / blocking::apps::Job", "Job.__init__":"apps::Job::new", "Job.id":"apps::Job::id", "Job.done":"apps::Job::done", "Job._update":"apps::Job::update", "Job._status_async":"apps::Job::status", "Job._result_async":"apps::Job::result", "Job._cancel_async":"apps::Job::cancel", "Job._retry_async":"apps::Job::retry", "Job._stream_async":"apps::Job::stream", "Job.__repr__":"apps::Job::fmt"},
        "runpod/apps/stubs.py": {"_StubBase":"apps::StubConfig", "_StubBase.__init__":"apps::StubConfig::new / ResourceRef", "_StubBase._target":"apps::targets::Target::sentinel", "Queue":"apps::Queue / blocking::apps::Queue", "Queue.__init__":"apps::Queue::new", "Queue.with_options":"apps::Queue::with_options", "Queue._payload":"apps::Queue::payload", "Queue._remote_async":"apps::Queue::remote", "Queue._spawn_async":"apps::Queue::spawn", "Queue._stream_async":"apps::Queue::stream", "Queue._job_async":"apps::Queue::job", "Queue.__repr__":"apps::Queue::fmt", "_StubRouteCaller":"apps::Api HTTP methods / blocking::apps::Api HTTP methods", "_StubRouteCaller.__init__":"apps::Api::new", "_StubRouteCaller.__call__":"blocking::apps::Api::request", "_StubRouteCaller.aio":"apps::Api::request", "Api":"apps::Api / blocking::apps::Api", "Api.__init__":"apps::Api::new", "Api.__repr__":"apps::Api::fmt"},
        "runpod/apps/targets.py": {"args_to_input":"apps::targets::args_to_input", "build_job_options":"apps::targets::JobOptions::to_payload", "merge_job_options":"apps::targets::merge_job_options", "unwrap_job_output":"apps::targets::unwrap_job_output", "_request_json":"apps::queue::QueueClient::request_json", "_post_json":"apps::queue::QueueClient::request_json", "_get_json":"apps::queue::QueueClient::request_json", "_wait_terminal":"apps::targets::QueueClient::wait", "QueueClient":"apps::targets::QueueClient", "QueueClient.__init__":"apps::targets::QueueClient::new", "QueueClient._call":"apps::queue::QueueClient::call", "SentinelTarget":"apps::targets::Target::sentinel", "SentinelTarget.__init__":"apps::targets::Target::sentinel", "SentinelTarget._sentinel_headers":"apps::queue::QueueClient::set_address", "LiveTarget.attach_source":"apps::targets::Target::attach_source", "LiveTarget._sync_source":"apps::targets::Target::sync_source", "LiveTarget.build_payload":"apps::targets::Target::build_payload", "LiveTarget.unwrap":"apps::targets::unwrap_live_output", "LiveTarget._unwrap_response":"apps::targets::unwrap_function_response"},
    }
    target = invocation_symbols.get(file, {}).get(name, target)
    log_symbols = {
        "runpod/apps/logs.py": {"_headers":"Client::build authentication headers", "pod_logs":"apps::logs::LogsClient::pod_logs", "stream_pod_logs":"apps::logs::LogsClient::stream_pod_logs", "tail_summary":"apps::logs::tail_summary"},
        "runpod/apps/monitor.py": {"_worker_frame":"apps::monitor::worker_frame", "_filter_line":"apps::monitor::filter_line", "_line_before":"apps::monitor::line_before", "format_worker_counts":"apps::monitor::format_worker_counts", "PodLogStream":"apps::monitor::PodLogStream", "PodLogStream.__init__":"apps::monitor::PodLogStream::new", "PodLogStream.attach":"apps::monitor::PodLogStream::attach", "PodLogStream.stop":"apps::monitor::PodLogStream::stop", "PodLogStream._emit_line":"apps::monitor::LogContext::emit_line", "PodLogStream._snapshot":"apps::monitor::LogContext::snapshot", "PodLogStream._follow":"apps::monitor::LogContext::follow", "WorkerMonitor":"apps::monitor::WorkerMonitor", "WorkerMonitor.__init__":"apps::monitor::WorkerMonitor::new", "WorkerMonitor.start":"apps::monitor::WorkerMonitor::start", "WorkerMonitor.on_status":"apps::monitor::WorkerMonitor::on_status", "WorkerMonitor.stop":"apps::monitor::WorkerMonitor::stop", "WorkerMonitor._poll_metrics":"apps::monitor::MonitorContext::poll_metrics", "WorkerMonitor._report_counts":"apps::monitor::WorkerMonitor::report_counts"},
        "runpod/apps/targets.py": {"LiveTarget":"apps::targets::Target::live", "LiveTarget.__init__":"apps::targets::Target::live / set_events / set_monitor_clients", "LiveTarget._monitor":"apps::targets::Target::monitor", "LiveTarget.invoke":"apps::targets::Target::invoke"},
    }
    target = log_symbols.get(file, {}).get(name, target)
    task_symbols = {
        "runpod/apps/images.py": {"runtime_tag":"apps::images::runtime_tag", "runtime_image":"apps::images::RuntimeImages::runtime_image", "image_for_spec":"apps::images::RuntimeImages::image_for_spec"},
        "runpod/apps/shim.py": {"runtime_launcher":"apps::shim::runtime_launcher"},
        "runpod/apps/targets.py": {"PodTarget":"apps::targets::PodTarget", "PodTarget.__init__":"apps::targets::PodTarget::with_config", "PodTarget.build_payload":"apps::targets::PodTarget::build_payload", "PodTarget.invoke":"apps::targets::PodTarget::invoke", "PodTarget.submit":"apps::targets::PodTarget::submit"},
        "runpod/apps/tasks.py": {"_runtime_command":"apps::shim::runtime_launcher", "cuda_versions_at_least":"apps::tasks::cuda_versions_at_least", "_device_names":"apps::tasks::device_names", "_proxy_url":"apps::tasks::TaskExecution::proxy_url", "_pod_input":"apps::tasks::pod_input", "TaskExecution":"apps::tasks::TaskExecution", "TaskExecution.__init__":"apps::tasks::TaskExecution::with_config", "TaskExecution._headers":"apps::tasks::TaskExecution::runner_request", "TaskExecution._deploy_and_record":"apps::tasks::TaskExecution::start owned deployment task", "TaskExecution._deploy_pod":"apps::tasks::TaskExecution::deploy_pod", "TaskExecution._attach_mounts":"apps::volume::attach_pod_mounts", "TaskExecution._post":"apps::tasks::TaskExecution::post", "unwrap_task_response":"apps::tasks::unwrap_task_response", "TaskJob":"apps::tasks::TaskJob", "TaskJob.__init__":"apps::tasks::TaskJob::new"},
    }
    target = task_symbols.get(file, {}).get(name, target)
    if file == "runpod/apps/deploy.py" and name in ("DeployResult", "build_manifest", "_deployed_endpoint_input", "reconcile_endpoints", "_container_disk_gb"):
        target = "apps::deploy::" + {"_deployed_endpoint_input":"deployed_endpoint_input", "_container_disk_gb":"deployed_endpoint_input container disk selection"}.get(name, name)
    if file == "runpod/apps/tasks.py" and name in ("TaskExecution.start", "TaskExecution.wait_ready", "TaskExecution.execute", "TaskExecution.submit", "TaskExecution.set_timeout", "TaskExecution.poll_result", "TaskExecution.terminate", "TaskJob.pod_id", "TaskJob.wait", "TaskJob.cancel"):
        target = "apps::tasks::" + name.replace(".", "::")
    if file == "runpod/apps/targets.py" and "." in name:
        owner, method = name.split(".", 1)
        if owner == "QueueClient" and method in ("base_url", "run", "invoke", "status", "cancel", "retry", "stream", "wait", "http"):
            target = "apps::targets::QueueClient::" + method
        if owner == "SentinelTarget" and method in ("invoke", "submit", "job_status", "cancel_job", "retry_job", "stream_job", "wait", "request"):
            target = "apps::targets::Target::" + method
        # Resolve the concrete queue operations separately from abstract/Pod targets.
        if owner == "LiveTarget" and method in ("submit", "job_status", "cancel_job", "retry_job", "stream_job", "wait", "request"):
            target = "apps::targets::Target::" + method
    if file == "runpod/serverless/modules/rp_prestart.py" and name in ("register_prestart_hook", "get_prestart_hooks", "has_prestart_hooks", "clear_prestart_hooks", "run_prestart_phase"):
        target = f"serverless::PrestartHooks::{name}"
    if file == "runpod/serverless/modules/worker_state.py" and name.startswith("JobsProgress.") and name.split(".")[-1] in ("clear", "add", "remove", "get", "get_job_list", "get_job_count"):
        target = "serverless::" + name.replace(".", "::")
    if target:
        entry.update(rust=target, status="implemented")
    if file == "runpod/apps/images.py" and name == "local_python_version":
        entry.update(rust="apps::images::RuntimeImages::python_version", status="adapted", note="Explicit remote interpreter version, default 3.12, replaces local Python introspection. Native Rust serialization never launches Python; see PORTING.md.")
    if file == "runpod/apps/tasks.py" and name in ("finish_cleanup", "_cleanup_finished"):
        entry.update(rust="apps::tasks::finish_cleanup / CleanupGuard", status="adapted", note="Tokio owns detached cleanup tasks. Future cancellation starts cleanup without blocking Drop; the runtime must remain alive. The 30-second foreground cleanup deadline and original error precedence are preserved.")
    if file == "runpod/apps/serialization.py" and name in ("serialize_arg", "serialize_args", "serialize_kwargs", "deserialize_result", "get_function_source", "require_json_args"):
        entry.update(rust="apps::serialization::" + name if name != "require_json_args" else "serde_json::Value argument boundary", status="adapted",
                     note="Native Serde values and explicit FunctionSource metadata replace Python object execution/reflection; see PORTING.md. Built-in pickle interoperability is tested in both directions.")
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
    if entry["file"] == "tests/test_apps/test_api_client.py":
        leaf = entry["name"].split(".")[-1]
        if leaf in ("test_retries_transport_errors", "test_exhausted_retries_raise", "test_graphql_errors_propagate_immediately", "test_create_auth_request_is_anonymous"):
            target = "apps_api::" + leaf
        lifecycle = {
            "test_get_app_by_name":"get_app_by_name", "test_create_app":"create_app",
            "test_create_environment":"create_environment", "test_list_apps":"list_apps",
            "test_delete_app":"delete_app", "test_delete_environment":"delete_environment",
            "test_get_environment_by_name":"get_environment_by_name",
            "test_get_environment_missing_app":"not_found_get_environment_by_name",
            "test_get_environment_not_found":"environment_not_found",
            "test_prepare_artifact_upload":"prepare_artifact_upload",
            "test_finalize_artifact_upload":"finalize_artifact_upload", "test_deploy_build":"deploy_build",
            "test_status_poll_is_anonymous":"get_auth_request_status",
            "test_empty_endpoint_read_prevents_mutation":"empty_endpoint_*",
            "test_routing_change_remains_atomic_graphql_operation":"routing_change",
            "test_rest_update_preserves_cpu_alternatives_and_load_balancer_idle_timeout":"rest_cpu_update",
        }
        if leaf in lifecycle:
            target = "apps_api::python_apps_api_fixtures [" + lifecycle[leaf] + "]"
    if entry["file"] == "tests/test_apps/test_gpu_catalog.py" and entry["name"] == "test_failed_catalog_retries_after_cooldown":
        target = "apps_catalog::test_failed_catalog_retries_after_cooldown"
    if entry["file"] == "tests/test_apps/test_placement.py":
        target = "apps_placement::python_placement_fixtures"
    if entry["file"] == "tests/test_apps/test_volume.py":
        leaf = entry["name"].split(".")[-1]
        covered = {"test_existing_by_name":"test_existing_by_name_and_id", "test_existing_by_id":"test_existing_by_name_and_id", "test_storage_capability_beats_higher_hardware_stock":"test_storage_capability_beats_higher_hardware_stock", "test_missing_no_create_raises":"test_missing_no_create_raises", "test_duplicate_names_raise":"test_duplicate_names_raise_and_ids_take_precedence", "test_global_creation_is_cached_and_mounts_resolve_by_name_and_id":"test_global_creation_is_cached_and_mounts_resolve_by_name_and_id", "test_resolution_cached_per_name":"test_existing_by_name_and_id", "test_same_reference_in_different_backends_does_not_share_cache":"test_same_reference_in_different_backends_does_not_share_cache", "test_unsupported_explicit_pin_never_relocates":"test_unsupported_explicit_pin_never_relocates", "test_global_attachment_does_not_pin_datacenter":"test_global_attachment_does_not_pin_datacenter", "test_removing_storage_clears_backend_and_worker_bindings":"test_global_attachment_does_not_pin_datacenter"}
        if leaf in covered:
            target = "apps_resolution::" + covered[leaf]
    if entry["file"] == "tests/test_apps/test_secret.py":
        if entry["name"] in ("TestValidateSecrets.test_existing_pass", "TestValidateSecrets.test_missing_raises_with_hint"):
            target = "apps_credentials::test_existing_pass_and_missing_raises_with_hint"
        elif entry["name"] == "TestValidateSecrets.test_no_references_no_api_call":
            target = "apps_credentials::test_no_references_no_api_call"
    if entry["file"] == "tests/test_apps/test_targets.py":
        name = entry["name"]
        if name.startswith(("TestArgsToInput.", "TestUnwrapJobOutput.", "TestLiveTarget.test_unwrap_")):
            target = "apps_protocol::python_invocation_helpers_and_pickle_values"
        covered = {
            "test_fast_invocation_returns_without_polling":"test_fast_invocation_returns_without_polling",
            "test_sync_wait_hands_off_the_same_job":"test_sync_wait_hands_off_the_same_job",
            "TestWaitTerminal.test_immediate_terminal":"test_fast_invocation_returns_without_polling",
            "TestWaitTerminal.test_polls_to_completion":"test_sync_wait_hands_off_the_same_job",
            "TestWaitTerminal.test_no_job_id_raises":"test_stream_failed_and_wait_missing_id_or_timeout",
            "TestWaitTerminal.test_timeout":"test_stream_failed_and_wait_missing_id_or_timeout",
            "TestSentinelTarget.test_job_operations":"test_job_cancel_and_retry_update_state",
            "TestSentinelTarget.test_stream_job":"test_stream_job_unwraps_chunks_and_stops_on_terminal_page",
            "TestSentinelTarget.test_stream_job_failed":"test_stream_failed_and_wait_missing_id_or_timeout",
            "TestLiveTarget.test_payload_carries_source":"test_live_payload_and_concurrent_requests_share_completed_source_sync",
            "TestLiveTarget.test_payload_args_are_plain_json":"test_live_payload_and_concurrent_requests_share_completed_source_sync",
            "TestLiveTarget.test_stream_job_unwraps_chunks":"test_stream_job_unwraps_chunks_and_stops_on_terminal_page",
            "TestLiveTarget.test_concurrent_requests_share_completed_source_sync":"test_live_payload_and_concurrent_requests_share_completed_source_sync",
            "TestLiveTarget.test_failed_source_sync_prevents_execution_and_can_retry":"test_failed_source_sync_prevents_execution_and_can_retry",
        }
        if name in covered:
            target = "apps_invocation::" + covered[name]
    if entry["file"] == "tests/test_apps/test_job.py":
        covered = {"test_job_status_polls_target":"test_job_cancel_and_retry_update_state", "test_job_status_skips_poll_once_terminal":"test_job_cancel_and_retry_update_state", "test_job_result_reaches_terminal_state":"test_job_result_reaches_terminal_state_and_output_alias", "test_job_output_aliases_result":"test_job_result_reaches_terminal_state_and_output_alias", "test_job_cancel_and_retry_update_state":"test_job_cancel_and_retry_update_state", "test_job_stream_async":"test_stream_job_unwraps_chunks_and_stops_on_terminal_page"}
        if entry["name"] in covered:
            target = "apps_invocation::" + covered[entry["name"]]
        if entry["name"] == "test_job_stream_sync":
            target = "apps_stubs::blocking_stubs_and_jobs_share_runtime_after_client_is_dropped"
    if entry["file"] == "tests/test_apps/test_retry.py":
        covered = {"test_retries_transient_5xx_then_succeeds":"transient_reads_retry_but_client_errors_and_invalid_json_do_not", "test_404_not_retried_maps_to_endpoint_not_found":"test_404_not_retried_maps_to_endpoint_not_found", "test_job_404_retried_during_propagation":"test_job_404_retried_during_propagation", "test_persistent_job_404_exhausts_retries":"persistent_job_failures_exhaust_seven_attempts", "test_client_4xx_not_retried":"transient_reads_retry_but_client_errors_and_invalid_json_do_not", "test_submission_is_not_repeated_after_ambiguous_failure":"test_submission_is_not_repeated_after_ambiguous_failure / disconnected_submissions_are_not_replayed", "test_sync_transport_timeout_does_not_resubmit":"sync_transport_timeout_does_not_resubmit"}
        if entry["name"] in covered:
            target = "apps_invocation::" + covered[entry["name"]]
    if entry["file"] == "tests/test_apps/test_dispatch.py":
        covered = {"TestStubs.test_queue_stub_remote":"stub_options_accumulate_without_changing_original", "TestStubs.test_queue_stub_with_options":"stub_options_accumulate_without_changing_original", "TestStubs.test_queue_stub_spawn_returns_job":"stub_spawn_reconnect_and_lazy_stream_use_sentinel", "TestStubs.test_queue_stub_stream":"stub_spawn_reconnect_and_lazy_stream_use_sentinel", "TestStubs.test_queue_stub_reconnects_to_job":"stub_spawn_reconnect_and_lazy_stream_use_sentinel", "TestStubs.test_api_stub_http":"api_stub_routes_all_verbs_with_headers_and_json_or_text"}
        if entry["name"] in covered:
            target = "apps_stubs::" + covered[entry["name"]]
    if entry["file"] == "tests/test_apps/test_logs.py":
        name = entry["name"]
        if name == "TestStreamPodLogs.test_connection_timeout_propagates":
            target = "apps_logs::socket_timeout_before_headers_propagates_even_with_idle_timeout"
        if name.startswith("TestTailSummary."):
            target = "apps_monitor::python_monitor_fixtures / apps_logs::snapshot_normalizes_nulls_and_tail_preserves_order_and_zero_quirk"
        covered = {"TestPodLogs.test_snapshot_normalizes_nulls":"snapshot_normalizes_nulls_and_tail_preserves_order_and_zero_quirk", "TestStreamPodLogs.test_combined_rest_stream_resumes_delivered_events":"stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery", "TestStreamPodLogs.test_parses_sse_frames":"stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery", "TestStreamPodLogs.test_cursor_matches_each_complete_frame":"stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery", "TestStreamPodLogs.test_multiline_data_and_incomplete_frames":"stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery", "TestStreamPodLogs.test_idle_timeout_starts_after_response_headers":"idle_timeout_starts_after_headers_and_http_errors_propagate", "TestStreamPodLogs.test_idle_timeout_closes_established_stream":"fragmented_utf8_and_idle_shutdown_close_the_connection", "TestStreamPodLogs.test_rest_stream_selects_resource_source_and_window":"stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery"}
        if name in covered:
            target = "apps_logs::" + covered[name]
    if entry["file"] == "tests/test_apps/test_monitor.py":
        name = entry["name"]
        if name.startswith(("TestFormatWorkerCounts.", "TestReportCounts.", "TestLineFiltering.")):
            target = "apps_monitor::python_monitor_fixtures"
        covered = {"TestOnStatus.test_worker_id_triggers_ready_and_stream":"monitor_reports_counts_attaches_each_worker_once_and_stops_pool_updates", "TestPodLogStream.test_follow_emits_and_dedups":"follow_emits_deduplicates_and_attach_is_idempotent", "TestPodLogStream.test_stop_snapshots_when_stream_never_attached":"stop_snapshots_without_a_stream_and_filters_old_and_runtime_lines", "TestPodLogStream.test_snapshot_skips_lines_before_since":"stop_snapshots_without_a_stream_and_filters_old_and_runtime_lines", "TestPodLogStream.test_attach_idempotent":"follow_emits_deduplicates_and_attach_is_idempotent", "TestMonitorLifecycle.test_metrics_polling_reports_counts":"monitor_reports_counts_attaches_each_worker_once_and_stops_pool_updates"}
        if name in covered:
            target = "apps_monitor::" + covered[name]
    if entry["file"] == "tests/test_apps/test_images.py":
        name = entry["name"]
        if name in ("TestRuntimeImage.test_full_matrix_resolves", "TestRuntimeImage.test_unsupported_python_rejected", "TestImageForSpec.test_custom_image_wins", "TestImageForSpec.test_cpu_spec_gets_cpu_image", "TestImageForSpec.test_gpu_spec_gets_gpu_image", "TestLocalPythonVersion.test_supported_range_policy"):
            target = "apps_tasks::python_task_payload_and_runtime_fixtures"
        if name in ("TestRuntimeImage.test_tag_channel_from_env", "TestRuntimeImage.test_default_channel_is_latest"):
            target = "apps_tasks::runtime_image_channel_environment_is_captured_without_global_mutation"
    if entry["file"] == "tests/test_apps/test_deploy.py" and entry["name"] in ("TestManifest.test_manifest_shape", "TestManifest.test_schedule_blocked_until_backend_support", "TestEndpointInput.test_custom_api_declares_runtime_port"):
        target = "apps_deploy::python_deployment_manifest_and_endpoint_fixtures"
    if entry["file"] == "tests/test_apps/test_config_params.py" and entry["name"] in ("TestDeployPayload.test_config_params_reach_payload", "TestDeployPayload.test_flashboot_default_present", "TestDeployPayload.test_flashboot_disabled_omitted"):
        target = "apps_deploy::python_deployment_manifest_and_endpoint_fixtures"
    if entry["file"] == "tests/test_apps/test_shim.py" and entry["name"] in ("test_starts_an_installed_runtime", "test_installs_missing_packages_and_honors_overrides"):
        target = "apps_shim::launcher_starts_installed_runtimes_installs_missing_packages_and_honors_overrides [Unix] / apps_tasks::python_task_payload_and_runtime_fixtures [all platforms]"
    if entry["file"] == "tests/test_apps/test_tasks.py":
        name = entry["name"]
        if name.startswith(("TestPodInput.", "TestUnwrapTaskResponse.", "TestGpuPoolExpansion.")):
            target = "apps_tasks::python_task_payload_and_runtime_fixtures"
        covered = {
            "TestPodTargetPayload.test_payload_carries_source_and_args":"pod_target_payload_and_completion_events_follow_cleanup",
            "TestPodTargetPayload.test_task_spawn_returns_job":"canceled_target_creation_and_wait_continue_cleanup_in_the_background",
            "TestPollResult.test_retries_transient_404_then_returns_result":"runner_404_retries_are_bounded_and_other_errors_are_not_replayed",
            "TestPollResult.test_persistent_404_raises":"runner_404_retries_are_bounded_and_other_errors_are_not_replayed",
            "TestPollResult.test_running_returns_none":"running_task_then_cancel_caches_null_and_unlimited_execute_submits_null_timeout",
            "TestTaskExecutionLifecycle.test_start_deploys_pod":"task_job_reads_caches_and_cleans_up_with_separate_runner_authentication",
            "TestTaskExecutionLifecycle.test_start_retries_datacenters_after_capacity_error":"capacity_fallback_tries_datacenters_and_preserves_the_initial_error",
            "TestTaskExecutionLifecycle.test_start_does_not_retry_non_capacity_error":"setup_failures_clean_up_and_non_capacity_creation_is_never_retried",
            "TestTaskExecutionLifecycle.test_shared_volume_placement_accounts_for_siblings":"shared_volume_placement_accounts_for_siblings_and_ignores_unrelated_resources",
            "TestTaskExecutionLifecycle.test_start_resolves_registry_auth":"task_start_resolves_registry_and_attaches_existing_network_storage",
            "TestTaskExecutionLifecycle.test_failed_termination_retains_id_for_retry":"failed_delete_retains_the_pod_id_for_manual_retry_and_not_found_is_success",
            "TestTaskExecutionLifecycle.test_terminate_noop_without_pod":"failed_delete_retains_the_pod_id_for_manual_retry_and_not_found_is_success",
            "TestTaskExecutionLifecycle.test_execute_polls_to_done":"running_task_then_cancel_caches_null_and_unlimited_execute_submits_null_timeout",
            "TestTaskTimeoutTransport.test_runtime_must_acknowledge_timeout":"task_timeout_validation_and_acknowledgement_prevent_silent_runtime_mismatch",
            "TestTaskTimeoutTransport.test_invalid_timeouts_fail_before_http":"task_timeout_validation_and_acknowledgement_prevent_silent_runtime_mismatch",
            "TestTaskJob.test_wait_returns_result_and_terminates":"task_job_reads_caches_and_cleans_up_with_separate_runner_authentication",
            "TestTaskJob.test_wait_sends_timeout_to_runtime":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
            "TestTaskJob.test_timeout_configuration_failure_terminates_pod":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
            "TestTaskJob.test_wait_timeout_terminates_pod":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
            "TestTaskJob.test_poll_failure_terminates_pod":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
            "TestTaskJob.test_failed_result_terminates_pod":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
            "TestTaskJob.test_wait_after_done_returns_cached":"task_job_reads_caches_and_cleans_up_with_separate_runner_authentication",
            "TestTaskJob.test_cancel_terminates":"running_task_then_cancel_caches_null_and_unlimited_execute_submits_null_timeout",
            "TestSpawnCleanup.test_setup_failure_terminates_pod":"setup_failures_clean_up_and_non_capacity_creation_is_never_retried",
            "TestCancellationCleanup.test_cancel_during_creation_recovers_and_deletes_pod":"canceled_target_creation_and_wait_continue_cleanup_in_the_background / canceling_creation_still_records_the_late_pod_for_cleanup",
            "TestCancellationCleanup.test_failed_cleanup_preserves_error_and_recoverable_pod":"job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure",
        }
        if name in covered:
            target = "apps_tasks::" + covered[name]
    if target:
        entry.update(rust=target, status="ported")
path.write_text(json.dumps(tests, indent=2) + "\n")
print(f"Mapped {sum(e['status']=='implemented' for e in api['entries'])} implemented and {sum(e['status']=='adapted' for e in api['entries'])} adapted symbols, and {sum(e['status']=='ported' for e in tests['entries'])} tests; remaining entries retained as pending")
