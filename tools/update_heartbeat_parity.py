"""Record the independent native heartbeat and its captured HTTP retry behavior."""
import hashlib
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    'Heartbeat': 'serverless::Heartbeat',
    'Heartbeat.__init__': 'serverless::Heartbeat::{new,with_options}',
    'Heartbeat.process_loop': 'serverless::Heartbeat::start_ping (private owned thread/runtime)',
    'Heartbeat.start_ping': 'serverless::Heartbeat::start_ping',
    'Heartbeat.ping_loop': 'serverless::Heartbeat::{ping_loop,run_once}',
    'Heartbeat._send_ping': 'serverless::Heartbeat::send_ping',
}
path = ROOT / 'parity/api.json'
data = json.loads(path.read_text())
selected = [e for e in data['entries'] if e['file'] == 'runpod/serverless/modules/rp_ping.py']
assert len(selected) == 6
for entry in selected:
    entry.update(rust=mapping[entry['name']], status='adapted', note=(
        'Owned native thread/runtime and cancellation guard replace the daemon process and '
        'global started flag. Queue/HTTP workers share a bounded snapshot with the heartbeat. '
        'GET retries, backoff, Retry-After, interval flooring and parent-option reset are preserved. '
        'Explicit send returns errors; periodic loops log and continue. See reviews/heartbeat.md.'))
for entry in data['entries']:
    if entry['file'] == 'runpod/serverless/modules/worker_state.py' and 'note' in entry:
        entry['note'] = entry['note'].replace(
            'Heartbeat execution/retries remain separate and pending.',
            'Independent heartbeat execution/retries are covered by parity/heartbeat.json.')
path.write_text(json.dumps(data, indent=2) + '\n')
print('API', dict(Counter(e['status'] for e in data['entries'])))
path = ROOT / 'parity/tests.json'
data = json.loads(path.read_text())
selected = [e for e in data['entries'] if e['file'] == 'tests/test_serverless/test_modules/test_ping.py']
assert len(selected) == 15
adapted = {'test_process_loop', 'test_start_ping_success', 'test_start_ping_already_started',
           'test_custom_pool_connections', 'test_send_ping_request_exception'}
for entry in selected:
    name = entry['name'].split('.')[-1]
    entry.update(rust='heartbeat::' + name, status='adapted' if name in adapted else 'ported', note=(
        'Named native counterpart; owned lifecycle, native pool accounting and typed send errors '
        'are documented adaptations. HTTP fixtures exercise the real pinned source session. '
        'See parity/heartbeat.json and reviews/heartbeat.md.'))
shared = [e for e in data['entries'] if e['file'] == 'tests/test_serverless/test_scale_mirror.py'
          and e['name'] == 'test_run_worker_attaches_one_shared_mirror']
assert len(shared) == 1
shared[0].update(rust='heartbeat_runtime::queue_heartbeat_survives_stalled_async_handler_and_stops_with_worker',
                 status='adapted', note='End-to-end worker test proves tracker mutations reach the independent heartbeat through the shared mirror.')
path.write_text(json.dumps(data, indent=2) + '\n')
print('Tests', dict(Counter(e['status'] for e in data['entries'])))
fixture = ROOT / 'tests/fixtures/heartbeat.json'
ledger = {
    'upstream': data['upstream'], 'commit': data['commit'],
    'api_mapping': mapping,
    'test_mapping': {e['name']: e['rust'] for e in selected + shared},
    'capture': {'file': 'tests/fixtures/heartbeat.json', 'cases': 9,
                'sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
                'generator': 'tools/heartbeat_fixtures.py', 'urllib3': '2.8.0'},
    'preserved_quirks': [
        'Environment milliseconds floor to whole seconds; explicit native durations retain their precision',
        'Ordinary nonretryable HTTP errors are accepted without raising',
        'One snapshot is held across every attempt in a logical ping',
        'First retry is immediate, then exponential delays of 2 and 4 seconds by default',
        'Retry-After takes priority; 413 retries only with that header and remaining budget',
        'Background startup reconstructs default pool/retry settings, discarding parent overrides',
        'The interval sleep follows request completion, including single-iteration mode',
    ],
    'adaptations': [
        'Owned native thread and Tokio runtime replace a Python daemon process',
        'One start per Heartbeat instance replaces global process-wide suppression',
        'Drop signals nonblocking cancellation; stop awaits runtime cleanup',
        'Low-level send returns typed errors; periodic loops log and continue',
        'Native maximum idle connections per host replaces requests host-pool caching limits',
        'Subsecond environment values produce an early typed error instead of requests zero-timeout failure',
        'WorkerConfig::from_values disables ping_url without a nonempty deployment ID',
        'Native TLS/proxy/user-agent behavior and rare malformed body/redirect failures remain reqwest behavior',
        'Retry-After uses HTTP-standard date parsing and the captured urllib3 2.8 six-hour cap',
        'The native SDK version is reported instead of the Python SDK version',
    ],
    'verification': ['Fifteen named source tests', 'Nine captured local HTTP scenarios',
                     'Stalled queue and realtime handler tests', 'Request/backoff/sleep cancellation tests',
                     'Dropped worker future and child-option reset tests'],
    'pending': ['Live Runpod/container validation'],
}
(ROOT / 'parity/heartbeat.json').write_text(json.dumps(ledger, indent=2) + '\n')
path = ROOT / 'parity/job-mirror.json'
mirror = json.loads(path.read_text())
mirror['pending'] = [item for item in mirror['pending'] if item != 'Heartbeat independent execution and retries']
mirror['heartbeat'] = 'parity/heartbeat.json'
path.write_text(json.dumps(mirror, indent=2) + '\n')
