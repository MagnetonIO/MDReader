// Synthetic local integration fixture only. Never reads a profile or deploys a Worker.
import { createRequire } from 'node:module';
import { createServer } from 'node:https';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import path from 'node:path';
const [web, temp] = process.argv.slice(2);
const require = createRequire(path.join(web, 'package.json'));
const { Miniflare, convertV4MiniflareOptions } = require('miniflare');
const { generateKeyPair, exportJWK, SignJWT } = await import(require.resolve('jose'));
const key=path.join(temp,'tls-key.pem'),cert=path.join(temp,'tls-cert.pem'),root=path.join(temp,'tls-root.pem'),rootKey=path.join(temp,'tls-root-key.pem'),csr=path.join(temp,'tls.csr'),ext=path.join(temp,'tls.ext');
writeFileSync(ext,'subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n');
for(const args of [
 ['req','-x509','-newkey','rsa:2048','-nodes','-keyout',rootKey,'-out',root,'-days','1','-subj','/CN=MDReader Synthetic Test Root','-addext','basicConstraints=critical,CA:TRUE'],
 ['req','-newkey','rsa:2048','-nodes','-keyout',key,'-out',csr,'-subj','/CN=localhost'],
 ['x509','-req','-in',csr,'-CA',root,'-CAkey',rootKey,'-CAcreateserial','-out',cert,'-days','1','-extfile',ext]
]){if(spawnSync('openssl',args,{stdio:'ignore'}).status!==0)throw new Error('synthetic TLS fixture unavailable');}
const issuerURL='https://native-synthetic-issuer.example.test';
const apiAudience='https://mdreader.dev/sync';
const issuer=await generateKeyPair('RS256',{extractable:true});
const jwk={...await exportJWK(issuer.publicKey),alg:'RS256',kid:'synthetic'};
const runtime=new Miniflare(convertV4MiniflareOptions({modules:true,scriptPath:path.join(web,'.worker-build/sync/index.js'),compatibilityDate:'2026-10-01',durableObjects:{READER_ACCOUNTS:{className:'ReaderAccount',useSQLite:true}},r2Buckets:['ENCRYPTED_BLOBS'],bindings:{SYNC_ENABLED:'development',BROWSER_ORIGIN:'https://reader.example.test',OIDC_ISSUER:issuerURL,OIDC_AUDIENCE:apiAudience,OIDC_CLIENT_IDS:JSON.stringify(['native-synthetic']),OIDC_JWKS_URL:issuerURL+'/keys',ENROLLMENT_SECRET:'synthetic-only-server-secret-at-least-32-bytes'},outboundService:async request=>{if(request.url!==issuerURL+'/keys')return new Response(null,{status:404});return Response.json({keys:[jwk]});}}));
await runtime.ready;
const codes=new Map();
let origin, dropBootstrap=false, dropCommit=false, dropAck=false, weakUpload=false, stall=false, release, forbidden=false, redirect=false, oversized=false;
const stats={oauthExchanges:0,pkceRejected:0,uploadHashes:[],commitHashes:[],acks:[],waiting:false};
const hash=b=>createHash('sha256').update(b).digest('hex');
const server=createServer({key:readFileSync(key),cert:readFileSync(cert)},async(req,res)=>{
  try {
    const chunks=[];let n=0;for await(const c of req){n+=c.length;if(n>2*1024*1024+8192){res.writeHead(413).end();return;}chunks.push(c);}
    const body=Buffer.concat(chunks),url=new URL(req.url,origin);
    if(url.pathname==='/__synthetic/keys'){res.setHeader('content-type','application/json');res.end(JSON.stringify({keys:[jwk]}));return;}
    if(url.pathname==='/__synthetic/oauth/authorize'){
      const q=url.searchParams,redirect=new URL(q.get('redirect_uri'));
      if(req.method!=='GET'||q.get('client_id')!=='native-synthetic'||q.get('response_type')!=='code'||q.get('scope')!=='openid sync:read sync:write'||q.get('audience')!==apiAudience||q.get('code_challenge_method')!=='S256'||!['state','nonce','code_challenge'].every(k=>/^[A-Za-z0-9_-]{43}$/.test(q.get(k)||''))||redirect.protocol!=='http:'||redirect.hostname!=='127.0.0.1'||!redirect.port||redirect.pathname!=='/mdreader-sync/callback'||redirect.search||redirect.hash||codes.size>=16){res.writeHead(400).end();return;}
      const code=randomUUID();codes.set(code,{nonce:q.get('nonce'),challenge:q.get('code_challenge'),redirect:redirect.toString(),expires:Date.now()+300000});
      redirect.searchParams.set('state',q.get('state'));redirect.searchParams.set('code',code);redirect.searchParams.set('iss',issuerURL);
      res.writeHead(302,{location:redirect.toString()}).end();return;
    }
    if(url.pathname==='/__synthetic/oauth/token'){
      const q=new URLSearchParams(body.toString()),code=q.get('code'),record=codes.get(code);codes.delete(code);
      if(req.method!=='POST'||!record||record.expires<Date.now()||q.get('grant_type')!=='authorization_code'||q.get('client_id')!=='native-synthetic'||q.get('redirect_uri')!==record.redirect||createHash('sha256').update(q.get('code_verifier')||'').digest('base64url')!==record.challenge){stats.pkceRejected++;res.writeHead(400).end();return;}
      stats.oauthExchanges++;
      const token=await new SignJWT({nonce:record.nonce}).setProtectedHeader({alg:'RS256',kid:'synthetic'}).setIssuer(issuerURL).setSubject('synthetic-alice').setAudience('native-synthetic').setIssuedAt().setExpirationTime('5m').sign(issuer.privateKey);
      const access=await new SignJWT({client_id:'native-synthetic',scope:'sync:read sync:write'}).setProtectedHeader({alg:'RS256',kid:'synthetic',typ:'at+jwt'}).setIssuer(issuerURL).setSubject('synthetic-alice').setAudience(apiAudience).setJti(randomUUID()).setIssuedAt().setExpirationTime('5m').sign(issuer.privateKey);
      res.setHeader('content-type','application/json');res.end(JSON.stringify({id_token:token,access_token:access,token_type:'Bearer',expires_in:300}));return;
    }
    if(url.pathname==='/__synthetic/token'){
      const input=JSON.parse(body.toString());
      const token=await new SignJWT(input.nonce?{nonce:input.nonce}:{}).setProtectedHeader({alg:'RS256',kid:'synthetic'}).setIssuer(issuerURL).setSubject('synthetic-alice').setAudience('native-synthetic').setIssuedAt().setExpirationTime('5m').sign(issuer.privateKey);
      const access=await new SignJWT({client_id:'native-synthetic',scope:'sync:read sync:write'}).setProtectedHeader({alg:'RS256',kid:'synthetic',typ:'at+jwt'}).setIssuer(issuerURL).setSubject('synthetic-alice').setAudience(apiAudience).setJti(randomUUID()).setIssuedAt().setExpirationTime('5m').sign(issuer.privateKey);
      res.setHeader('content-type','application/json');res.end(JSON.stringify({token:access,loginToken:token}));return;
    }
    if(url.pathname==='/__synthetic/control'){
      const input=JSON.parse(body.toString());
      if(input.release){stall=false;release?.();release=undefined;stats.waiting=false;}
      if(input.dropBootstrap)dropBootstrap=true;if(input.dropCommit)dropCommit=true;if(input.dropAck)dropAck=true;if(input.weakUpload)weakUpload=true;if(input.stall)stall=true;
      if(typeof input.forbidden==='boolean')forbidden=input.forbidden;if(input.redirect)redirect=true;if(input.oversized)oversized=true;
      res.end(JSON.stringify(stats));return;
    }
    if(redirect){redirect=false;res.writeHead(302,{location:'https://untrusted.example.test/v1/changes'}).end();return;}
    if(oversized){oversized=false;res.writeHead(200,{'content-length':300000});res.end(Buffer.alloc(300000));return;}
    if(forbidden){res.writeHead(403).end();return;}
    if(req.method==='PUT'){
      stats.uploadHashes.push(hash(body));
      if(stall){stats.waiting=true;await new Promise(r=>{release=r;});}
    }
    if(url.pathname==='/v1/commit')stats.commitHashes.push(hash(body));
    if(url.pathname==='/v1/ack')stats.acks.push(JSON.parse(body.toString()).cursor);
    const headers={...req.headers};delete headers.host;delete headers.connection;delete headers['content-length'];
    const result=await runtime.dispatchFetch(origin+req.url,{method:req.method,headers,body:['GET','HEAD'].includes(req.method)?undefined:body});
    let bytes=Buffer.from(await result.arrayBuffer());
    if(url.pathname==='/v1/enrollment/bootstrap'&&dropBootstrap){dropBootstrap=false;res.destroy();return;}
    if(url.pathname==='/v1/ack'&&dropAck){dropAck=false;res.destroy();return;}
    if(url.pathname==='/v1/commit'&&dropCommit){dropCommit=false;res.destroy();return;}
    if(req.method==='PUT'&&weakUpload&&result.ok){weakUpload=false;const value=JSON.parse(bytes);bytes=Buffer.from(JSON.stringify({blob:value.blob,bytes:value.bytes}));}
    res.writeHead(result.status,{'content-type':result.headers.get('content-type')||'application/octet-stream'});res.end(bytes);
  } catch {if(!res.destroyed)res.writeHead(500).end();}
});
await new Promise(r=>server.listen(0,'localhost',r));
origin=`https://localhost:${server.address().port}`;
process.stdout.write(JSON.stringify({origin,cert:root})+'\n');
const lines=createInterface({input:process.stdin});
lines.on('line',async line=>{if(line==='stop'){release?.();server.closeAllConnections();await new Promise(r=>server.close(r));await runtime.dispose();lines.close();process.exit(0);}});
