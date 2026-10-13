#!/usr/bin/env python3
"""Actual tasks and conditional user feedback in the original SQLite; no paid calls."""
import concurrent.futures
import hashlib
import http.client
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import uuid
import sys
import http_turns as h
import tenant_http_browser as tenant
import tenant_http_fixture as f
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def original(receipt):
    return {k:v for k,v in receipt.items() if k not in ('feedback','feedback_revision')}

def proposal(revision=0, decision='usable', note=''):
    return {'expected_revision':revision,'decision':decision,'note':note}

app=None
native=None
proxy=None
class Control(BaseHTTPRequestHandler):
    def log_message(self,*_args): pass
    def do_POST(self):
        value=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if value['action']=='restart':
            if value['mode']=='standalone': native.stop(kill=True);native.start();result={'base':f'http://127.0.0.1:{native.port}'}
            else:
                f.stop(app.process,kill=True)
                for who,b in app.backends.items():
                    f.stop(b['process'],kill=True);b['process']=f.launch(['serve','--config',str(app.root/who/'config.json')],b['port'],app.root/who/'restarted.log')
                app.start();result={'base':f'http://127.0.0.1:{app.gp}'}
        elif value['action']=='status':
            if value['mode']=='standalone': result={'posts':h.count(value['prompt']),'rows':native.sql('SELECT id,feedback_revision,feedback FROM http_turns'),'storage':native.sql('PRAGMA user_version')}
            else: result=app.status('alice',value['prompt'])
        else: raise AssertionError('unknown fixture action')
        raw=json.dumps(result).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)

try:
    with tempfile.TemporaryDirectory(prefix='jiaclaw-task-feedback-') as temporary:
        root=Path(temporary);native=h.App(root,'native');native.start()
        identity,body=native.submit('feedback-native');before=native.terminal(identity);path='/api/turns/'+identity
        note='🦀'*256
        if '--parent-only' in sys.argv:
            c=http.client.HTTPConnection('127.0.0.1',native.port,timeout=10)
            c.request('PUT',path+'/feedback',json.dumps(proposal(note=note)).encode(),{'Authorization':'Bearer '+h.api_token,'Content-Type':'application/json'})
            r=c.getresponse();raw=r.read();status=r.status;c.close()
            print('ACTUAL frozen parent feedback route '+json.dumps({'id':identity,'completed_original':before['state']=='completed','status':status,'body_bytes':len(raw),'body_sha256':hashlib.sha256(raw).hexdigest(),'model_posts':h.count('feedback-native')}),flush=True)
            assert status==200, 'new feedback route expected200; frozen parent actual status '+str(status)
        status,value=native.http(path+'/feedback','PUT',proposal(note=note))
        assert status==200,(status,value)
        if '--parent-only' in sys.argv: raise AssertionError('parent unexpectedly supplied new feedback route')
        saved=value['receipt'];assert saved['feedback_revision']==1 and saved['feedback']['note']==note and original(saved)==before
        assert native.http(path+'/feedback','PUT',proposal())[0]==409
        assert native.http(path+'/feedback','PUT',proposal(1,note=note))[1]['receipt']['feedback_revision']==1
        for bad in [proposal(1,note=note+'x'),proposal(True),proposal(1.0),proposal(1,note='\0'),proposal(1,decision='clear',note='not empty'),dict(proposal(1),extra=1)]:
            assert native.http(path+'/feedback','PUT',bad)[0]==400,bad
        c=http.client.HTTPConnection('127.0.0.1',native.port,timeout=10)
        c.request('PUT',path+'/feedback',b'{"expected_revision":1,"expected_revision":2,"decision":"usable","note":""}',{'Authorization':'Bearer '+h.api_token,'Content-Type':'application/json'})
        r=c.getresponse();assert r.status==400;r.read();c.close()
        assert h.count('feedback-native')==1
        print('PASS task feedback 1: actual original task, full1024UTF8 note, strict typed/duplicate/body refusals, original receipt and one model POST retained',flush=True)

        native.stop(kill=True);native.start();restored=native.http(path)[1]['receipt'];assert restored==saved
        assert native.http(path+'/feedback','PUT',proposal(1,'needs_changes','private correction'))[1]['receipt']['feedback_revision']==2
        assert native.http(path+'/feedback','PUT',proposal(2,'clear'))[1]['receipt']['feedback_revision']==3
        assert native.http(path)[1]['receipt'].get('feedback') is None and native.history(body['session_id'])
        assert h.count('feedback-native')==1
        print('PASS task feedback 2: actual SIGKILL/restart restores exact feedback and original history without replay; correction and clear preserve permanent revision',flush=True)

        race,rb=native.submit('feedback-race');race_original=native.terminal(race)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            codes=list(pool.map(lambda n:native.http('/api/turns/'+race+'/feedback','PUT',proposal(note='candidate'+str(n)))[0],range(2)))
        assert sorted(codes)==[200,409],codes
        race_saved=native.http('/api/turns/'+race)[1]['receipt'];assert race_saved['feedback_revision']==1 and original(race_saved)==race_original
        print('PASS task feedback 3: two actual same-revision writers admit exactly one SQLite update; stale writer rejects with original facts unchanged',flush=True)

        lost,lb=native.submit('feedback-lost');native.terminal(lost);conn=http.client.HTTPConnection('127.0.0.1',native.port,timeout=10)
        conn.request('PUT','/api/turns/'+lost+'/feedback',json.dumps(proposal(note='lost response')).encode(),{'Authorization':'Bearer '+h.api_token,'Content-Type':'application/json'})
        h.eventually(lambda:native.sql('SELECT feedback_revision FROM http_turns WHERE id=?',(lost,))[0]['feedback_revision']==1,'actual feedback commit before dropping unread response')
        conn.close();assert native.http('/api/turns/'+lost)[1]['receipt']['feedback']['note']=='lost response' and h.count('feedback-lost')==1
        print('PASS task feedback 4: real committed write response deliberately unread; original GET recovers revision1 and note with no automatic write/model replay',flush=True)

        assert native.http(path+'/result','DELETE')[0]==200
        assert native.http(path+'/feedback','PUT',proposal(3))[0]==409
        assert native.http('/api/turns/'+str(uuid.uuid4())+'/feedback','PUT',proposal())[0]==404
        gate=h.gate('killed');interrupted,ib=native.submit('killed');assert gate['ready'].wait(10)
        assert native.http('/api/turns/'+interrupted+'/feedback','PUT',proposal())[0]==409
        native.stop(kill=True);gate['release'].set();native.start()
        prior=native.sql('SELECT * FROM http_turns WHERE id=?',(interrupted,));ledger=native.ledger()
        assert native.http('/api/turns/'+interrupted+'/feedback','PUT',proposal())[0]==409
        assert native.sql('SELECT * FROM http_turns WHERE id=?',(interrupted,))==prior and native.ledger()==ledger
        print('PASS task feedback 5: actual running and submitted-model SIGKILL needs_review, purged and missing IDs reject; original unknown rows and model hold untouched',flush=True)

        (root/'tenant').mkdir();app=tenant.App(root/'tenant')
        def call(who,endpoint,method='GET',data=None,token=None):
            return f.request(app.gp,endpoint,token or app.users[who]['token'],method,data)
        tenant_ids={};tenant_originals={}
        for who in ['alice','bob']:
            tid=str(uuid.uuid4());p='/api/turns/'+tid;prompt='feedback-'+who;data={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':['datetime_now']}
            assert call(who,p,'PUT',data)[0]==202
            def done():
                code,v,_=call(who,p);assert code==200
                return v['receipt'] if v['receipt']['state']=='completed' and not v['active'] else None
            tenant_originals[who]=f.wait(done,'actual tenant completed task');tenant_ids[who]=tid
            f.wait(lambda:f.sql(app.registry,'SELECT * FROM write_holds WHERE user_id=?',(app.users[who]['user_id'],))==[], 'original completed gateway hold settlement within original fixture deadline')
            code,v,_=call(who,p+'/feedback','PUT',proposal(note=who+' private'))
            assert code==200 and v['receipt']['feedback_revision']==1 and original(v['receipt'])==tenant_originals[who]
        p='/api/turns/'+tenant_ids['alice']+'/feedback'
        assert call('alice',p,'PUT',proposal(1),app.readonly['token'])[0]==403
        assert call('bob',p,'PUT',proposal(1))[0]==404
        extra=app.cli('key-add','--user',app.users['alice']['user_id']);app.cli('key-revoke','--key',extra['key_id'])
        assert call('alice',p,'PUT',proposal(1),extra['token'])[0]==401
        assert call('alice',p,'PUT',proposal(0))[0]==409
        admissions=f.sql(app.registry,'SELECT * FROM http_turn_requests');holds=f.sql(app.registry,'SELECT * FROM write_holds')
        print('ACTUAL task feedback tenant admission stage '+json.dumps({'admissions':len(admissions),'holds':holds},sort_keys=True),flush=True)
        assert len(admissions)==2 and holds==[]
        print('PASS task feedback 6: real two-user gateway stores own feedback; readonly/revoked/cross-user/stale refuse without new admissions or holds',flush=True)

        held=str(uuid.uuid4());held_prompt='feedback-held';g={'ready':threading.Event(),'release':threading.Event()};f.gates[('alice',held_prompt)]=g
        assert call('alice','/api/turns/'+held,'PUT',{'session_id':'http:'+str(uuid.uuid4()),'prompt':held_prompt,'enabled_tools':['datetime_now']})[0]==202
        assert g['ready'].wait(10)
        before_hold=f.sql(app.registry,'SELECT * FROM write_holds');assert before_hold
        assert call('alice','/api/turns/'+held+'/feedback','PUT',proposal())[0]==409
        assert f.sql(app.registry,'SELECT * FROM write_holds')==before_hold
        g['release'].set()
        f.wait(lambda:f.sql(app.registry,'SELECT * FROM write_holds')==[], 'original held task settles after its model finishes')
        f.stop(app.process,kill=True)
        for who,b in app.backends.items():
            f.stop(b['process'],kill=True);b['process']=f.launch(['serve','--config',str(app.root/who/'config.json')],b['port'],app.root/who/'feedback-restart.log')
        app.start()
        for who,tid in tenant_ids.items():
            code,v,_=call(who,'/api/turns/'+tid);assert code==200 and v['receipt']['feedback']['note']==who+' private' and original(v['receipt'])==tenant_originals[who]
            assert app.status(who,'feedback-'+who)['count']==1
        print('PASS task feedback 7: actual gateway and both backend SIGKILL/restarts retain user feedback, original hashes/history/receipts and one model POST each',flush=True)
        # Forward the real backend write, corrupt only its acknowledgement, and
        # reconcile each actual committed note with the original authenticated GET.
        class Proxy(BaseHTTPRequestHandler):
            mutation=None
            def log_message(self,*_args):pass
            def do_GET(self):self.forward()
            def do_PUT(self):self.forward()
            def forward(self):
                conn=http.client.HTTPConnection('127.0.0.1',app.backends['alice']['port'],timeout=10)
                data=self.rfile.read(int(self.headers.get('Content-Length','0')))
                headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection')}
                conn.request(self.command,self.path,data or None,headers);r=conn.getresponse();raw=r.read();code=r.status
                if self.command=='PUT' and self.path.endswith('/feedback') and code==200:
                    value=json.loads(raw);result=value['receipt']['result'];mode=type(self).mutation
                    if mode=='wrong_result_status':result['status']='needs_review'
                    elif mode=='missing_reply':del result['reply']
                    elif mode=='missing_tools':del result['tool_names']
                    elif mode=='nonarray_tools':result['tool_names']='datetime_now'
                    elif mode=='invalid_tool':result['tool_names']=['exec']
                    elif mode=='nonstr_reply':result['reply']=True
                    elif mode=='oversized_reply':result['reply']='x'*(2*1024*1024+1)
                    elif mode=='oversized_tools':result['tool_names']=['datetime_now']*2049
                    raw=json.dumps(value).encode()
                self.send_response(code);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw);conn.close()
        proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);proxy.daemon_threads=True;threading.Thread(target=proxy.serve_forever,daemon=True).start()
        f.stop(app.process);original_url=app.gateway['backends'][0]['url'];app.gateway['backends'][0]['url']=f'http://127.0.0.1:{proxy.server_port}/';app.config_path.write_text(json.dumps(app.gateway));app.start()
        acknowledgements={};p='/api/turns/'+tenant_ids['alice'];admissions=f.sql(app.registry,'SELECT * FROM http_turn_requests');holds=f.sql(app.registry,'SELECT * FROM write_holds')
        for mode in ['wrong_result_status','missing_reply','missing_tools','nonarray_tools','invalid_tool','nonstr_reply','oversized_reply','oversized_tools']:
            Proxy.mutation=mode;current=call('alice',p)[1]['receipt']
            code,v,_=call('alice',p+'/feedback','PUT',proposal(current['feedback_revision'],note=mode));acknowledgements[mode]=code
            print('ACTUAL task feedback acknowledgement case '+json.dumps({'case':mode,'status':code}),flush=True)
            assert code==503,(mode,code,v)
            reconciled=call('alice',p)[1]['receipt'];assert reconciled['feedback']['note']==mode and original(reconciled)==tenant_originals['alice']
            assert f.sql(app.registry,'SELECT * FROM http_turn_requests')==admissions and f.sql(app.registry,'SELECT * FROM write_holds')==holds
        Proxy.mutation=None;revision=call('alice',p)[1]['receipt']['feedback_revision'];assert call('alice',p+'/feedback','PUT',proposal(revision,note='alice private'))[0]==200
        f.stop(app.process);app.gateway['backends'][0]['url']=original_url;app.config_path.write_text(json.dumps(app.gateway));app.start();proxy.shutdown();proxy=None
        assert app.status('alice','feedback-alice')['count']==1
        print('ACTUAL task feedback acknowledgement refusals '+json.dumps(acknowledgements,sort_keys=True),flush=True)
        print('PASS task feedback 8: eight malformed real backend acknowledgements reject503; original GET recovers committed intent without repeated write/model/admission or hold settlement',flush=True)
        receipt={'protocol':1,'native_id':identity,'race_id':race,'lost_id':lost,'unknown_id':interrupted,'alice_id':tenant_ids['alice'],'bob_id':tenant_ids['bob'],'held_id':held,'held_original_posts':1,'native_note_bytes':1024,'native_original_posts':1,'tenant_original_posts':[1,1],'no_extra_admissions':True,'no_hold_clear':True,'restart_exact':True}
        print('ACTUAL task feedback receipts '+json.dumps(receipt,sort_keys=True),flush=True)
        if '--browser' in sys.argv:
            control=ThreadingHTTPServer(('127.0.0.1',0),Control);control.daemon_threads=True;threading.Thread(target=control.serve_forever,daemon=True).start()
            # Separate new native instance has no original unknown model hold.
            native.stop();native=h.App(root,'browser-native',timeout=15);native.start()
            for mode in ['standalone','tenant_json','tenant_preview']:
                cfg={'mode':mode,'base':f'http://127.0.0.1:{native.port if mode=="standalone" else app.gp}','token':h.api_token if mode=='standalone' else app.users['alice']['token'],'readonly':app.readonly['token'],'control':f'http://127.0.0.1:{control.server_port}'}
                config=root/('browser-'+mode+'.json');config.write_text(json.dumps(cfg))
                r=subprocess.run(['node','tests/task_feedback_browser.cjs',str(config)],timeout=150);assert r.returncode==0,(mode,r.returncode)
            control.shutdown()
        assert not h.faults,h.faults
        assert not f.faults,f.faults
        print('ALL TASK FEEDBACK HTTP GROUPS PASS',flush=True)
finally:
    for gate in h.gates.values():gate['release'].set()
    for gate in f.gates.values():gate['release'].set()
    for process in reversed(h.processes+f.processes):
        if process.poll() is None:
            process.terminate();process.wait(timeout=8)
    if proxy:proxy.shutdown()
    h.server.shutdown();f.model.shutdown()

