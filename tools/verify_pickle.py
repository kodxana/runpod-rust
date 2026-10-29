"""Run after cargo test --test apps_protocol to check Rust output in Python."""
import base64
import json
from pathlib import Path
import cloudpickle

cases=json.loads((Path(__file__).resolve().parents[1]/"test-results/rust-pickle.json").read_text(encoding="utf-8"))
for case in cases:
    assert cloudpickle.loads(base64.b64decode(case["encoded"]))==case["expected"],case
print(f"Python decoded all {len(cases)} Rust pickle values correctly")
