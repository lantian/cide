#!/usr/bin/env python3
"""Quota-free OpenCode 2 peer-tool audit: real CLI, local model and fake MCP.

Run from the repository root: python3 scripts/audit-opencode-peer-tools.py.
All CLI state is temporary. No model credentials or real agent messages are used.
Compare old tool/directory permissions with paired tools, then read external sources and reply.
"""
import json, os, subprocess, tempfile, time, threading, urllib.request, socket, base64
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

for direct in (False, True):
 with tempfile.TemporaryDirectory(prefix='cide-peer-reference-',dir=Path.cwd()/'target') as reference, tempfile.TemporaryDirectory(prefix='cide-peer-native-tools-') as tmp:
  source=Path(reference)/'reference.txt'
  source.write_text('External source audit content\n')
  captured=[]; arrived=threading.Event()
  class Provider(BaseHTTPRequestHandler):
   def do_POST(self):
    captured.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))));arrived.set()
    self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
    tools=[t.get('function',{}).get('name') for t in captured[-1].get('tools',[])]
    invoke=not any(m.get('role')=='tool' for m in captured[-1].get('messages',[]))
    delta={'role':'assistant'}
    if invoke:
     calls=[('read',{'path':str(source)}),('glob',{'pattern':'*.txt','path':reference})]
     if 'cide_cide_peer_chat_send' in tools:calls.append(('cide_cide_peer_chat_send',{'message':'Peer audit reply'}))
     delta['tool_calls']=[{'index':i,'id':f'call_{i}','type':'function','function':{'name':name,'arguments':json.dumps(args)}} for i,(name,args) in enumerate(calls)]
    else:delta['content']='Audit complete'
    for event in [{'delta':delta,'finish_reason':None},{'delta':{},'finish_reason':'tool_calls' if invoke else 'stop'}]:
     self.wfile.write(('data: '+json.dumps({'id':'audit','object':'chat.completion.chunk','created':0,'model':'test','choices':[{'index':0,**event}]})+'\n\n').encode())
    self.wfile.write(b'data: [DONE]\n\n')
   def log_message(self,*args):pass
  provider=ThreadingHTTPServer(('127.0.0.1',0),Provider)
  threading.Thread(target=provider.serve_forever,daemon=True).start()
  mcp=Path(tmp)/'mcp.py'
  mcp.write_text('''import sys,json
for line in sys.stdin:
 v=json.loads(line); method=v.get('method');id=v.get('id')
 if id is None:continue
 if method=='initialize':r={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'peer-audit','version':'1'}}
 elif method=='tools/list':r={'tools':[{'name':'cide_peer_chat_send','description':'Send a message to the other agent','inputSchema':{'type':'object','properties':{'message':{'type':'string'}},'required':['message']}},{'name':'unsafe_write','description':'Must be hidden','inputSchema':{'type':'object'}}]}
 elif method=='tools/call':
  with open(__file__+'.sent','w') as log:log.write(json.dumps(v['params']))
  r={'content':[{'type':'text','text':'Accepted'}]}
 else:r={}
 print(json.dumps({'jsonrpc':'2.0','id':id,'result':r}),flush=True)
''')
  rules=[{'action':'*','resource':'*','effect':'deny'}]+[{'action':x,'resource':'*','effect':'allow'} for x in ('read','glob','grep','list','cide_cide_peer_chat_send')]+([{'action':'external_directory','resource':'*','effect':'allow'}] if direct else [])
  server={'type':'local','command':['python3',str(mcp)]}
  if direct:server['codemode']=False
  config={'providers':{'peer-audit':{'package':'aisdk:@ai-sdk/openai-compatible','settings':{'baseURL':f'http://127.0.0.1:{provider.server_port}/v1','apiKey':'local-test'},'models':{'test':{'name':'Test','limit':{'context':32000,'output':1000}}}}},'mcp':{'servers':{'cide':server}},'permissions':rules,'agents':{'cide-peer-reviewer':{'system':'Send your reply with cide_cide_peer_chat_send','permissions':rules}}}
  env=dict(os.environ,OPENCODE_CONFIG_CONTENT=json.dumps(config),OPENCODE_SERVER_PASSWORD='peer-audit-only',OPENCODE_SERVER_USERNAME='opencode',XDG_CONFIG_HOME=tmp+'/config',XDG_DATA_HOME=tmp+'/data',XDG_STATE_HOME=tmp+'/state',XDG_CACHE_HOME=tmp+'/cache')
  s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
  def request(path,body=None):
   headers={'Authorization':'Basic '+base64.b64encode(b'opencode:peer-audit-only').decode(),'Content-Type':'application/json'}
   r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=json.dumps(body).encode() if body is not None else None,headers=headers)
   return json.load(urllib.request.urlopen(r,timeout=3))
  with open(tmp+'/server.log','w+') as log:
   process=subprocess.Popen(['opencode','serve','--hostname','127.0.0.1','--port',str(port)],cwd=tmp,env=env,stdout=log,stderr=log,start_new_session=True)
   try:
    for _ in range(150):
     try:request('/api/mcp');break
     except Exception:time.sleep(.1)
    else:raise Exception('OpenCode server did not start')
    for _ in range(100):
     status=request('/api/mcp')
     if 'connected' in json.dumps(status):break
     time.sleep(.1)
    else:raise Exception('Fake MCP did not connect: '+str(status))
    # Connected precedes asynchronous tool registration; let the catalog settle.
    time.sleep(1)
    session=request('/api/session',{'title':'Peer reply tool audit','agent':'cide-peer-reviewer','model':{'providerID':'peer-audit','id':'test'},'location':{'directory':tmp}})['data']['id']
    request('/api/session/'+session+'/prompt',{'text':'Send a peer reply','resume':True})
    if not arrived.wait(20):
     log.seek(0);raise Exception('No local model request; '+log.read()[-1500:])
    time.sleep(1)
    tools=[t.get('function',{}).get('name',t.get('name')) for body in captured for t in body.get('tools',[])]
    print('direct=',direct,'model-visible tools=',sorted(set(tools)),flush=True)
    assert ('cide_cide_peer_chat_send' in tools)==direct
    assert all(x not in tools for x in ('execute','write','edit','patch','shell','subagent','cide_unsafe_write'))
    for _ in range(100):
     results=[m for body in captured for m in body.get('messages',[]) if m.get('role')=='tool']
     if len(results)>=(3 if direct else 2):break
     time.sleep(.1)
    read_result=next(m for m in results if m.get('tool_call_id')=='call_0')
    glob_result=next(m for m in results if m.get('tool_call_id')=='call_1')
    if not direct:
     assert 'External source audit content' not in json.dumps(read_result), read_result
     assert 'external_directory' in json.dumps(read_result), read_result
     assert 'external_directory' in json.dumps(glob_result), glob_result
     print('Old reviewer permissions reject external read/glob')
    if direct:
     sent=Path(str(mcp)+'.sent')
     for _ in range(100):
      if sent.exists():break
      time.sleep(.1)
     assert sent.exists(), 'Reply tool was visible but was not called'
     assert json.loads(sent.read_text())['arguments']['message']=='Peer audit reply'
     assert 'External source audit content' in json.dumps(read_result), read_result
     assert str(source) in json.dumps(glob_result), glob_result
     assert source.read_text()=='External source audit content\n'
     print('External source read/glob succeeded; peer reply reached the fake MCP receiver')
   finally:
    import signal
    try:os.killpg(process.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    try:process.wait(timeout=5)
    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    provider.shutdown();provider.server_close()
print('OpenCode peer reply visibility verified without model charges')
