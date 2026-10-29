"""Record the pinned Python source surface without importing or executing it."""

import ast
import json
import pathlib
import subprocess
import sys

source = pathlib.Path(sys.argv[1]).resolve()
destination = pathlib.Path(__file__).resolve().parents[1] / "parity"
destination.mkdir(exist_ok=True)
symbols = []
tests = []
lines = 0
for package, output in [("runpod", symbols), ("tests", tests)]:
    for path in sorted((source / package).rglob("*.py")):
        content = path.read_text(encoding="utf-8")
        tree = ast.parse(content)
        if package == "runpod":
            lines += len(content.splitlines())

        def visit(nodes, parents=()):
            for node in nodes:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    name = ".".join((*parents, node.name))
                    is_class = isinstance(node, ast.ClassDef)
                    if package == "runpod" or node.name.startswith("test_"):
                        output.append({
                            "file": path.relative_to(source).as_posix(),
                            "name": name,
                            "line": node.lineno,
                            "kind": "class" if is_class else "function",
                            "public": all(not p.startswith("_") for p in (*parents, node.name)),
                            "signature": "" if is_class else ast.unparse(node.args),
                            "rust": None,
                            "status": "pending",
                        })
                    if is_class:
                        visit(node.body, (*parents, node.name))

        visit(tree.body)

commit = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
for name, entries in [("api", symbols), ("tests", tests)]:
    path = destination / f"{name}.json"
    if path.exists():
        old = {(x["file"], x["name"]): x for x in json.loads(path.read_text())["entries"]}
        for entry in entries:
            previous = old.get((entry["file"], entry["name"]), {})
            for key in ("rust", "status", "notes"):
                if key in previous:
                    entry[key] = previous[key]
    path.write_text(json.dumps({"upstream": "https://github.com/runpod/runpod-python", "commit": commit, "entries": entries}, indent=2) + "\n", encoding="utf-8")
print(f"Pinned {commit}: {lines} source lines, {len(symbols)} symbols ({sum(x['public'] for x in symbols)} public), {len(tests)} tests")
