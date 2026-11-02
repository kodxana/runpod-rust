"""Map native registry and secret commands and their upstream test counterparts."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
mapping = {kind: "cli::credentials::Kind::command" for kind in ("registry", "secret")}
mapping.update({f"{kind}_{verb}": "cli::credentials::Operation::{read,execute}" for kind in ("registry", "secret") for verb in ("add", "list", "delete")})
count = 0
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/main.py" and entry["name"] in mapping:
        entry.update(rust=mapping[entry["name"]], status="adapted", note="Native registry/secret CLI using existing Apps transports, hidden prompts, sorted output and exact first-name deletion after confirmation. Explicit empty values and false secretDelete success are preserved. Terminal prompts, plain rendering and typed diagnostics are native adaptations; see reviews/credential-cli.md.")
        count += 1
assert count == 8, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))

path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
cases = json.loads((ROOT / "tests/fixtures/credential_cli.json").read_text())
names = {case["name"] for case in cases if not case["name"].startswith("characterization.")}
count = 0
for entry in data["entries"]:
    if entry["file"] != "tests/test_cli/test_management_commands.py" or entry["name"] not in names:
        continue
    cls, name = entry["name"].split(".")
    kind = cls.removeprefix("Test").removesuffix("Commands").lower()
    entry.update(rust=f"cli::credential_tests::test_{kind}_{name.removeprefix('test_')}", status="ported", note="Named native counterpart replaying captured API requests, output, prompts and confirmation. Ten unchanged source tests plus 34 source characterizations yield 44 invocations; 31 also replay through the native executables. Hidden-input and confirmation terminal checks use loopback APIs and synthetic values.")
    count += 1
assert count == 10, count
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
