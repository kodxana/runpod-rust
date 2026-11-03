"""Record job snapshot parity separately from pending heartbeat execution."""
import hashlib
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
mapping = {
    'JobsProgress': 'serverless::JobsProgress',
    'JobsProgress.__new__': 'serverless::JobsProgress::{new,clone}',
    'JobsProgress.__init__': 'serverless::JobsProgress::new',
    'JobsProgress.set_mirror': 'serverless::JobsProgress::set_mirror',
    'JobsProgress._notify_mirror': 'serverless::state::State::notify_mirror',
    'PingJobMirror': 'serverless::PingJobMirror',
    'PingJobMirror.__init__': 'serverless::PingJobMirror::{new,with_capacity}',
    'PingJobMirror.set': 'serverless::PingJobMirror::set',
    'PingJobMirror.get': 'serverless::PingJobMirror::get',
}
path = ROOT / 'parity/api.json'
data = json.loads(path.read_text())
selected = [e for e in data['entries'] if e['file'] == 'runpod/serverless/modules/worker_state.py' and e['name'] in mapping]
assert len(selected) == 9
for entry in selected:
    entry.update(rust=mapping[entry['name']], status='adapted', note=(
        'Owned trackers replace a process singleton; explicit clones share one tracker. '
        'Bounded thread-shared mirror retains source byte truncation, NUL and invalid UTF-8 '
        'semantics. Attach/add/remove/clear publish ordered snapshots under the mutation lock. '
        'Heartbeat execution/retries remain separate and pending. See reviews/job-mirror.md.'))
path.write_text(json.dumps(data, indent=2) + '\n')
print('API', dict(Counter(e['status'] for e in data['entries'])))
path = ROOT / 'parity/tests.json'
data = json.loads(path.read_text())
selected = [e for e in data['entries'] if e['file'] == 'tests/test_serverless/test_integration_worker_state.py']
assert len(selected) == 13
for entry in selected:
    name = entry['name'].split('.')[-1]
    adapted = name in {'test_singleton_identity', 'test_mirror_roundtrip_across_process'}
    if name == 'test_mirror_roundtrip_across_process':
        name = 'test_mirror_roundtrip_across_threads'
    entry.update(rust='worker_state::' + name, status='adapted' if adapted else 'ported', note=(
        'Named native counterpart; explicit clones replace singleton identity and a native '
        'thread replaces the Python IPC child. A real process verifies no writes in its cwd. '
        'Six captured mirror sequences cover byte boundaries. See reviews/job-mirror.md.'))
path.write_text(json.dumps(data, indent=2) + '\n')
print('Tests', dict(Counter(e['status'] for e in data['entries'])))
fixture = ROOT / 'tests/fixtures/job_mirror.json'
ledger = {
    'upstream': data['upstream'], 'commit': data['commit'],
    'file': 'runpod/serverless/modules/worker_state.py',
    'api_mapping': mapping,
    'test_mapping': {e['name']: e['rust'] for e in selected},
    'capture': {'file': 'tests/fixtures/job_mirror.json', 'sequences': 6,
                'sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
                'generator': 'tools/job_mirror_fixtures.py'},
    'adaptations': [
        'Owned tracker identity and explicit clones replace global singleton membership',
        'Native heartbeat design uses a thread-shared buffer, not OS process-shared memory',
        'Tracker IDs have deterministic lexical order; Python set iteration is unspecified',
        'Best-effort poisoned-lock failures use native diagnostics; zero capacity always reads empty',
        'Mutation and mirror publication share one lock, preventing stale concurrent publication',
    ],
    'preserved_quirks': ['Reserve one NUL byte', 'Overflow cuts at the last available comma, even after a complete ID',
                        'No comma means a possibly partial ID or UTF-8 prefix', 'Invalid UTF-8 reads None',
                        'Embedded NUL ends visible text', 'Duplicate add and missing remove still notify',
                        'Replaced/detached mirrors retain their last snapshot'],
    'pending': ['Heartbeat independent execution and retries', 'Process-shared mirror API is adapted to native threads'],
}
(ROOT / 'parity/job-mirror.json').write_text(json.dumps(ledger, indent=2) + '\n')
