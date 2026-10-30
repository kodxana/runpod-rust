"""Capture archive member selection and file contents from the pinned Python packager."""
import base64
import json
import tarfile
import tempfile
from pathlib import Path
from runpod.apps.deploy import package_project

cases=[]
def capture(name,files,env_files=None,env_path='vendor',output='artifact.bin',manifest=None):
    manifest={} if manifest is None else manifest
    case=dict(name=name,files=files,env_files=env_files,env_path=env_path,output=output,manifest=manifest)
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp)/'project';root.mkdir()
        for name,content in files.items():
            file=root/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(content,encoding='utf-8',newline='')
        env=None
        if env_files is not None:
            env=root/env_path;env.mkdir(parents=True,exist_ok=True)
            for name,content in env_files.items():
                file=env/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(content,encoding='utf-8',newline='')
        archive=package_project(root,manifest,output=root/output,env_dir=env)
        with tarfile.open(archive) as tar:
            case['names']=tar.getnames()
            case['contents']={member.name:base64.b64encode(tar.extractfile(member).read()).decode() for member in tar if member.name!='runpod_manifest.json'}
            case['archived_manifest']=json.load(tar.extractfile('runpod_manifest.json'))
    cases.append(case)

capture('source_and_manifest',{'main.py':'print(42)\n','runpod_manifest.json':'forged','nested/runpod_manifest.json':'nested is source'},manifest={'app':'demo','unicode':'crab 🦀','resources':[]})
capture('protected_cannot_be_unignored',{'main.py':'source','.env':'private','x.env.production':'private','private.pem':'private','private.key':'private','.aws/config':'private','nested/.ssh/id_ed25519':'private','.docker/config.json':'private','nested/.docker/config.json':'private','service-account-demo.json':'private','service_account_demo.json':'private','.runpodignore':'!*.env*\n!*.pem\n!*.key\n!.aws/\n!.ssh/\n!.docker/\n!service*.json\n!**/.docker/config.json\n'})
capture('credential_like_source_names',{'credentials':'source','secrets':'source','src/credentials.py':'source','src/secrets.py':'source','src/nested/secrets/config.py':'source','src/nested/credentials/config.py':'source'})
capture('nested_precedence',{'root.log':'root','nested/keep.log':'keep','nested/drop.log':'drop','other/keep.log':'other','.gitignore':'*.log\n','nested/.gitignore':'!*.log\n','.runpodignore':'!root.log\nnested/drop.log\n'})
capture('defaults_and_unignore',{'test_app.py':'test','module_test.py':'test','tests/unit.py':'test','nested/tests/unit.py':'test','nested/env/a.py':'env','env/a.py':'root env','node_modules/a.js':'node','.venv/a.py':'venv','__pycache__/a.pyc':'cache','pkg/a.pyc':'cache','.DS_Store':'os','.runpodignore':'!test_app.py\n!tests/\n!nested/tests/\n!nested/env/\n!env/\n!node_modules/\n!*.pyc\n'})
capture('directory_pruning',{'blocked/keep.py':'blocked','blocked/.gitignore':'!keep.py\n','open/deep/file.py':'keep','.gitignore':'blocked/\nopen/*\n!open/deep/\n','.runpodignore':''})
capture('root_anchoring',{'root-only.txt':'root','nested/root-only.txt':'nested','pkg/a.txt':'a','other/pkg/a.txt':'b','.gitignore':'/root-only.txt\n/pkg/\n'})
capture('nested_root_anchoring',{'a/root.txt':'a root','a/deep/root.txt':'a nested','b/root.txt':'b root','a/.gitignore':'/root.txt\n'})
capture('glob_patterns',{'a.tmp':'tmp','a1.txt':'a1','a2.txt':'a2','ab.txt':'ab','foo/bar/a.log':'log','foo/baz/a.txt':'text','docs/a.md':'doc','docs/keep.md':'keep','.gitignore':'a[12].txt\n**/*.log\n*.tmp\ndocs/*\n!docs/keep.md\n'})
capture('escaped_literals',{'#literal':'hash','!literal':'bang','space name.txt':'space','.gitignore':'\\#literal\n\\!literal\nspace\\ name.txt\n'})
capture('runpodignore_root_scope',{'a/.runpodignore':'file.py\n','a/file.py':'included','file.py':'root'})
capture('env_is_separate',{'main.py':'source','.runpodignore':'*.pem\n*.txt\n','user.pem':'private'},env_files={'numpy/__init__.py':'','numpy/ca.pem':'dependency CA','.env':'dependency data','tests/fixture.txt':'dependency fixture'})
capture('env_outside_project',{'main.py':'source'},env_files={'pkg/data.bin':'data'},env_path='../built-env')
capture('empty_project',{},manifest={'version':1})
capture('unicode_and_order',{'z.txt':'z','a.txt':'a','目录/脚本.py':'print("值")','a/second.txt':'second','a/first.txt':'first','b/a.txt':'b'},manifest={'name':'文字'})
capture('output_excluded_by_identity',{'main.py':'source','archive.bin':'old archive','.runpodignore':'!*.bin\n'},output='archive.bin')

path=Path(__file__).resolve().parents[1]/'tests/fixtures/package.json'
path.write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} archive cases')
