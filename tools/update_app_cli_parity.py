"""Map the app management CLI without claiming discovery or deployment coverage."""
import json
from collections import Counter
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
path=ROOT/'parity/api.json'
data=json.loads(path.read_text())
mapping={'app':'cli::flash::command','app_list':'cli::flash::Operation::execute (List)',
         'app_get':'cli::flash::Operation::execute (Get)','app_delete':'cli::flash::Operation::execute (Delete)'}
selected=[entry for entry in data['entries'] if entry['file']=='runpod/rp_cli/main.py' and entry['name'] in mapping]
assert len(selected)==4
for entry in selected:
    entry.update(rust=mapping[entry['name']],status='adapted',note='Native flash app list/get/delete with SDK management calls, exact data output, ordered cleanup failures and confirmation before credentials. Plain progress lines, native terminal prompts and typed malformed-response errors adapt Click/Rich. Discovery, deployment and Rich console classes remain pending; see reviews/app-cli.md.')
path.write_text(json.dumps(data,indent=2)+'\n')
print('API',dict(Counter(entry['status'] for entry in data['entries'])))

path=ROOT/'parity/tests.json'
data=json.loads(path.read_text())
names={case['name'] for case in json.loads((ROOT/'tests/fixtures/app_cli.json').read_text()) if case['name'].startswith('TestAppCommands.')}
count=0
for entry in data['entries']:
    if entry['file']!='tests/test_cli/test_management_commands.py' or entry['name'] not in names: continue
    entry.update(rust=f'cli::flash::tests::{entry["name"].split(".")[-1]}',status='ported',note='Unchanged source tests captured and replayed with exact management calls, confirmation, output and exit/error assertions. Fifteen transport characterizations additionally test both executables, cleanup ordering and partial failure; see reviews/app-cli.md.')
    count+=1
assert count==7,count
path.write_text(json.dumps(data,indent=2)+'\n')
print('Tests',dict(Counter(entry['status'] for entry in data['entries'])))
