"""Map environment CLI coverage while keeping full live-handle discovery pending."""
import json
from collections import Counter
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
path=ROOT/'parity/api.json'
data=json.loads(path.read_text())
mapping={'_fmt_ts':'cli::environment::timestamp','env':'cli::environment::command',
         '_resolve_app_name':'cli::environment::resolve_app_name',
         'env_list':'cli::environment::Operation::execute (List)',
         'env_get':'cli::environment::Operation::execute (Get)',
         'env_add':'cli::environment::Operation::execute (Add)',
         'env_delete':'cli::environment::Operation::execute (Delete)',
         'undeploy':'cli::environment::Operation::execute (Undeploy)'}
count=0
for entry in data['entries']:
    if entry['file']=='runpod/rp_cli/main.py' and entry['name'] in mapping:
        entry.update(rust=mapping[entry['name']],status='implemented' if entry['name']=='_fmt_ts' else 'adapted',note='Native environment inspection/create/delete and undeploy, with explicit names or unique Cargo-project metadata discovery. Ordered SDK cleanup, source output and error behavior are covered. Native terminal/progress/link rendering and compiled declarations adapt Click/Rich/imports; see reviews/environment-discovery.md.')
        count+=1
    elif entry['file']=='runpod/apps/discovery.py':
        if entry['name']=='DiscoveryError':
            entry.update(rust='apps::discovery::DiscoveryError',status='adapted',note='Native typed discovery errors separate invalid targets, process/metadata failures and interruption, preserving source errors when wrapped.')
            count+=1
        else:
            assert entry['status']=='pending'
            entry['note']='Compiled Cargo metadata discovery is available for CLI name resolution. Full source discovery remains pending: live callable handles, import/module identity and cache-invalidation semantics are not covered by metadata export. See reviews/environment-discovery.md.'
assert count==9,count
path.write_text(json.dumps(data,indent=2)+'\n')
print('API',dict(Counter(e['status'] for e in data['entries'])))

path=ROOT/'parity/tests.json'
data=json.loads(path.read_text())
names={case['name'] for case in json.loads((ROOT/'tests/fixtures/env_cli.json').read_text()) if not case['name'].startswith('wire.')}
count=0
for entry in data['entries']:
    if entry['file']=='tests/test_cli/test_management_commands.py' and entry['name'] in names:
        entry.update(rust=f'cli::environment::tests::{entry["name"].split(".")[-1]}',status='ported',note='Eight unchanged source tests replayed with management calls, output and failures; sixteen wire cases check both CLI executables. Actual Cargo projects cover automatic unique-name selection, ambiguity and bypass with --app. See reviews/environment-discovery.md.')
        count+=1
assert count==8,count
path.write_text(json.dumps(data,indent=2)+'\n')
print('Tests',dict(Counter(e['status'] for e in data['entries'])))
