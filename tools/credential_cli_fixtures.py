"""Capture registry/secret CLI behavior and wire requests from pinned Python."""
import asyncio
import copy
import json
import tempfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import click
import pytest
from click.testing import CliRunner
from rich.markup import render
from runpod.apps import api
from runpod.error import QueryError
from runpod.rp_cli import console as ui
from runpod.rp_cli.main import cli

ROOT = Path(__file__).resolve().parents[1]
REAL_CLIENT = api.AppsApiClient
OPERATIONS = ("create_secret", "list_secrets", "delete_secret",
              "create_registry_auth", "list_registry_auths", "delete_registry_auth")
CASES = []


async def wire(call):
    """Execute the actual SDK method with only its transport replaced."""
    requests = []
    operation = call["operation"]
    result = call.get("result")
    failure = call.get("error")

    async def rest(method, path, **kwargs):
        response = {"registries": result} if operation == "list_registry_auths" else result
        if failure:
            response = {"error": failure}
        requests.append(dict(method=method, path=path, body=kwargs.get("json"),
            params=kwargs.get("params", {}), status=400 if failure else 200, response=response))
        if failure:
            raise QueryError(failure)
        return response

    async def graphql(query, **kwargs):
        data = {"myself": {"secrets": result}} if operation == "list_secrets" else {
            "secretCreate" if operation == "create_secret" else "secretDelete": result}
        response = {"errors": [{"message": failure}]} if failure else {"data": data}
        body = {"query": query}
        if kwargs.get("variables"):
            body["variables"] = kwargs["variables"]
        requests.append(dict(method="POST", path="/graphql", body=body, params={},
                             status=200, response=response))
        if failure:
            raise QueryError(failure)
        return response

    with patch.object(api, "run_rest_request_async", rest), patch.object(api, "run_graphql_query_async", graphql):
        try:
            actual = await getattr(REAL_CLIENT("fixture-key"), operation)(*call["args"])
            assert not failure
            # Registry deletion always returns True after a successful response.
            assert actual == result or operation == "delete_registry_auth"
        except QueryError:
            assert failure
    return requests


def capture(name, invoke, *args, **kwargs):
    case = dict(name=name, args=list(args[1]), calls=[], prompts=[], confirmations=[], messages=[])
    client = api.AppsApiClient.return_value
    original_prompt, original_confirm, original_print = click.prompt, click.confirm, ui.console.print

    def prompt(message, **options):
        record = dict(message=message, hidden=options.get("hide_input", False))
        case["prompts"].append(record)
        result = original_prompt(message, **options)
        record["answer"] = result
        return result

    def confirm(message, **options):
        record = dict(message=message, answer=False)
        case["confirmations"].append(record)
        result = original_confirm(message, **options)
        record["answer"] = result
        return result

    def output(*values, **options):
        case["messages"].append(" ".join(render(str(value)).plain for value in values))
        return original_print(*values, **options)

    def recorder(operation, mock):
        async def call(*values):
            record = dict(operation=operation, args=list(values))
            case["calls"].append(record)
            if isinstance(mock.side_effect_original, Exception):
                record["error"] = str(mock.side_effect_original)
                raise mock.side_effect_original
            record["result"] = copy.deepcopy(mock.return_value)
            return mock.return_value
        return call

    with ExitStack() as stack:
        for operation in OPERATIONS:
            method = getattr(client, operation)
            if isinstance(method, AsyncMock):
                method.side_effect_original = method.side_effect
                stack.enter_context(patch.object(method, "side_effect", recorder(operation, method)))
        stack.enter_context(patch.object(click, "prompt", prompt))
        stack.enter_context(patch.object(click, "confirm", confirm))
        stack.enter_context(patch.object(ui.console, "print", output))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result = invoke(*args, **kwargs)
    case.update(code=result.exit_code, stdout=result.stdout, stderr=result.stderr)
    assert not result.exit_code or isinstance(result.exception, SystemExit), result.exception
    case["requests"] = [request for call in case["calls"] for request in asyncio.run(wire(call))]
    CASES.append(case)
    return result


class Capture:
    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.cls and item.cls.__name__ in {
            "TestRegistryCommands", "TestSecretCommands"}]

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item):
        invoke = CliRunner.invoke
        name = f"{item.cls.__name__}.{item.originalname}"
        with patch.object(CliRunner, "invoke", lambda runner, *args, **kwargs:
                          capture(name, lambda *a, **kw: invoke(runner, *a, **kw), *args, **kwargs)):
            yield


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="credential-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_management_commands.py"),
        "--basetemp", str(Path(scratch) / "pytest"), "--no-cov", "-q"], plugins=[Capture()])
if status:
    raise SystemExit(status)

# Source-characterized cases cover the untested prompts, ordering, duplicates and errors.
for kind in ("registry", "secret"):
    for scenario in ("empty", "sorted", "prompts", "partial_prompt", "cancel", "confirm",
                     "prompt_abort", "hidden_abort", "first_match", "case_sensitive", "empty_values", "false_delete",
                     "add_error", "list_error", "delete_error", "flaglike_values", "repeated_values"):
        client = MagicMock()
        rows = [{"name": "zulu", "id": "z", "description": "last", "value": "never-print-me"},
                {"name": "tok", "id": "first/id", "description": "token"},
                {"name": "alpha", "id": "a", "description": None},
                {"name": "tok", "id": "duplicate", "description": "second"}]
        create, listing, delete = (("create_registry_auth", "list_registry_auths", "delete_registry_auth")
                                  if kind == "registry" else ("create_secret", "list_secrets", "delete_secret"))
        for operation, result in ((create, {"id": "new"}), (listing, rows), (delete, True)):
            setattr(client, operation, AsyncMock(return_value=result))
        args, input_text = [kind, "list"], None
        if scenario == "empty":
            getattr(client, listing).return_value = []
        if scenario in {"prompts", "partial_prompt", "prompt_abort"}:
            args = [kind, "add"]
            input_text = "tok\nu\np\n" if kind == "registry" else "tok\nv\n"
            if scenario == "partial_prompt":
                args += ["tok"] + (["--username", "u"] if kind == "registry" else ["--description", "a token"])
                input_text = "p\n" if kind == "registry" else "v\n"
            if scenario == "prompt_abort":
                input_text = ""
        if scenario in {"cancel", "confirm", "first_match", "case_sensitive", "false_delete", "delete_error"}:
            args = [kind, "delete", "TOK" if scenario == "case_sensitive" else "tok"]
            if scenario in {"cancel", "confirm"}:
                input_text = "n\n" if scenario == "cancel" else "y\n"
            else:
                args += ["--yes"]
        if scenario in {"empty_values", "add_error"}:
            args = [kind, "add", ""] + (["--username", "", "--password", ""] if kind == "registry" else ["--value", ""])
        if scenario == "hidden_abort":
            args = [kind, "add", "tok"] + (["--username", "u"] if kind == "registry" else [])
            input_text = ""
        if scenario in {"flaglike_values", "repeated_values"}:
            fields = ["username", "password"] if kind == "registry" else ["value", "description"]
            args = [kind, "add", "tok"]
            for field in fields:
                if scenario == "repeated_values":
                    args += [f"--{field}", "old", f"--{field}", "new"]
                else:
                    args += [f"--{field}", "--synthetic-value"]
        if scenario.endswith("_error"):
            operation = {"add_error": create, "list_error": listing, "delete_error": delete}[scenario]
            getattr(client, operation).side_effect = RuntimeError("service rejected request")
        if scenario == "false_delete":
            getattr(client, delete).return_value = False
        with patch.object(api, "AppsApiClient", return_value=client):
            capture(f"characterization.{kind}_{scenario}", CliRunner().invoke, cli, args, input=input_text)

(ROOT / "tests/fixtures/credential_cli.json").write_text(json.dumps(CASES, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(CASES)} invocations; ten unchanged source tests passed")
