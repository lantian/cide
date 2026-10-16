#!/usr/bin/env python3
"""Quota-free OpenCode 2 peer-tool audit: real CLI, local model and fake MCP.

Run from the repository root: python3 scripts/audit-opencode-peer-tools.py.
All CLI state is temporary. No model credentials or real agent messages are used.
Compare old permissions with native reply, analysis and web tools. Read external
sources, run a temporary Python analysis script, fetch a URL, search the internet
through OpenCode's native keyless pool, and send a peer reply. No provider selection
or credentials are persisted outside the temporary audit state.
"""
import json, os, subprocess, tempfile, time, threading, urllib.request, socket, base64, shlex
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

policy=json.loads(os.environ.get('CIDE_PEER_AUDIT_POLICY','null'))
for direct,analysis,search in ((False,False,False),(True,False,False),(True,True,False),(True,True,True)):
 with tempfile.TemporaryDirectory(prefix='cide-peer-reference-',dir=Path.cwd()/'target') as reference, tempfile.TemporaryDirectory(prefix='cide-peer-native-tools-') as tmp:
  source=Path(reference)/'reference.txt'
  source.write_text('External source audit content\n')
  scratch=Path(policy['scratch']) if policy and analysis else Path(tmp)/'scratch'
  scratch.mkdir(exist_ok=True)
  script='import pathlib,hashlib; p=pathlib.Path('+repr(str(source))+'); print("ANALYSIS_OK", hashlib.sha256(p.read_bytes()).hexdigest())'
  launcher='import pathlib,subprocess,os; p=pathlib.Path(os.environ["CIDE_PEER_SCRATCH"])/"analysis.py"; p.write_text('+repr(script)+'); subprocess.run(["python3",str(p)],check=True)'
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
     if analysis:
      calls.append(('shell',{'command':'python3 -c '+shlex.quote(launcher)}))
      calls.append(('webfetch',{'url':'https://example.com','format':'text'}))
      calls.append(('websearch',{'query':'Rust programming language official website'}))
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
  if analysis:
   rules=policy['opencodePermissions'] if policy else rules+[{'action':x,'resource':'*','effect':'allow'} for x in ('shell','webfetch','websearch')]
  server={'type':'local','command':['python3',str(mcp)]}
  if direct:server['codemode']=False
  config={'providers':{'peer-audit':{'package':'aisdk:@ai-sdk/openai-compatible','settings':{'baseURL':f'http://127.0.0.1:{provider.server_port}/v1','apiKey':'local-test'},'models':{'test':{'name':'Test','limit':{'context':32000,'output':1000}}}}},'mcp':{'servers':{'cide':server}},'permissions':rules,'agents':{'cide-peer-reviewer':{'system':'Send your reply with cide_cide_peer_chat_send','permissions':rules}}}
  if search:config['websearch']=policy['opencodeWebsearch'] if policy else {'provider':'random'}
  search_keys={'EXA_API_KEY','FIRECRAWL_API_KEY','PARALLEL_API_KEY','TAVILY_API_KEY','TINYFISH_API_KEY'}
  env={k:v for k,v in os.environ.items() if not k.startswith(('OPENCODE_','OPENAI_','ANTHROPIC_','CODEX_','CIDE_')) and k not in search_keys}
  env.update(OPENCODE_CONFIG_CONTENT=json.dumps(config),OPENCODE_SERVER_PASSWORD='peer-audit-only',OPENCODE_SERVER_USERNAME='opencode',XDG_CONFIG_HOME=tmp+'/config',XDG_DATA_HOME=tmp+'/data',XDG_STATE_HOME=tmp+'/state',XDG_CACHE_HOME=tmp+'/cache')
  env.update(CIDE_PEER_SCRATCH=str(scratch),TMPDIR=str(scratch),PYTHONDONTWRITEBYTECODE='1',GIT_OPTIONAL_LOCKS='0')
  s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
  def request(path,body=None,method=None):
   headers={'Authorization':'Basic '+base64.b64encode(b'opencode:peer-audit-only').decode(),'Content-Type':'application/json'}
   r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=json.dumps(body).encode() if body is not None else None,headers=headers,method=method)
   with urllib.request.urlopen(r,timeout=3) as response:
    data=response.read()
    return json.loads(data) if data else None
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
    print('direct=',direct,'analysis=',analysis,'search=',search,'model-visible tools=',sorted(set(tools)),flush=True)
    assert ('cide_cide_peer_chat_send' in tools)==direct
    assert all(x not in tools for x in ('execute','write','edit','patch','subagent','cide_unsafe_write'))
    assert ('shell' in tools)==analysis
    if analysis:assert {'webfetch','websearch'} <= set(tools),tools
    cancelled_provider_form=False
    for _ in range(900):
     if analysis:
      forms=request('/api/session/'+session+'/form')['data']
      if search:assert not forms, 'Configured search must not ask for provider selection: '+str(forms)
      elif forms and not cancelled_provider_form:
       form=forms[0].get('form',forms[0])
       assert form.get('metadata',{}).get('kind')=='websearch.provider',form
       request('/api/session/'+session+'/form/'+form['id'],method='DELETE')
       cancelled_provider_form=True
     results=[m for body in captured for m in body.get('messages',[]) if m.get('role')=='tool']
     if len(results)>=(6 if analysis else 3 if direct else 2):break
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
     if analysis:
      shell_result=next(m for m in results if m.get('tool_call_id')=='call_2')
      fetch_result=next(m for m in results if m.get('tool_call_id')=='call_3')
      search_result=next(m for m in results if m.get('tool_call_id')=='call_4')
      assert 'ANALYSIS_OK' in json.dumps(shell_result),shell_result
      assert 'Example Domain' in json.dumps(fetch_result),fetch_result
      assert (scratch/'analysis.py').is_file()
      assert source.read_text()=='External source audit content\n'
      if search:
       assert not cancelled_provider_form
       assert 'https://' in search_result['content'] or search_result['content']=='No search results found. Please try a different query.',search_result
       assert 'Web search cancelled' not in search_result['content'],search_result
       print('Actual websearch completed without a provider-selection form; result URLs='+str(search_result['content'].count('https://')))
      else:
       assert cancelled_provider_form
       assert 'Web search cancelled' in search_result['content'],search_result
       print('Reproduced first-use provider form cancellation with the old search configuration')
      print('Temporary Python analysis and URL fetch succeeded; source unchanged')
   finally:
    import signal
    try:os.killpg(process.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    try:process.wait(timeout=5)
    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    provider.shutdown();provider.server_close()
print('OpenCode peer reply visibility verified without model charges')
