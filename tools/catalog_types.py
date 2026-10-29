"""Generate static Rust catalog enums from the pinned Python source AST."""
import ast
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
root = pathlib.Path(__file__).resolve().parents[1]
output = ["//! Pinned Python catalog values; regenerate with tools/catalog_types.py.\n", "use std::{fmt, str::FromStr};\nuse serde::{Serialize, Deserialize};\nuse crate::{Error, Result};\n"]
classes = {}
trees = {}
variant = lambda name: "".join(part.capitalize() for part in name.split("_"))
for file in ["gpu", "datacenter"]:
    tree = ast.parse((source / f"runpod/apps/{file}.py").read_text())
    trees[file] = tree
    for node in tree.body:
        if not isinstance(node, ast.ClassDef) or node.name.startswith("_"):
            continue
        pairs = [(n.targets[0].id, ast.literal_eval(n.value)) for n in node.body if isinstance(n, ast.Assign) and isinstance(n.value, ast.Constant)]
        if not pairs:
            continue
        classes[node.name] = pairs
        output.append(f"/// Upstream {node.name} catalog values.\n#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]\n#[non_exhaustive]\npub enum {node.name} {{")
        for name, value in pairs:
            output.append(f"    /// {value}.\n    #[serde(rename = {json.dumps(value)})]\n    {variant(name)},")
        output.append(f"}}\nimpl {node.name} {{\n    /// Returns the platform's exact identifier.\n    pub fn as_str(self) -> &'static str {{ match self {{")
        for name, value in pairs:
            output.append(f"        Self::{variant(name)} => {json.dumps(value)},")
        output.append("    }}\n    /// Returns concrete catalog entries in upstream declaration order.\n    pub fn all() -> &'static [Self] { &[")
        output.extend(f"        Self::{variant(name)}," for name, _ in pairs if name != "ANY")
        output.append("    ] }\n}")
        output.append(f"impl fmt::Display for {node.name} {{ fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{ f.write_str(self.as_str()) }} }}")
        output.append(f"impl FromStr for {node.name} {{ type Err = Error;\n    fn from_str(value: &str) -> Result<Self> {{")
        if node.name == "DataCenter":
            output.append('        let normalized = value.trim().to_uppercase().replace(\'_\', "-");\n        match normalized.as_str() {')
        else:
            output.append("        match value {")
        output.extend(f"            {json.dumps(value)} => Ok(Self::{variant(name)})," for name, value in pairs)
        output.append(f'            _ => Err(Error::InvalidInput(format!("unknown {node.name} {{value}}"))),\n        }}\n    }}\n}}')

pool_node = next(n for n in trees["gpu"].body if isinstance(n, ast.Assign) and n.targets[0].id == "POOLS_TO_TYPES")
output.append("impl GpuGroup {\n    /// Returns the statically pinned members of this GPU pool.\n    pub fn device_names(self) -> &'static [GpuType] { match self {")
for key, values in zip(pool_node.value.keys, pool_node.value.values):
    types = ", ".join("GpuType::" + variant(v.attr) for v in values.elts)
    output.append(f"        Self::{variant(key.attr)} => &[{types}],")
output.append("        Self::Any => &[],\n    }}\n}")
output.append("/// Returns the static pool containing the exact GPU device.\npub fn pool_for_gpu_type(device: GpuType) -> Option<GpuGroup> { GpuGroup::all().iter().copied().find(|pool|pool.device_names().contains(&device)) }")
(root / "src/apps/catalog.rs").write_text("\n".join(output) + "\n", encoding="utf-8")
print(f"Generated {len(classes)} catalog enums from pinned upstream source")
