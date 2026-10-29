"""Extract static upstream GraphQL documents without executing Python SDK code."""
import ast
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
destination = pathlib.Path(__file__).resolve().parents[1] / "src/apps/graphql"
destination.mkdir(parents=True, exist_ok=True)
declarations = ["//! Pinned upstream GraphQL documents (MIT; see LICENSE-UPSTREAM).\n"]
for relative in ["runpod/api/queries/apps.py", "runpod/api/mutations/apps.py"]:
    tree = ast.parse((source / relative).read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            name = node.targets[0].id
            filename = name.lower() + ".graphql"
            (destination / filename).write_text(node.value.value, encoding="utf-8")
            declarations.append(f'/// Upstream {name.lower().replace("_", " ")} operation.\npub const {name}: &str = include_str!("graphql/{filename}");\n')
(destination.parent / "graphql.rs").write_text("\n".join(declarations), encoding="utf-8")
print(f"Extracted {len(declarations) - 1} GraphQL documents")
