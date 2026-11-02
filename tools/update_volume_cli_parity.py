"""Update reviewed volume commands and their shared CLI helpers only."""
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
volume = {
    "_label": "cli::volume::label", "_choices": "cli::volume::choices",
    "_show": "cli::volume::Options::show", "_data_center": "cli::volume::select_data_center",
    "_growth": "cli::volume::Growth::{parse,resolve}", "volume": "cli::volume::command",
    **{f"volume_{name}": f"cli::volume::Options::execute / Action::{name.title()}" for name in ["list", "get", "create", "update", "expand", "delete"]},
}
output = {"print_fields": "cli::output::fields", "interactive": "cli::interaction::Terminal::interactive",
          "pick": "cli::interaction::Interaction::pick", "confirm": "cli::interaction::Interaction::confirm"}
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    file, name = entry["file"], entry["name"]
    target = volume.get(name) if file == "runpod/rp_cli/volume.py" else output.get(name) if file == "runpod/rp_cli/output.py" else None
    if file == "runpod/rp_cli/api.py": target = "cli::api::list_all" if name == "list_all" else "cli::CliError / cli::run"
    if target:
        entry.update(rust=target, status="implemented" if name in {"_label", "interactive"} else "adapted",
                     note="Native CLI with owned client, typed usage/API/abort errors, terminal fuzzy selection and confirmation. Requests, growth checks and output values preserve Python contracts; display and bounded numeric adaptations are in reviews/volume-cli.md. Shell completion is still pending.")
path.write_text(json.dumps(data, indent=2) + "\n")
print("API", dict(Counter(e["status"] for e in data["entries"])))
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
interactive = {"test_get_picker", "test_create_filters_picker_candidates", "test_delete_confirmation"}
output_tests = {"test_print_fields", "test_pick_requires_tty", "test_pick_with_no_choices", "test_pick_uses_fuzzy_prompt", "test_confirm", "test_pick_truncates_labels_to_terminal_cells"}
adapted = {"test_api_errors_are_clean", "test_api_errors_are_click_errors", "test_pick_uses_fuzzy_prompt", "test_pick_truncates_labels_to_terminal_cells"}
for entry in data["entries"]:
    file, name = entry["file"], entry["name"].split(".")[-1]
    target = None
    if file == "tests/test_cli/test_volume_command.py" and not name.startswith("test_completion_"):
        target = ("cli::volume::tests::" if name in interactive else "cli_volume::") + name
    elif file == "tests/test_cli/test_cli_api.py": target = "cli::api::tests::" + name
    elif file == "tests/test_cli/test_cli_output.py" and name in output_tests:
        target = ("cli::output::tests::" if name == "test_print_fields" else "cli::interaction::tests::") + name
    if target:
        entry.update(rust=target, status="adapted" if name in adapted else "ported",
                     note="Named native counterpart; actual Python fixtures compare requests, payloads, choices, fields and errors. Native terminal backend replaces InquirerPy/Click. HTTP 401 tests use the source transport's fixed authentication error; completion tests remain pending.")
path.write_text(json.dumps(data, indent=2) + "\n")
print("Tests", dict(Counter(e["status"] for e in data["entries"])))
