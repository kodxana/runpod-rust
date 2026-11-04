"""Capture the pinned heartbeat's HTTP requests and urllib3 retry decisions locally."""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import urllib3
from runpod.serverless.modules import rp_ping
from runpod.serverless.modules.worker_state import PingJobMirror

cases = []
for name, responses in [
    ('ok', [(200, None)]),
    ('ordinary_client_error', [(400, None)]),
    ('rate_limit', [(429, None), (200, None)]),
    ('exhausted', [(503, None)] * 4),
    ('server_recovery', [(500, None), (502, None), (504, None), (200, None)]),
    ('retry_after', [(503, '1'), (200, None)]),
    ('payload_retry_after_zero', [(413, '0'), (200, None)]),
    ('payload_without_header', [(413, None)]),
    ('unlisted_with_header', [(418, '1')]),
]:
    requests, delays = [], []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            request = urlsplit(self.path)
            requests.append(dict(path=request.path, query=parse_qs(request.query),
                                 authorization=self.headers['Authorization']))
            status, retry_after = responses[min(len(requests) - 1, len(responses) - 1)]
            self.send_response(status)
            if retry_after is not None:
                self.send_header('Retry-After', retry_after)
            self.send_header('Content-Length', '0')
            self.end_headers()

        def log_message(self, *_):
            pass

    with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = dict(RUNPOD_POD_ID='worker', RUNPOD_AI_API_KEY='test-key',
                   RUNPOD_WEBHOOK_PING=f'http://127.0.0.1:{server.server_port}/ping/$RUNPOD_POD_ID',
                   RUNPOD_PING_INTERVAL='5999')
        with patch.dict(os.environ, env, clear=True), patch.object(rp_ping, 'WORKER_ID', 'worker'), \
                patch.object(rp_ping, 'runpod_version', 'VERSION'), \
                patch('urllib3.util.retry.time.sleep', side_effect=delays.append), \
                patch.object(rp_ping, 'log') as log:
            heartbeat = rp_ping.Heartbeat()
            mirror = PingJobMirror()
            mirror.set('job 1,job/2')
            heartbeat._mirror = mirror
            heartbeat._send_ping()
            cases.append(dict(name=name, responses=responses, requests=requests,
                              delays=delays, failed=log.error.called,
                              interval=heartbeat.PING_INTERVAL))
            heartbeat._session.close()
        server.shutdown()
        thread.join()

path = Path(__file__).resolve().parents[1] / 'tests/fixtures/heartbeat.json'
path.write_text(json.dumps(dict(urllib3=urllib3.__version__, cases=cases), indent=2) + '\n', encoding='utf-8')
print(f'Captured {len(cases)} heartbeat wire/retry cases')
