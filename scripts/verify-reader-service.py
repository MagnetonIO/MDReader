#!/usr/bin/env python3
"""Synthetic packaged-entrypoint acceptance. Never opens an installed profile."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',required=True,type=Path)
    parser.add_argument('--output',type=Path)
    parser.add_argument('--model-cache',type=Path,help='Explicit public BGE cache for positive semantic/restart acceptance')
    args=parser.parse_args()
    binary=args.binary.resolve(strict=True)
    checks=[]
    with tempfile.TemporaryDirectory(prefix='mdreader-service-acceptance-') as temporary:
        root=Path(temporary)
        data=root/'private library';notes=root/'synthetic worktree'
        data.mkdir();notes.mkdir()
        if args.model_cache:
            (data/'models').symlink_to(args.model_cache.resolve(strict=True),target_is_directory=True)
            (notes/'solar.md').write_text('# Rooftop energy\nPhotovoltaic panels convert sunlight into electrical power for homes.\n')
        original=b'# Original\nSource-owned Markdown remains untouched.\n'
        (notes/'original.md').write_bytes(original)
        service=None
        def call(method,params=None):
            endpoint=json.loads((data/'reader-endpoint.json').read_text())
            request={'method':method}
            if params is not None: request['params']=params
            with socket.create_connection(('127.0.0.1',endpoint['port']),timeout=3) as stream:
                stream.sendall(json.dumps({'protocol':1,'token':endpoint['token'],'request':request}).encode()+b'\n')
                response=json.loads(stream.makefile('rb').readline(16*1024*1024+1))
            if response['error']: raise AssertionError(response['error'])
            return response['result']
        def await_ready():
            for _ in range(200):
                try:
                    call('ping');return
                except (OSError,ValueError,KeyError): time.sleep(.02)
            raise AssertionError('service readiness deadline')
        def hook(policy,payload):
            output=subprocess.run([str(binary),'--mdreader-capture','hook','claude',str(policy),str(data/'captures')],input=json.dumps(payload).encode(),capture_output=True,timeout=5)
            assert output.returncode==0,output.stderr.decode()
            assert output.stdout.strip()==b'{}'
        def mcp(source,requests):
            frames=[{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-11-25','capabilities':{},'clientInfo':{'name':'synthetic-acceptance','version':'1'}}},
                    {'jsonrpc':'2.0','method':'notifications/initialized'}]+requests
            output=subprocess.run([str(binary),'--mdreader-mcp','--data-dir',str(data),'--source',source],input=('\n'.join(json.dumps(frame) for frame in frames)+'\n').encode(),capture_output=True,timeout=15)
            assert output.returncode==0,output.stderr.decode()
            replies=[json.loads(line) for line in output.stdout.splitlines()]
            assert len(replies)==1+len(requests),'notifications must not produce responses'
            return replies[1:]
        try:
            launch=[str(binary),'--mdreader-service']+([] if args.model_cache else ['--no-model'])+[str(data)]
            service=subprocess.Popen(launch,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            await_ready()
            identity=notes.stat()
            original_source=call('admit',{'name':'Synthetic notes','path':str(notes),'identity':{'platform':'unix','device':identity.st_dev,'inode':identity.st_ino}})['id']
            call('index',{'source_id':original_source})
            if args.model_cache:
                call('start_index',{'source_id':original_source})
                for _ in range(500):
                    outcome=call('search_ranked',{'query':'generate electricity from sunshine','sources':[original_source],'limit':10,'mode':'semantic'})
                    if outcome['hits']:
                        assert outcome['hits'][0]['path']=='solar.md'
                        assert outcome['hits'][0]['semantic_rank']==1
                        assert outcome['semantic_unavailable'] is None
                        checks.append('actual BGE paraphrase retrieval through packaged service')
                        break
                    time.sleep(.02)
                else: raise AssertionError('real model indexing deadline')
            checks.append('packaged headless service indexes source-owned Markdown')
            policy={'enabled':True,'project_id':'synthetic-packaged-project','project_root':str(notes),'repository_root':str(notes),'worktree_roots':[str(notes)],'capture_subagents':False,'excluded_paths':[],'excluded_sessions':[],'selected_backfill_sessions':[]}
            configured=call('configure_capture',{'policy':policy})
            policy_path=Path(configured['policyPath'])
            assert policy_path.stat().st_mode & 0o777==0o600
            hook(policy_path,{'hook_event_name':'Stop','session_id':'synthetic-session','cwd':str(notes),'last_assistant_message':'# Cross-agent handoff\nPackagedcaptureverification works without a reader window.'})
            captured=None
            for _ in range(250):
                sources=call('list_sources')
                capture_source=next(source['id'] for source in sources if source['name']=='Agent captures')
                hits=call('search',{'query':'Packagedcaptureverification','sources':[capture_source],'limit':10})
                if hits:
                    captured=hits[0];break
                time.sleep(.02)
            assert captured and captured['revision'] and captured['line_start']>0
            checks.append('packaged hook automatically projects, admits and indexes visible output')
            assert call('capture_status')['pending_events']==0
            service.terminate();service.wait(timeout=5);service=None
            request={'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'read_document','arguments':{'source_id':capture_source,'path':captured['path'],'revision':captured['revision']}}}
            denied={'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'search','arguments':{'query':'Original','sources':[original_source]}}}
            replies=mcp(capture_source,[request,denied])
            result=replies[0]['result']
            assert not result['isError']
            document=result['structuredContent']['data']
            assert document['revision']==captured['revision']
            assert 'Packagedcaptureverification' in document['markdown']
            assert replies[1]['result']['isError'],'explicit source scope must be enforced'
            checks.append('packaged MCP bootstraps service after restart and retrieves exact captured revision')
            checks.append('MCP rejects a source outside its explicit grant')
            hook(policy_path,{'hook_event_name':'Stop','session_id':'echo-session','cwd':str(notes),'last_assistant_message':result['content'][0]['text']})
            assert call('capture_status')['pending_events']==0
            assert call('capture_status')['admitted_occurrences']==1
            assert (notes/'original.md').read_bytes()==original
            checks.append('MCP retrieval echo is excluded by actual packaged capture hook')
            checks.append('source-owned original bytes are unchanged')
            if args.model_cache:
                # The MCP launcher just recovered the completed index and model
                # state. Require real semantic query success without reindexing.
                for _ in range(500):
                    outcome=call('search_ranked',{'query':'generate electricity from sunshine','sources':[original_source],'limit':10,'mode':'semantic'})
                    if outcome['hits']:
                        assert outcome['hits'][0]['path']=='solar.md'
                        checks.append('service restart restores semantic retrieval without reindexing')
                        break
                    time.sleep(.02)
                else: raise AssertionError('restart model recovery deadline')
            else:
                assert not (data/'models').exists(),'lexical-only acceptance must not download a model'
        finally:
            if service is not None:
                service.terminate();service.wait(timeout=5)
            try:
                endpoint=json.loads((data/'reader-endpoint.json').read_text())
                if endpoint['pid']!=os.getpid(): os.kill(endpoint['pid'],signal.SIGTERM)
            except (OSError,ValueError,KeyError): pass
    receipt={'kind':'synthetic-packaged-entrypoint-acceptance','binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'checks':checks,'actual_bge':bool(args.model_cache),'accepts_gui':False,'accepts_mobile':False,'accepts_cloud_sync':False,'touches_installed_profile':False}
    encoded=json.dumps(receipt,indent=2)+'\n'
    if args.output:
        args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(encoded)
    print(encoded,end='')

if __name__=='__main__': main()
