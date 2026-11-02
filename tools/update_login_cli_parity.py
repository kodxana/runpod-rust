"""Map login CLI authentication, prompt and credential persistence coverage."""
import json
from collections import Counter
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
path=ROOT/'parity/api.json'
data=json.loads(path.read_text())
selected=[e for e in data['entries'] if e['file']=='runpod/rp_cli/main.py' and e['name']=='login']
assert len(selected)==1
selected[0].update(rust='cli::login::Options::{read,run,execute}',status='adapted',note='Native direct-key, browser and email-code login with anonymous transports, hidden validation/retry prompts, interruption and atomic selected-profile writes. Native terminal/browser launch and diagnostics are adaptations; see reviews/login-cli.md.')
path.write_text(json.dumps(data,indent=2)+'\n')
print('API',dict(Counter(e['status'] for e in data['entries'])))

path=ROOT/'parity/tests.json'
data=json.loads(path.read_text())
names={case['name'] for case in json.loads((ROOT/'tests/fixtures/login_cli.json').read_text()) if not case['name'].startswith('characterization.')}
count=0
for entry in data['entries']:
    if entry['file'] not in {'tests/test_cli/test_agent_login.py','tests/test_cli/test_management_commands.py'} or entry['name'] not in names: continue
    name=entry['name'].split('.')[-1]
    platform=name=='test_agent_saves_named_profile_and_hides_code_and_key'
    entry.update(rust=f'cli::login::tests::{name}',status='adapted' if platform else 'ported',note='Captured source login invocation replaying authentication callbacks, hidden code prompts, stdout/stderr and selected-profile persistence; native process tests and loopback terminal checks cover real SDK transport. '+('Source functional assertions pass on Windows before its Unix mode assertion fails (0666 vs 0600); Rust checks 0600 only on Unix. ' if platform else '')+'18 source invocations plus 11 characterizations regenerate deterministically. See reviews/login-cli.md.')
    count+=1
assert count==11,count
path.write_text(json.dumps(data,indent=2)+'\n')
print('Tests',dict(Counter(e['status'] for e in data['entries'])))
