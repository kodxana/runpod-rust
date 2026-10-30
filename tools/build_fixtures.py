"""Capture dependency selection/closure/pruning from the pinned SDK, without pip or network."""
import base64
import json
import tempfile
import sys
from pathlib import Path
from types import SimpleNamespace
# The wheel-tree builders are test helpers from the pinned sibling checkout.
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'runpod-python'))
from runpod.apps import build
from tests.test_apps.test_build import _wheel, TestPruneExcluded

cases=[]
def record(name,op,args,expected,files=None):
    cases.append(dict(name=name,op=op,args=args,expected=expected,files=files))

for spec in ['numpy','numpy==1.26.0','Torch>=2.0','scikit_learn~=1.4','pillow[extras]==10','  numpy >= 1','Foo.Bar==1','models @ https://example.com/a.whl','./package.whl','https://example.com/a.whl','', 'a__b']:
    record(spec,'name',spec,build.requirement_name(spec))
for name,requirements,extra,auto in [
    ('torch_family_auto_excluded',['numpy','torch==2.4','torchvision','requests'],[],True),
    ('user_excludes_added',['numpy','transformers'],['Transformers'],True),
    ('auto_exclude_off_vendors_everything',['numpy','torch==2.4','triton'],[],False),
    ('auto_exclude_off_user_excludes_still_apply',['numpy','torch'],['torch'],False),
    ('deduplicated_sorted_exclusions',['triton','torch==1','torch==2','numpy'],[],True),
]:
    record(name,'split',dict(requirements=requirements,extra_excludes=extra,auto_exclude=auto),build.split_exclusions(requirements,extra,auto_exclude=auto))
for name,text,deps in [('merges_requirements_txt_and_decorator_deps','numpy==1.26\n# comment\n\npandas\n',[['requests','numpy']]),('no_requirements_file',None,[[]]),('first_names_win','-r other.txt\n--index-url https://example.com\nTorch==1\nfoo_bar\n',[['torch==2','foo-bar','Foo.Bar']])]:
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp)
        if text is not None: (root/'requirements.txt').write_text(text)
        app=SimpleNamespace(resources={str(i):SimpleNamespace(spec=SimpleNamespace(dependencies=d)) for i,d in enumerate(deps)})
        record(name,'collect',dict(text=text,dependencies=deps),build.collect_requirements(root,app))

def snapshot(root):
    return {p.relative_to(root).as_posix():base64.b64encode(p.read_bytes()).decode() for p in sorted(root.rglob('*')) if p.is_file()}

def closure(name,roots,wheels):
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp)
        for wheel in wheels: _wheel(root,**wheel)
        record(name,'closure',dict(roots=roots),sorted(build.installed_closure(root,roots)),snapshot(root))

for requested in [False,True]:
    closure('named_url_requirements_preserve_extras_and_markers_'+str(requested),[('models[GPU_Mode]' if requested else 'models')+' @ https://example.com/models.whl'],[dict(name='models',requires=['torch @ https://example.com/torch.whl;download=1 ; extra == "gpu-mode"']),dict(name='torch')])
for root in ['https://example.com/models.whl','./models.whl','models.whl','models.zip','models.tar.gz','models.tgz']:
    closure('unnamed_roots_fail_closed_'+root,[root],[dict(name='models'),dict(name='other')])
for i,marker in enumerate(['(sys_platform == "linux" or sys_platform == "win32") and extra == "torch"','extra == "torch" and (sys_platform == "linux" or sys_platform == "win32")','"torch" == extra','extra == "torch" and python_version < "3.12"','extra == "torch" or python_version >= "3.12"','extra == "a" and extra == "b"','extra == "torch-or-gpu"','extra != "torch"']):
    for extra in ['', '[torch]', '[a,b]', '[torch-or-gpu]']:
        closure(f'marker_{i}_{extra}',['models'+extra],[dict(name='models',requires=['torch; '+marker]),dict(name='torch')])
closure('transitive_extras_activate_grouped_marker',['wrapper'],[dict(name='wrapper',requires=['models[torch]']),dict(name='models',requires=['torch; (sys_platform == "linux" or sys_platform == "win32") and extra == "torch"']),dict(name='torch')])
closure('invalid_marker_fails_closed',['models'],[dict(name='models',requires=['torch; extra ==']),dict(name='torch'),dict(name='other')])
closure('missing_metadata_fails_closed',['models'],[dict(name='models',metadata=False),dict(name='other')])
closure('cycle_with_late_extra',['wrapper','models'],[dict(name='wrapper',requires=['models[torch]']),dict(name='models',requires=['wrapper','torch; extra == "torch"']),dict(name='torch')])

image=sorted(build.SIZE_PROHIBITIVE_PACKAGES)
for name in ['transitive_torch_and_its_tree_removed','root_requirement_is_kept','nothing_excluded_installed_is_noop','missing_record_fails_closed','missing_record_on_excluded_falls_back_to_dir','unnamed_root_fails_closed','unreadable_requires_dist_fails_closed','user_exclude_keeps_its_dependencies','no_image_provided_removes_only_named']:
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp); roots=['peft'];excluded=image;provided=image
        if name=='nothing_excluded_installed_is_noop': _wheel(root,'numpy');roots=['numpy']
        elif name=='missing_record_on_excluded_falls_back_to_dir': _wheel(root,'peft',requires=['torch']);_wheel(root,'torch',record=False)
        else: TestPruneExcluded()._peft_env(root)
        if name=='transitive_torch_and_its_tree_removed': _wheel(root,'faster-whisper',requires=['nvidia-cudnn-cu12']);roots+=['faster-whisper']
        if name=='root_requirement_is_kept': roots+=['sympy==1.14']
        if name=='missing_record_fails_closed': _wheel(root,'oddball',record=False);roots+=['oddball']
        if name=='unnamed_root_fails_closed': roots+=['https://example.com/pkg.tar.gz']
        if name=='unreadable_requires_dist_fails_closed': _wheel(root,'weird',requires=['https://example.com/x.whl']);roots+=['weird']
        if name=='user_exclude_keeps_its_dependencies': _wheel(root,'bigpkg',requires=['sympy']);roots+=['bigpkg'];excluded=image+['bigpkg']
        if name=='no_image_provided_removes_only_named': provided=[];excluded=['torch']
        before=snapshot(root)
        dropped=build.prune_excluded(root,excluded,roots,provided)
        record(name,'prune',dict(excluded=excluded,roots=roots,image_provided=provided),dict(dropped=dropped,files=snapshot(root),dirs=sorted(p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_dir())),before)

path=Path(__file__).resolve().parents[1]/'tests/fixtures/build.json'
path.write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} dependency cases')
