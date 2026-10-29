"""Characterize the pinned placement solver without requesting cloud capacity."""
import json
from pathlib import Path
from runpod.apps.placement import StockMap, candidates, solve_placement
from runpod.apps.spec import ResourceSpec, ResourceKind
from runpod.apps.datacenter import DataCenter

CASES=[]

def case(name, specs, stocks, **options):
    CASES.append(dict(name=name,specs=specs,stocks=stocks,options=options))

def spec(name="r", **kwargs):
    return dict(kind="task",name=name,**kwargs)

for gpu in ["4090","ADA_24","NVIDIA GeForce RTX 4090"]:
    case("gpu_"+gpu,[spec(gpu=gpu)],[["gpu-pod",gpu if gpu.startswith("NVIDIA") else "NVIDIA GeForce RTX 4090",1,"EU-RO-1",3]])
for pin in [" eu_ro_1 ",["eu-ro-1"],["US-KS-2"]]:
    case("pin_"+str(pin),[spec(cpu="cpu5c-2-4",datacenter=pin)],[["cpu-pod","cpu5c-2-4",0,"EU-RO-1",2]])
case("any_gpu",[spec()],[])
case("intersection",[spec("train",gpu="H200"),spec("infer",gpu="4090")],[["gpu-pod","NVIDIA H200",1,"EU-RO-1",2],["gpu-pod","NVIDIA GeForce RTX 4090",1,"EU-RO-1",3],["gpu-pod","NVIDIA GeForce RTX 4090",1,"US-KS-2",3]])
case("disjoint",[spec("train",gpu="H200"),spec("eval",gpu="B200")],[["gpu-pod","NVIDIA H200",1,"EU-RO-1",3],["gpu-pod","NVIDIA B200",1,"US-KS-2",3]])
for dc in ["EU-RO-1","US-KS-2"]:
    case("existing_"+dc,[spec("train",gpu="H200")],[["gpu-pod","NVIDIA H200",1,"EU-RO-1",3]],existing_dc=dc,volume_datacenters=[])
case("maximin",[spec("a",gpu="H200"),spec("b",gpu="B200")],[["gpu-pod","NVIDIA H200",1,"EU-RO-1",3],["gpu-pod","NVIDIA H200",1,"US-KS-2",2],["gpu-pod","NVIDIA B200",1,"EU-RO-1",1],["gpu-pod","NVIDIA B200",1,"US-KS-2",2]])
case("tie_lexical",[spec()],[],volume_datacenters=["US-KS-2","EU-RO-1"])
case("mixed",[spec("gpu",gpu="4090"),spec("cpu",cpu="cpu5c-2-4")],[["gpu-pod","NVIDIA GeForce RTX 4090",1,"EU-RO-1",3],["gpu-pod","NVIDIA GeForce RTX 4090",1,"US-KS-2",3],["cpu-pod","cpu5c-2-4",0,"EU-RO-1",2]])
case("storage_excludes_best",[spec(cpu="cpu5c-2-4")],[["cpu-pod","cpu5c-2-4",0,"EU-RO-1",1],["cpu-pod","cpu5c-2-4",0,"US-KS-2",3]],volume_datacenters=["EU-RO-1"])
case("pinned_disjoint",[spec(gpu="4090")],[["gpu-pod","NVIDIA GeForce RTX 4090",1,"EU-RO-1",3]],volume_dc="US-KS-2")
for kind in ["queue","task"]:
    stock_kind = "gpu" if kind=="queue" else "gpu-pod"
    case("gpu_count_"+kind,[dict(kind=kind,name="single",gpu="4090"),dict(kind=kind,name="pair",gpu="4090",gpu_count=2)],[[stock_kind,"NVIDIA GeForce RTX 4090",1,"EU-RO-1",3],[stock_kind,"NVIDIA GeForce RTX 4090",1,"US-KS-2",2],[stock_kind,"NVIDIA GeForce RTX 4090",2,"US-KS-2",2]])
case("product_stock",[dict(kind="queue",name="serve",cpu="cpu5c-2-4"),spec("task",cpu="cpu5c-2-4")],[["cpu","cpu5c-2-4",0,"EU-RO-1",3],["cpu","cpu5c-2-4",0,"US-KS-2",2],["cpu-pod","cpu5c-2-4",0,"US-KS-2",2]])

fixtures=[]
for item in CASES:
    specs=[ResourceSpec(**{**s,"kind":ResourceKind(s["kind"])}) for s in item["specs"]]
    stock=StockMap(api=object())
    for kind,hw,count,dc,score in item["stocks"]:
        if kind.startswith("gpu"):
            target=stock._gpu_pod if kind=="gpu-pod" else stock._gpu
            target[(hw,count,dc)]=score
        else:
            stock._cpu[(hw,dc,kind=="cpu-pod")]=score
    options={"volume_name":"models","volume_datacenters":[dc.value for dc in DataCenter.all()],**item["options"]}
    item["options"]=options
    item["candidates"]=[sorted(candidates(s,stock)) for s in specs]
    try:
        item["output"]=solve_placement(specs,stock,**{**options,"volume_datacenters":set(options["volume_datacenters"])})
    except Exception as error:
        item["error"]=str(error)
    fixtures.append(item)
(Path(__file__).resolve().parents[1]/"tests/fixtures/placement.json").write_text(json.dumps(fixtures,indent=2)+"\n",encoding="utf-8")
print(f"Captured {len(fixtures)} placement cases")
