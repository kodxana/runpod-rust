"""Capture native JSON resource manifests from the pinned Python definitions."""
import json
import pathlib
from runpod.apps.spec import ResourceSpec

cases = []
for kind in ["queue", "api", "task"]:
    cases.append({"kind":kind,"name":"resource"})
for change in [
    {"gpu":"h100"},{"gpu":["4090","ampere_80"]},{"cpu":"cpu3c-4-8"},
    {"workers":5},{"workers":[2,8]},{"workers":0},{"workers":[5,2]},
    {"gpu":"4090","cpu":"cpu3c-4-8"},{"scaler_type":" request_count ","scaler_value":7},
    {"min_cuda_version":" 12.8 "},{"cpu":"cpu3c-4-8","min_cuda_version":"12.8"},
    {"datacenter":["us_ca_2","EU-RO-1"]},{"model":"owner/model:revision"},
    {"env":{"X":12,"FLAG":False,"NULL":None}},
    {"dependencies":["numpy"],"system_dependencies":["git"],"image":"custom:tag","registry_auth":"reg", "schedule":"0 * * * *","max_concurrency":4,"execution_timeout_ms":1000,"flashboot":False,"accelerate_downloads":False,"container_disk_gb":40},
    {"gpu_count":0},{"max_concurrency":0},{"execution_timeout_ms":-1},{"scaler_value":0},{"container_disk_gb":0},
]:
    cases.append({"kind":"queue","name":"resource",**change})
cases.append({"kind":"task","name":"resource","model":"owner/model"})
fixtures=[]
for kwargs in cases:
    fixture={"kwargs":kwargs}
    try:
        fixture["manifest"]=ResourceSpec(**kwargs).to_manifest()
    except Exception as error:
        fixture["error"]=str(error)
    fixtures.append(fixture)
path=pathlib.Path(__file__).resolve().parents[1]/"tests/fixtures/spec.json"
path.write_text(json.dumps(fixtures,indent=2)+"\n")
print(f"Captured {len(fixtures)} Python resource specifications")
