"""Map deployment CLI coverage without claiming the remaining Flash commands."""
import hashlib
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
API = {
    'runpod/rp_cli/main.py': {
        'deploy': 'cli::deployment::{Options,run_operation,observe}',
        '_app_source': 'cli::deployment::source_name',
    },
    'runpod/rp_cli/console.py': {
        'deploy_plan': 'cli::deployment::plan',
        'deploy_banner': 'cli::deployment::banner',
        '_bar': 'cli::deployment::progress::bar',
        'DeployEvents': 'cli::deployment::progress::Progress',
        'DeployEvents.__init__': 'cli::deployment::progress::Progress::default',
        'DeployEvents._ensure_started': 'cli::deployment::observe',
        'DeployEvents._finish_current': 'cli::deployment::progress::Progress::close',
        'DeployEvents.phase': 'cli::deployment::progress::Progress::event',
        'DeployEvents.vendor_progress': 'cli::deployment::progress::Progress::event',
        'DeployEvents.upload_progress': 'cli::deployment::progress::Progress::event',
        'DeployEvents.endpoint_ready': 'cli::deployment::progress::Progress::event',
        'DeployEvents.close': 'cli::deployment::progress::Progress::close',
    },
}
note = ('Native Cargo discovery/build and queue/API deployment; explicit --runtime python '
        'retains the Python artifact pipeline with compiled metadata. Owned terminal progress '
        'replaces Rich. Cross-target tools are caller-installed; native task/cloud validation '
        'and other Flash commands remain pending. See reviews/deploy-cli.md and parity/deploy-cli.json.')
path = ROOT / 'parity/api.json'
data = json.loads(path.read_text(encoding='utf-8'))
selected = [e for e in data['entries'] if e['name'] in API.get(e['file'], {})]
assert len(selected) == 14
for entry in selected:
    entry.update(rust=API[entry['file']][entry['name']], status='adapted', note=note)
for entry in data['entries']:
    if entry['file'] == 'runpod/rp_cli/main.py' and entry['name'] in {'app', 'app_list', 'app_get', 'app_delete'}:
        entry['note'] = entry['note'].replace(
            'Discovery, deployment and Rich console classes remain pending;',
            'Discovery and deployment are covered by later milestones; remaining console helpers stay pending;')
path.write_text(json.dumps(data, indent=2) + '\n', encoding='utf-8')
print('API', dict(Counter(e['status'] for e in data['entries'])))

fixture = ROOT / 'tests/fixtures/deploy_cli.json'
cases = json.loads(fixture.read_text(encoding='utf-8'))
command_names = {c['name'] for c in cases}
assert len(command_names) == 9
console_names = {
    'TestHelpers.test_bar_bounds',
    'TestBanners.test_deploy_plan',
    'TestBanners.test_deploy_banner',
    'TestBanners.test_deploy_banner_no_resources',
    'TestDeployEvents.test_phase_lifecycle',
    'TestDeployEvents.test_progress_updates_ignored_outside_phase',
}
unit_commands = {'test_exclude_passthrough', 'test_python_version_fallback_warns'}
path = ROOT / 'parity/tests.json'
data = json.loads(path.read_text(encoding='utf-8'))
selected = []
for entry in data['entries']:
    command = entry['file'] == 'tests/test_cli/test_deploy_command.py' and entry['name'] in command_names
    console = entry['file'] == 'tests/test_cli/test_console.py' and entry['name'] in console_names
    if not (command or console):
        continue
    name = entry['name'].split('.')[-1]
    process = command and name not in unit_commands
    adapted = process or entry['name'].startswith(('TestDeployEvents.', 'TestHelpers.'))
    rust = ('cli_deploy::' if process else 'cli::deployment::tests::') + name
    entry.update(rust=rust, status='adapted' if adapted else 'ported', note=(
        'Named Rust counterpart; nine unchanged command invocations captured for arguments, '
        'banners and error codes. Native process cases use compiled Cargo metadata and a '
        'local HTTP backend; native progress replaces Rich styling. Six unchanged console '
        'tests also pass. See reviews/deploy-cli.md.'))
    selected.append(entry)
assert len(selected) == 15
path.write_text(json.dumps(data, indent=2) + '\n', encoding='utf-8')
print('Tests', dict(Counter(e['status'] for e in data['entries'])))

ledger = {
    'upstream': data['upstream'],
    'commit': data['commit'],
    'api_mapping': API,
    'test_mapping': {e['file'] + '::' + e['name']: e['rust'] for e in selected},
    'source_capture': {
        'file': 'tests/fixtures/deploy_cli.json',
        'cases': len(cases),
        'sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
        'generator': 'tools/deploy_cli_fixtures.py',
        'normalization': 'Project path replaced by <PROJECT>; elapsed live time by <ELAPSED>s',
    },
    'source_validation': {'command_cases': 9, 'console_cases': 6, 'discovery_regressions': 11},
    'additional_native_tests': ['tests/apps_discovery.rs', 'tests/cli_deploy.rs', 'src/cli/deployment_tests.rs'],
    'adaptations': [
        'Cargo declarations replace Python imports; package roots own assets in workspaces',
        'Native is the default; --runtime python selects legacy source/wheel artifact building',
        'Cargo build --locked with explicit target/profile and required features; parse compiler-artifact executable, never run the deployment binary',
        'Caller installs target standard library, linker and native dependencies; --worker bypasses deployment compilation',
        'Terminal spinner frames and plain glyphs replace Rich colors; redirected output prints final phase summaries',
        'Zero upload totals render empty progress instead of a suppressed source callback exception',
        'Source quirks retained: sequential apps, no rollback, completed phase on failure, comma-split excludes, persistent progress counters',
        'Blocking packaging already running at cancellation finishes; later cloud operations do not run',
    ],
    'pending': ['Live Runpod/container validation', 'Native coordinator task deployment', 'Native live source loading/dispatch', 'Other Flash commands and remaining console helpers'],
}
(ROOT / 'parity/deploy-cli.json').write_text(json.dumps(ledger, indent=2) + '\n', encoding='utf-8')
