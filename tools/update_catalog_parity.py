"""Update only the reviewed catalog CLI and shared rendering mappings."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
catalog = {
    "_money": "cli::catalog::money / cli::output::general",
    "_stock": "cli::catalog::stock_cells",
    "_filter_stock": "cli::catalog::Options::filter_stock",
    "_unit_price": "cli::catalog::Options::unit_price",
    "_gpu_rows": "cli::catalog::Options::gpu_rows",
    "_gpu_columns": "cli::catalog::Options::{gpu_headers,gpu_cells}",
    "_cpu_rows": "cli::catalog::Options::cpu_rows",
    "_cpu_columns": "cli::catalog::Options::cpu_cells",
    "catalog": "cli::{run,catalog::Options} / rp and runpod binaries",
}
implemented = {"_money", "_stock", "_filter_stock", "_unit_price"}
output = {"print_json": "json", "cell": "cell", "table": "table", "print_table": "table"}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] == "runpod/rp_cli/catalog.py":
        name = entry["name"]
        entry.update(rust=catalog[name], status="implemented" if name in implemented else "adapted",
                     note="Native Clap CLI preserves catalog filtering, query parameters, row order, values and errors. Sized counts and native table/help layout are documented in reviews/catalog.md and PORTING.md.")
    elif entry["file"] == "runpod/rp_cli/output.py" and entry["name"] in output:
        entry.update(rust="cli::output::" + output[entry["name"]], status="adapted",
                     note="Native JSON-value rendering and unwrapped Unicode-width tables; no Python object repr/default=str or Rich object/style emulation. Other output/prompt/completion helpers remain pending.")
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] == "tests/test_cli/test_catalog_command.py" and not entry["name"].startswith("TestCompletion."):
        entry.update(rust="cli_catalog::" + entry["name"].split(".")[-1], status="ported",
                     note="Named native executable test replays actual Python JSON, table cells, exit codes, errors and complete HTTP query maps; Rich visual layout is an explicit adaptation.")
    elif entry["file"] == "tests/test_cli/test_cli_output.py" and entry["name"] in {"TestOutputHelpers.test_cell", "TestOutputHelpers.test_print_table_keeps_ids_and_values_whole"}:
        name = entry["name"].split(".")[-1]
        entry.update(rust="cli::output::tests::" + name, status="ported" if name == "test_cell" else "adapted",
                     note="Named native counterpart preserves cell values and whole identifiers; native tables are unwrapped and do not emulate Rich terminal folding.")
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
