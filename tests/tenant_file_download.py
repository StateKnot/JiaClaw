#!/usr/bin/env python3
"""Current private files through original IO/HTTP owners; no supplier calls."""
import concurrent.futures, hashlib, http.client, json, os, subprocess, sys, tempfile, threading, time, uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import tenant_http_fixture as f
from tenant_http_browser import App

sha=lambda b:hashlib.sha256(b).hexdigest()
app=None; proxy=None; release=None
try:
  with tempfile.TemporaryDirectory(prefix='jiaclaw-file-download-') as tmp:
    root=Path(tmp);grant={'alice':['tenant-documents/report.md','tenant-documents/资料.txt'],'bob':['tenant-documents/report.md']}
    app=App(root,file_write=grant,file_read=grant,file_download=True)
    raw={who:('\ufeff'+who+'-PRIVATE\r\n\t完整🦀  \r\n').encode() for who in grant}
    for who,b in app.backends.items():
        (b['workspace']/'tenant-documents').mkdir(exist_ok=True)
        for path in grant[who]:(b['workspace']/path).write_bytes(raw[who])
    caps={who:f.request(app.gp,'/api/turns/capabilities',app.users[who]['token'])[1] for who in grant}
    assert all(c['file_download_protocol']==1 and c['file_read']['paths']==grant[w] for w,c in caps.items())
    def route(who,index=0,policy=None):return '/api/turns/files/'+str(index)+'/'+(policy or caps[who]['file_read']['policy_sha256'])
    def get(who='alice',path=None,token=None,native=False,extra=None,body=None):
        b=app.backends[who];c=http.client.HTTPConnection('127.0.0.1',b['port'] if native else app.gp,timeout=12)
        headers={'Authorization':'Bearer '+(b['token'] if native else token or app.users[who]['token'])}
        if native:headers['X-JiaClaw-Gateway-Turns']='1'
        headers.update(extra or {});c.request('GET',path or route(who),body=body,headers=headers);r=c.getresponse();data=r.read();h=dict(r.getheaders());code=r.status;c.close();return code,data,h
    def exact(who='alice',token=None,expected=None,native=False):
        code,data,h=get(who,token=token,native=native);expected=raw[who] if expected is None else expected
        assert code==200 and data==expected,(code,data[:150]);lower={k.lower():v for k,v in h.items()}
        assert lower['content-type']=='application/octet-stream' and lower['cache-control']=='no-store' and lower['x-content-type-options']=='nosniff'
        assert lower['x-jiaclaw-file-policy']==caps[who]['file_read']['policy_sha256'] and lower['x-jiaclaw-file-protocol']=='1'
        assert lower['x-jiaclaw-file-index']=='0' and lower['x-jiaclaw-file-bytes']==str(len(data)) and lower['x-jiaclaw-file-sha256']==sha(data)
        return data
    def counts():return {'posts':len(f.posts),'identities':len(f.sql(app.registry,'SELECT * FROM http_turn_requests')),'holds':f.sql(app.registry,'SELECT * FROM write_holds')}
    before=counts();assert get(token='invalid')[0]==401
    for who in grant:exact(who);exact(who,native=True)
    exact(token=app.readonly['token']);assert get('bob',route('alice'))[0]==403
    assert counts()==before
    print('PASS file download 1: paired exact current grants, native/gateway/readonly byte-identical BOM/CRLF and no model/admission/hold changes',flush=True)
    generated=[]
    for who in grant:
        (app.backends[who]['workspace']/grant[who][0]).write_bytes(b'prior file before actual model write')
        prompt='download-generated-'+who;f.tool_plans[(who,prompt)]=[{'id':'write-'+uuid.uuid4().hex,'type':'function','function':{'name':'file_write','arguments':json.dumps({'path':grant[who][0],'content':raw[who].decode()},ensure_ascii=False)}}]
        identity=str(uuid.uuid4());body={'session_id':'http:'+str(uuid.uuid4()),'prompt':prompt,'enabled_tools':['file_write'],'enabled_skills':[],'file_write_policy_sha256':caps[who]['file_write']['policy_sha256']}
        assert f.request(app.gp,'/api/turns/'+identity,app.users[who]['token'],'PUT',body)[0]==202
        f.wait(lambda:not f.sql(app.registry,'SELECT * FROM write_holds WHERE user_id=?',(app.users[who]['user_id'],)),'original write settles before artifact read')
        receipt=f.request(app.gp,'/api/turns/'+identity,app.users[who]['token'])[1]['receipt'];assert receipt['state']=='completed' and receipt['result']['tool_names']==['file_write']
        snapshot=counts();data=exact(who);destination=root/(who+'-actual-download.md');destination.write_bytes(data);assert destination.read_bytes()==(app.backends[who]['workspace']/grant[who][0]).read_bytes()==raw[who]
        assert counts()==snapshot and len([p for p in f.posts if p['prompt']==prompt])==2
        generated.append({'user':who,'id':identity,'source_bytes':len(data),'source_sha256':sha(data),'physical_file_sha256':sha(destination.read_bytes()),'model_posts':2,'download_model_posts':0,'download_identity_rows':0,'hold_preserved':True})
    print('PASS file download 2: two actual original file_write tasks produce physical private files downloaded without model reconstruction or new identities',flush=True)
    target=app.backends['alice']['workspace']/grant['alice'][0]
    for value in [b'',b'x'*262144]:target.write_bytes(value);exact(expected=value)
    for value in [b'x'*262145,b'\xff',b'no\0binary']:
        target.write_bytes(value);assert get()[0]==503 and get(native=True)[0]==400
    target.write_bytes(raw['alice']);snapshot=counts()
    for path in [route('alice',0,'a'*64),route('alice',32),route('alice','00'),route('alice')+'?offset=1',route('alice')+'/',route('alice').replace('/0/','/%30/')]:assert get(path=path)[0]!=200
    assert get(body=b'x')[0]!=200 and get(extra={'Range':'bytes=0-1'})[0]!=200
    assert counts()==snapshot
    print('PASS file download 3: exact zero/262144 bytes, whole oversize/invalid UTF8/NUL refusal, hash/index/query/body/range aliases reject',flush=True)
    protected=app.backends['alice']['workspace']/'protected';protected.write_bytes(b'PRIVATE-PROTECTED')
    for kind in ['link','hardlink','fifo','directory','missing']:
        target.unlink()
        if kind=='link':target.symlink_to(protected)
        elif kind=='hardlink':os.link(protected,target)
        elif kind=='fifo':os.mkfifo(target)
        elif kind=='directory':target.mkdir()
        assert get()[0]==503 and get(native=True)[0]==400,kind
        assert protected.read_bytes()==b'PRIVATE-PROTECTED'
        if kind=='directory':target.rmdir()
        elif kind!='missing':target.unlink()
        target.write_bytes(raw['alice'])
    print('PASS file download 4: original symlink/hardlink/FIFO/directory/missing guards refuse without disclosed bytes or file changes',flush=True)
    fault={'mode':None};ready=threading.Event();unblock=threading.Event()
    class Proxy(BaseHTTPRequestHandler):
        def log_message(self,*a):pass
        def do_GET(self):
            b=app.backends['alice'];c=http.client.HTTPConnection('127.0.0.1',b['port'],timeout=7);c.request('GET',self.path,headers={k:v for k,v in self.headers.items() if k.lower() not in ['host','connection']});r=c.getresponse();data=r.read();headers={k.lower():v for k,v in r.getheaders() if k.lower() not in ['content-length','transfer-encoding','connection']};code=r.status;c.close()
            file='/api/turns/files/' in self.path;mode=fault['mode'] if file else None
            if mode=='held':ready.set();assert unblock.wait(4),'proxy release budget'
            if mode in ['protocol','index','policy','bytes','sha256']:headers['x-jiaclaw-file-'+mode]='invalid'
            if mode=='mime':headers['content-type']='text/html'
            if mode=='short':data=data[:-1]
            if mode=='extra':data+=b'x'
            if mode=='utf8':data=b'\xff';headers['x-jiaclaw-file-bytes']='1';headers['x-jiaclaw-file-sha256']=sha(data)
            self.send_response(code)
            for k,v in headers.items():self.send_header(k,v)
            if mode=='duplicate':self.send_header('x-jiaclaw-file-policy',headers['x-jiaclaw-file-policy'])
            self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);proxy.daemon_threads=True;threading.Thread(target=proxy.serve_forever,daemon=True).start()
    f.stop(app.process);app.gateway['backends'][0]['url']=f'http://127.0.0.1:{proxy.server_port}/';app.config_path.write_text(json.dumps(app.gateway));app.start()
    snapshot=counts()
    for mode in ['protocol','index','policy','bytes','sha256','mime','short','extra','utf8','duplicate']:
        fault['mode']=mode;assert get()[0]==503,mode
    fault['mode']='held'
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        future=pool.submit(get);assert ready.wait(4);app.cli('key-revoke','--key',app.users['alice']['key_id']);unblock.set();assert future.result()[0]==503
    app.users['alice']['token']=app.cli('key-add','--user',app.users['alice']['user_id'])['token'];fault['mode']=None
    exact();assert counts()==snapshot
    f.stop(app.process);app.gateway['backends'][0]['url']=f"http://127.0.0.1:{app.backends['alice']['port']}/";app.config_path.write_text(json.dumps(app.gateway));app.start();proxy.shutdown();proxy.server_close();proxy=None
    print('PASS file download 5: ten actual backend acknowledgement/body corruptions and Key revocation after capture reject; fresh original GET recovers without replay',flush=True)
    b=app.backends['alice'];f.stop(b['process']);lib=root/('download-hold.dylib' if sys.platform=='darwin' else 'download-hold.so')
    command=['cc','-dynamiclib' if sys.platform=='darwin' else '-shared','-std=c11','-O2','-fPIC',str(Path(__file__).with_name('tenant_file_read_hold.c')),'-o',str(lib)]+([] if sys.platform=='darwin' else ['-ldl'])
    subprocess.run(command,check=True,capture_output=True,timeout=30)
    arm=root/'arm';marker=root/'held';release=root/'release';injected=dict(f.env,JIACLAW_TEST_FILE_SOURCE=str(target),JIACLAW_TEST_FILE_ARM=str(arm),JIACLAW_TEST_FILE_MARKER=str(marker),JIACLAW_TEST_FILE_RELEASE=str(release));injected['DYLD_INSERT_LIBRARIES' if sys.platform=='darwin' else 'LD_PRELOAD']=str(lib)
    b['process']=f.launch(['serve','--config',str(b['path'])],b['port'],b['log'],injected);arm.write_text('arm');connections=[]
    try:
        for n in range(1,5):
            c=http.client.HTTPConnection('127.0.0.1',b['port'],timeout=8);c.request('GET',route('alice'),headers={'Authorization':'Bearer '+b['token'],'X-JiaClaw-Gateway-Turns':'1'});connections.append(c);f.wait(lambda:(root/('held-'+str(n))).exists(),'actual current file reader owner',5)
        for c in connections:c.close()
        time.sleep(5.1);assert get(native=True)[0]==409;exact('bob');assert len(list(root.glob('held-*')))==4
        release.write_text('settle');f.wait(lambda:get(native=True)[0]==200,'original native controls released with real reader',5)
    finally:
        release.write_text('settle')
        for c in connections:c.close()
        f.stop(b['process'])
    b['process']=f.launch(['serve','--config',str(b['path'])],b['port'],b['log']);exact()
    print('PASS file download 6: four real held native readers retain original control and I/O ownership after disconnect/5s waiter timeout; fifth refuses until actual release, independent Bob remains usable',flush=True)
    # A new independently isolated fixture proves default-off, without touching current grants.
    off=root/'off';off.mkdir();disabled=App(off,file_read=grant);dc=f.request(disabled.gp,'/api/turns/capabilities',disabled.users['alice']['token'])[1];assert 'file_download_protocol' not in dc
    conn=http.client.HTTPConnection('127.0.0.1',disabled.gp,timeout=8);conn.request('GET','/api/turns/files/0/'+dc['file_read']['policy_sha256'],headers={'Authorization':'Bearer '+disabled.users['alice']['token']});rr=conn.getresponse();assert rr.status==403;rr.read();conn.close()
    assert not f.faults,f.faults
    print('PASS file download 7: omitted switch preserves default-off even with original file_read grant; original tasks and their effects remain separate',flush=True)
    for mode in ['preview']:
        prompt='download-browser-'+mode;content='\ufeffbrowser-'+mode+'\r\n\t完整🦀  \r\n';f.tool_plans[('alice',prompt)]=[{'id':'write-'+uuid.uuid4().hex,'type':'function','function':{'name':'file_write','arguments':json.dumps({'path':grant['alice'][0],'content':content},ensure_ascii=False)}}]
    if '--browser' in sys.argv:
        for mode in ['preview']:
            cfg=root/('browser-'+mode+'.json');cfg.write_text(json.dumps({'base':f'http://127.0.0.1:{app.gp}','mode':mode,'alice':app.users['alice']['token'],'bob':app.users['bob']['token'],'readonly':app.readonly['token'],'expected':('\ufeffbrowser-'+mode+'\r\n\t完整🦀  \r\n')}));r=subprocess.run(['node',str(Path(__file__).with_suffix('.cjs')),str(cfg)],timeout=150);assert r.returncode==0
            assert len([p for p in f.posts if p['prompt']=='download-browser-'+mode])==2
    print('ACTUAL file download receipt '+json.dumps({'users':generated,'held_native_readers':4,'backend_faults':10,'mid_capture_revocations':1,'ordinary_groups':7,'browser_commands':1 if '--browser' in sys.argv else 0,'download_model_posts':0,'automatic_retries':0,'source_bytes_preserved':True}),flush=True)
    print('ALL FILE DOWNLOAD GROUPS PASS',flush=True)
finally:
    if release and release.parent.exists():release.write_text('settle')
    for gate in f.gates.values():gate['release'].set()
    for p in reversed(f.processes):f.stop(p)
    if proxy:proxy.shutdown();proxy.server_close()
    f.model.shutdown();f.model.server_close()
