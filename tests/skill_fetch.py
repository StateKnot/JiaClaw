#!/usr/bin/env python3
"""Real local TLS capture/recovery; no public supplier or release certification."""
import fcntl
import hashlib
import http.server
import json
import os
import pathlib
import signal
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from stopped_child import wait_stopped

binary = pathlib.Path(sys.argv[1]).resolve()
sha = lambda b: hashlib.sha256(b if isinstance(b, bytes) else b.encode()).hexdigest()
with tempfile.TemporaryDirectory(prefix='jiaclaw-skill-fetch-') as temp:
    root = pathlib.Path(temp).resolve()
    def openssl(*args):
        subprocess.run(['openssl', *args], cwd=root, check=True, capture_output=True)
    openssl('req', '-x509', '-nodes', '-newkey', 'rsa:2048', '-days', '1', '-subj', '/CN=JiaClaw disposable test CA', '-keyout', 'ca.key', '-out', 'ca.pem')
    openssl('req', '-nodes', '-newkey', 'rsa:2048', '-subj', '/CN=localhost', '-keyout', 'server.key', '-out', 'server.csr')
    (root/'extensions').write_text('subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n')
    openssl('x509', '-req', '-in', 'server.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key', '-CAcreateserial', '-days', '1', '-extfile', 'extensions', '-out', 'server.pem')
    (root/'empty-roots').mkdir()
    env = {k:v for k,v in os.environ.items() if k not in ['JIACLAW_API_KEY', 'BROKERROUTER_API_KEY']}
    env.update(SSL_CERT_FILE=str(root/'ca.pem'), SSL_CERT_DIR=str(root/'empty-roots'), HTTPS_PROXY='http://127.0.0.1:1', ALL_PROXY='http://127.0.0.1:1', NO_PROXY='')
    reference='exact reference \u4e2d\u6587\n'
    body='---\nname: fetched\ndescription: exact capture\njiaclaw_resources:\n  - path: references/a.md\n    sha256: '+sha(reference)+'\n---\nread only skill\n'
    raw=(json.dumps({'version':1,'directory':'fetched','source':{'repository':'https://example.com/skills','revision':'a'*40,'skill_sha256':sha(body)},'files':[{'path':'SKILL.md','text':body},{'path':'references/a.md','text':reference}]}, ensure_ascii=False, indent=2)+'\n').encode()
    requests=[]; routes={}; held=threading.Event(); release=threading.Event()
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version='HTTP/1.1'
        def handle(self):
            # Certificate/hostname rejection can close before an HTTP request.
            try: super().handle()
            except (ConnectionResetError, ssl.SSLError): pass
        def log_message(self,*a): pass
        def do_GET(self):
            requests.append((self.path,dict(self.headers)))
            code,payload,headers=routes.get(self.path,(200,raw,{}))
            if self.path=='/held': held.set(); release.wait(20)
            try:
                self.send_response(code)
                for k,v in headers.items(): self.send_header(k,v)
                if 'Transfer-Encoding' not in headers and 'Content-Length' not in headers: self.send_header('Content-Length',str(len(payload)))
                self.end_headers()
                if headers.get('Transfer-Encoding')=='chunked':
                    for offset in range(0,len(payload),4096):
                        chunk=payload[offset:offset+4096]; self.wfile.write(f'{len(chunk):x}\r\n'.encode()+chunk+b'\r\n'); self.wfile.flush()
                    self.wfile.write(b'0\r\n\r\n')
                else: self.wfile.write(payload)
                self.wfile.flush(); self.close_connection=True
            except (BrokenPipeError, ConnectionResetError, ssl.SSLError): pass
            finally: self.close_connection=True
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    server.daemon_threads=False  # server_close joins every test-owned handler
    tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); tls.load_cert_chain(root/'server.pem',root/'server.key'); server.socket=tls.wrap_socket(server.socket,server_side=True)
    thread=threading.Thread(target=server.serve_forever,daemon=True); thread.start()
    url=f'https://localhost:{server.server_port}'
    def fixture(label):
        work=root/label; work.mkdir(); (work/'skills.lock.json').write_text('{"version":2,"skills":[]}\n')
        config=work/'config.json'; config.write_text(json.dumps({'agent':{'name':'fetch','description':'disposable','system_instructions':'','max_turns':3,'workspace_path':str(work),'skill_lock_required':True},'provider':{'provider_type':'brokerrouter','base_url':'http://127.0.0.1:1','model':'unused'},'model_calls':{'enabled':True,'store_path':'../state/calls.sqlite3'},'memory':{'semantic':{'enabled':True,'model':'unused','space_revision':'test','dimensions':8,'index_path':'../state/index.sqlite3'}}}))
        return work,config
    def command(f,action,*args): return [str(binary),'skills',action,*args,'--config',str(f[1])]
    def run(f,action,*args,e=env): return subprocess.run(command(f,action,*args),env=e,capture_output=True,text=True,timeout=40)
    def fetch(f,id,path='/bundle',digest=None,private=True,e=env): return run(f,'fetch',url+path,'--operation-id',id,'--if-bundle-sha256',digest or sha(raw),*(['--allow-private'] if private else []),e=e)
    def observe(f,id):
        r=run(f,'fetch-status',id); assert r.returncode==0,r.stderr; return json.loads(r.stdout)
    def op(f,id): return f[0]/'.jiaclaw-skill-fetches'/id
    f=fixture('success'); id=str(uuid.uuid4()); policy=(f[0]/'skills.lock.json').read_bytes(); r=fetch(f,id)
    assert r.returncode==0,r.stderr; state=json.loads(r.stdout)
    assert state['state']=='completed' and state['runtime_applied'] is False and state['bundle_sha256']==sha(raw)
    assert (f[0]/state['bundle_path']).read_bytes()==raw and (f[0]/'skills.lock.json').read_bytes()==policy and not (f[0]/'skills').exists() and not (root/'state').exists()
    assert op(f,id).stat().st_mode&0o777==0o700 and all(p.stat().st_mode&0o777==0o600 and p.stat().st_nlink==1 for p in op(f,id).iterdir())
    count=len(requests); assert fetch(f,id).returncode!=0 and observe(f,id)==state
    assert run(f,'finish-fetch',id,'--if-intent-sha256',state['intent_sha256']).returncode==0 and len(requests)==count
    observed_headers={k.lower():v for k,v in requests[0][1].items()}
    assert observed_headers.get('accept-encoding')=='identity' and 'authorization' not in observed_headers, observed_headers
    install_args=['--from',state['bundle_path'],'--if-bundle-sha256',sha(raw),'--if-manifest-sha256',sha(policy)]
    assert run(f,'install','--operation-id',id,*install_args).returncode!=0
    assert (f[0]/'skills.lock.json').read_bytes()==policy and not (f[0]/'skills').exists()
    installed_id=str(uuid.uuid4())
    installed=run(f,'install','--operation-id',installed_id,*install_args)
    assert installed.returncode==0,installed.stderr
    assert (f[0]/'skills/fetched/SKILL.md').read_text()==body and (f[0]/'skills/fetched/references/a.md').read_text()==reference
    assert not json.loads((f[0]/'skills.lock.json').read_text())['skills'][0]['enabled'] and len(requests)==count
    assert fetch(f,installed_id).returncode!=0 and len(requests)==count
    print('PASS skill fetch 1: actual verified TLS/UTF8 exact bytes, no ambient initialization/proxy/activation, same UUID never GETs twice',flush=True)

    f=fixture('admission'); count=len(requests)
    for u in ['http://localhost:1/a',url+'/a?q=token',url+'/a#fragment',url.replace('://','://user:password@'),url.replace('://','://@')]:
        assert run(f,'fetch',u,'--operation-id',str(uuid.uuid4()),'--if-bundle-sha256',sha(raw),'--allow-private').returncode!=0
    assert fetch(f,str(uuid.uuid4()),private=False).returncode!=0
    assert fetch(f,'not-a-uuid').returncode!=0 and fetch(f,str(uuid.uuid4()),digest='bad').returncode!=0
    assert not (f[0]/'.jiaclaw-skill-fetches').exists() and len(requests)==count
    f=fixture('wrong-host'); u=url.replace('localhost','127.0.0.1'); id=str(uuid.uuid4())
    assert run(f,'fetch',u,'--operation-id',id,'--if-bundle-sha256',sha(raw),'--allow-private').returncode!=0
    assert observe(f,id)['failure']=='transport' and len(requests)==count
    f=fixture('untrusted-ca'); id=str(uuid.uuid4()); (root/'no-ca.pem').write_bytes(b'')
    assert fetch(f,id,e={**env,'SSL_CERT_FILE':str(root/'no-ca.pem')}).returncode!=0
    assert observe(f,id)['failure']=='transport' and len(requests)==count
    print('PASS skill fetch 2: URL/hash/UUID/private-network admission precedes identity/GET; actual TLS hostname mismatch rejects',flush=True)

    invalid=json.loads(raw); invalid['files'].pop(); invalid=json.dumps(invalid).encode()
    cases=[('redirect',302,b'',{'Location':url+'/follow'},'unexpected_status'),('status',404,b'no',{},'unexpected_status'),('encoding',200,raw,{'Content-Encoding':'gzip'},'encoding'),('length',200,b'',{'Content-Length':str(512*1024+1)},'too_large'),('chunked',200,b'x'*(512*1024+1),{'Transfer-Encoding':'chunked'},'too_large'),('utf8',200,b'\xff',{},'invalid_utf8'),('digest',200,raw+b' ',{},'digest_mismatch'),('invalid-bundle',200,invalid,{},'invalid_bundle')]
    for name,code,payload,headers,error in cases:
        routes['/'+name]=(code,payload,headers); f=fixture(name); id=str(uuid.uuid4()); before=len(requests)
        r=fetch(f,id,'/'+name,digest=sha(payload) if error in ['invalid_utf8','invalid_bundle'] else sha(raw)); assert r.returncode!=0,(name,r.stdout,r.stderr)
        state=observe(f,id); assert state['state']=='failed' and state['failure']==error and not (op(f,id)/'bundle.json').exists(),(name,state)
        assert fetch(f,id,'/'+name).returncode!=0 and len(requests)==before+1
    print('PASS skill fetch 3: actual redirects/status/encoding/declared and chunked budgets/UTF8/digest/full references reject without auto retry or capture',flush=True)

    f=fixture('conflict'); id=str(uuid.uuid4()); assert fetch(f,id).returncode==0
    receipt=op(f,id)/'receipt.json'; saved=receipt.read_bytes(); inode=receipt.stat().st_ino; receipt.unlink()
    captured=observe(f,id); assert captured['state']=='captured'; count=len(requests)
    assert run(f,'finish-fetch',id,'--if-intent-sha256','0'*64).returncode!=0 and not receipt.exists()
    assert run(f,'finish-fetch',id,'--if-intent-sha256',captured['intent_sha256']).returncode==0 and len(requests)==count
    bundle=op(f,id)/'bundle.json'; original=bundle.read_bytes(); bundle.write_bytes(original+b' ')
    inode=bundle.stat().st_ino; assert run(f,'fetch-status',id).returncode!=0 and run(f,'finish-fetch',id,'--if-intent-sha256',captured['intent_sha256']).returncode!=0
    assert bundle.read_bytes()==original+b' ' and bundle.stat().st_ino==inode and len(requests)==count
    print('PASS skill fetch 4: captured-only conditional finish is GET-free; changed original bytes refuse and preserve evidence',flush=True)

    interposer=root/('fsync.dylib' if sys.platform=='darwin' else 'fsync.so')
    subprocess.run(['cc','-dynamiclib' if sys.platform=='darwin' else '-shared','-fPIC',str(pathlib.Path(__file__).with_name('skill_install_fsync.c')),'-o',str(interposer),*([] if sys.platform=='darwin' else ['-ldl'])],check=True,capture_output=True)
    for phase in ['intent','bundle','receipt']:
        f=fixture('kill-'+phase); id=str(uuid.uuid4()); marker=root/('held-'+phase)
        fault={**env,('DYLD_INSERT_LIBRARIES' if sys.platform=='darwin' else 'LD_PRELOAD'):str(interposer),'JIACLAW_TEST_FETCH_WORKSPACE':str(f[0]),'JIACLAW_TEST_FETCH_ID':id,'JIACLAW_TEST_FETCH_PHASE':phase,'JIACLAW_TEST_FETCH_MARKER':str(marker)}
        p=subprocess.Popen(command(f,'fetch',url+'/bundle','--operation-id',id,'--if-bundle-sha256',sha(raw),'--allow-private'),env=fault,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            wait_stopped(p,time.monotonic()+15)
            assert marker.exists()
            assert run(f,'fetch-status',id).returncode!=0
            p.kill(); p.communicate(timeout=10); assert p.returncode==-signal.SIGKILL
        finally:
            if p.poll() is None: p.kill(); p.communicate(timeout=10)
        state=observe(f,id); count=len(requests)
        if phase=='intent':
            assert state['state']=='outcome_unknown' and run(f,'finish-fetch',id,'--if-intent-sha256',state['intent_sha256']).returncode!=0 and fetch(f,str(uuid.uuid4())).returncode!=0
        else:
            assert state['state']==('captured' if phase=='bundle' else 'completed')
            assert run(f,'finish-fetch',id,'--if-intent-sha256',state['intent_sha256']).returncode==0
        assert len(requests)==count
    print('PASS skill fetch 5: three actual sync/SIGSTOP/SIGKILL phases, real owner refusal, GET-free original recovery and unresolved new-request gate',flush=True)

    f=fixture('held-get'); id=str(uuid.uuid4()); held.clear(); release.clear()
    p=subprocess.Popen(command(f,'fetch',url+'/held','--operation-id',id,'--if-bundle-sha256',sha(raw),'--allow-private'),env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    try:
        assert held.wait(10); count=len(requests)
        assert run(f,'fetch-status',id).returncode!=0 and fetch(f,str(uuid.uuid4())).returncode!=0 and len(requests)==count
        p.kill(); p.communicate(timeout=10); release.set()
        state=observe(f,id); assert state['state']=='outcome_unknown'
        assert run(f,'finish-fetch',id,'--if-intent-sha256',state['intent_sha256']).returncode!=0 and len(requests)==count
    finally:
        release.set()
        if p.poll() is None:p.kill();p.communicate(timeout=10)
    print('PASS skill fetch 6: actual HTTPS GET hold retains original writer; SIGKILL and finish never resubmit',flush=True)
    f=fixture('competing'); id=str(uuid.uuid4()); held.clear(); release.clear(); before=len(requests)
    p=subprocess.Popen(command(f,'fetch',url+'/held','--operation-id',id,'--if-bundle-sha256',sha(raw),'--allow-private'),env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    try:
        assert held.wait(10)
        assert fetch(f,id).returncode!=0 and len(requests)==before+1
        release.set(); stdout,stderr=p.communicate(timeout=15)
        assert p.returncode==0,(stdout,stderr)
        assert observe(f,id)['state']=='completed' and len(requests)==before+1
    finally:
        release.set()
        if p.poll() is None: p.kill();p.communicate(timeout=10)
    # Populate bounded historical receipts by administrator setup, then exercise
    # the real public admission. This does not claim 64 independent network GETs.
    intent=json.loads((op(f,id)/'intent.json').read_text()); receipt=json.loads((op(f,id)/'receipt.json').read_text())
    for _ in range(63):
        another=str(uuid.uuid4()); path=op(f,another); path.mkdir(mode=0o700)
        i={**intent,'operation_id':another}; i_raw=json.dumps(i)
        r={**receipt,'operation_id':another,'intent_sha256':sha(i_raw),'bundle_path':f'.jiaclaw-skill-fetches/{another}/bundle.json'}
        for name,data in [('intent.json',i_raw.encode()),('bundle.json',raw),('receipt.json',json.dumps(r).encode())]:
            (path/name).write_bytes(data); (path/name).chmod(0o600)
    assert fetch(f,str(uuid.uuid4())).returncode!=0 and len(requests)==before+1
    assert len(list((f[0]/'.jiaclaw-skill-fetches').iterdir()))==64
    print('PASS skill fetch 7: real same-UUID competition admits one GET; 64-record bounded historical setup rejects next identity without GET',flush=True)
    server.shutdown();server.server_close();thread.join(10);assert not thread.is_alive()
