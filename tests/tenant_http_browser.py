#!/usr/bin/env python3
"""Real Chromium + two private backends/gateway/registry/model; no vendor calls."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import tempfile
import threading
import uuid
import sys
import tenant_http_fixture as f

class App:
    def __init__(self, root):
        self.root=root; self.registry=root/'registry/users.sqlite3'; self.backends={}
        self.gp=f.port(); self.config_path=root/'gateway.json'
        self.gateway={'bind':'127.0.0.1:'+str(self.gp),'registry_path':str(self.registry),
                      'request_timeout_seconds':15,'max_in_flight':4,'tracked_turns':True,'backends':[]}
        for who in ('alice','bob'):
            directory=root/who; directory.mkdir(mode=0o700);workspace=directory/'workspace';workspace.mkdir(mode=0o700)
            token='fixture-backend-'+uuid.uuid4().hex;token_file=directory/'token';token_file.write_text(token);token_file.chmod(0o600);number=f.port()
            config={'agent':{'name':who,'description':'Isolated tenant browser acceptance','system_instructions':'Only explicit tools.','max_turns':10,'workspace_path':str(workspace),'tool_timeout_secs':2},
                'provider':{'provider_type':'brokerrouter','base_url':f'http://127.0.0.1:{f.model.server_port}','api_key':f.keys[who],'model':'fixture-tenant-http'},
                'model_calls':{'enabled':True,'store_path':'../state/model-calls/index.sqlite3'},
                'http':{'bind':f'127.0.0.1:{number}','persist':True,'persist_path':'../state/sessions.sqlite3','api_token':token,'tracked_turns':True,'tracked_turn_timeout_secs':10,'shutdown_timeout_secs':2,'gateway_channel_chat':True}}
            path=directory/'config.json';path.write_text(json.dumps(config));self.backends[who]={'port':number,'token':token,'db':directory/'state/sessions.sqlite3','ledger':directory/'state/model-calls/index.sqlite3'}
            self.backends[who]['process']=f.launch(['serve','--config',str(path)],number,directory/'serve.log')
            self.gateway['backends'].append({'id':who,'url':f'http://127.0.0.1:{number}/','token_file':str(token_file)})
        self.config_path.write_text(json.dumps(self.gateway))
        self.users={who:self.cli('user-add','--backend',who) for who in ('alice','bob')}
        self.readonly=self.cli('key-add','--user',self.users['alice']['user_id'],'--read-only')
        self.start()
    def cli(self,*args):
        r=subprocess.run([str(f.binary),'gateway',*args,'--config',str(self.config_path)],env=f.env,capture_output=True,text=True,timeout=15)
        assert r.returncode==0,(args,r.stdout,r.stderr);return json.loads(r.stdout)
    def start(self):self.process=f.launch(['gateway','serve','--config',str(self.config_path)],self.gp,self.root/'gateway.log')
    def status(self,who,prompt):
        with f.lock: records=[p for p in f.posts if p['who']==who and p['prompt']==prompt];gate=f.gates.get((who,prompt))
        b=self.backends[who]
        return {'count':len(records),'ready':bool(gate and gate['ready'].is_set()),'rows':f.sql(b['db'],'SELECT id,session_id,state,session_committed,cancel_requested,reviewed_ms FROM http_turns'),
                'holds':f.sql(self.registry,'SELECT request_id,state FROM write_holds WHERE user_id=?',(self.users[who]['user_id'],)), 'faults':f.faults}
    def review(self,who,identity):
        b=self.backends[who];code,value,_=f.request(b['port'],'/api/turns/'+identity,b['token'],marker=True)
        assert code==200 and not value['active'] and value['receipt']['state']=='needs_review',value
        code,value,_=f.request(b['port'],'/api/turns/'+identity+'/review',b['token'],'POST',{'decision':'abandon','note':'Actual fixture backend and original model receipt checked.'},marker=True)
        assert code==200 and value['receipt']['reviewed_ms'] is not None,value
        self.cli('review-clear','--user',self.users[who]['user_id'],'--confirm-backend-idle','--note','Actual fixture model and backend idle; no tool effects.')

app=None
class Control(BaseHTTPRequestHandler):
    def log_message(self,*_args):pass
    def do_POST(self):
        try:
            value=json.loads(self.rfile.read(int(self.headers['Content-Length'])));who=value.get('who','alice');prompt=value.get('prompt','none');action=value['action'];assert who in ('alice','bob')
            if action=='gate':f.gates[(who,prompt)]={'ready':threading.Event(),'release':threading.Event()};result=True
            elif action=='release':f.gates[(who,prompt)]['release'].set();result=True
            elif action=='status':result=app.status(who,prompt)
            elif action=='review':app.review(who,value['id']);result=True
            elif action=='restart':f.stop(app.process,kill=True);app.start();result=True
            elif action=='stop-backend':f.stop(app.backends[who]['process']);result=True
            else:raise AssertionError('unknown fixture control')
            raw=json.dumps({'value':result}).encode();self.send_response(200)
        except Exception as e:raw=json.dumps({'error':repr(e)}).encode();self.send_response(500)
        self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)

try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-tenant-browser-') as temporary:
        root=Path(temporary);app=App(root);control=ThreadingHTTPServer(('127.0.0.1',0),Control);control.daemon_threads=True
        threading.Thread(target=control.serve_forever,daemon=True).start()
        config=root/'browser.json';config.write_text(json.dumps({'base':f'http://127.0.0.1:{app.gp}','alice':app.users['alice']['token'],'bob':app.users['bob']['token'],'readonly':app.readonly['token'],'control':f'http://127.0.0.1:{control.server_port}'}))
        preview = sys.argv[2:] == ['--preview']
        assert not sys.argv[2:] or preview, 'only --preview is supported'
        script = 'tests/tenant_http_preview_browser.cjs' if preview else 'tests/tenant_http_browser.cjs'
        r=subprocess.run(['node',script,str(config)],timeout=240);assert r.returncode==0,r.returncode;assert not f.faults,f.faults
        print('ALL TENANT PREVIEW BROWSER GROUPS PASS' if preview else 'ALL TENANT HTTP BROWSER GROUPS PASS',flush=True)
finally:
    for gate in f.gates.values():gate['release'].set()
    for p in reversed(f.processes):f.stop(p)
    f.model.shutdown()
