"""Capture pinned Python log filtering, timestamps, summaries, and metric transitions."""
import json
from pathlib import Path
from runpod.apps.logs import tail_summary
from runpod.apps.monitor import _filter_line, _line_before, format_worker_counts, WorkerMonitor

cases=[]
for line in ['hello','', '42','--- Starting Serverless Worker v1 ---','{"result":5}','{broken',' {"message":"kept","level":"INFO"}', '{"message":"hidden","level":"INFO"}', '{"message":"boom","level":"ERROR"}', '{"message":"warning","level":"warn"}', '{"message":null,"level":"WARNING"}', '{"message":false,"level":"ERROR"}']:
    cases.append(dict(operation='filter',input=line,output=_filter_line(line)))
for counts in [{},{'ready':0},{'initializing':1,'ready':2,'running':0},{'unhealthy':-1,'ready':None}]:
    cases.append(dict(operation='format',input=counts,output=format_worker_counts(counts)))
for logs,lines in [({},20),({'system':['s'],'container':['a','b','c']},2),({'container':['a','b']},0),({'container':['a','b','c']},-1),({'container':['a']},-5)]:
    cases.append(dict(operation='tail',input={'logs':logs,'lines':lines},output=tail_summary(logs,lines)))
for line,since in [('no timestamp','2026-01-01T00:00:00Z'),('2026-01-01T00:00:00Z same','2026-01-01T00:00:00Z'),('2026-01-01T00:00:00.123456789Z same micro','2026-01-01T00:00:00.123456Z'),('2026-01-01T01:00:00+01:00 same','2026-01-01T00:00:00Z'),('2026-01-01T00:00:01Z later','2026-01-01T00:00:00Z'),('2026-99-01T00:00:00Z invalid','2026-01-01T00:00:00Z'),('2026-01-01T00:00:00 naive','2026-01-01T00:00:00'),('2026-01-01T00:00:00Z valid','invalid')]:
    cases.append(dict(operation='before',input={'line':line,'since':since},output=_line_before(line,since)))
class Sink:
    def __init__(self): self.events=[]
    def worker_status(self,*args): self.events.append(list(args))
for sequence in [[{'summary':{'idle':2,'running':1}}],[{'summary':{'initializing':1}},{'summary':{'initializing':1}},{'summary':{'idle':1}}],[{}, {'summary':None}], [{'summary':{'throttled':1}},{'summary':{'unhealthy':1}},{'summary':{'idle':None}}]]:
    sink=Sink(); monitor=WorkerMonitor('ep','resource',sink)
    for data in sequence: monitor._report_counts(data)
    cases.append(dict(operation='counts',input=sequence,output=sink.events))
path=Path(__file__).resolve().parents[1]/'tests/fixtures/monitor.json'
path.write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} monitor cases')
