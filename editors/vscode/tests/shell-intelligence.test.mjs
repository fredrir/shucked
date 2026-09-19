import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';
import { createRequire } from 'node:module';
import { runInNewContext } from 'node:vm';
import { fileURLToPath } from 'node:url';
import { mkdtemp, chmod, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:net';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
const require = createRequire(import.meta.url);
async function moduleFrom(filename) {
  const built = await build({entryPoints:[fileURLToPath(new URL(`../src/${filename}`, import.meta.url))],bundle:true,platform:'node',format:'cjs',external:['vscode'],write:false});
  const module = {exports:{}};
  runInNewContext(built.outputFiles[0].text, {module,exports:module.exports,process,Buffer,require:id=>id==='vscode'?{}:require(id)});
  return module.exports;
}
const {HistoryIndex,parseHistory,acceptedSessionCommand} = await moduleFrom('history.ts');
const {validateSessionMessage} = await moduleFrom('terminal.ts');
test('history index isolates targets and never proposes private, multiline, or oversized entries',()=>{
  const index = new HistoryIndex();
  for (const command of ['brew install fish',' secret token','brew\nsecret','b'.repeat(9000)]) {index.record('hostA',command);}
  assert.equal(index.suggest('hostA','brew'), 'brew install fish');
  assert.equal(index.suggest('hostB','brew'),undefined);
  assert.equal(index.suggest('hostA',' secret'),undefined);
  index.clear('hostA');assert.equal(index.suggest('hostA','brew'),undefined);
});
test('shell history formats are decoded as data and multiline commands omitted',()=>{
  assert.deepEqual(Array.from(parseHistory(': 123:0;brew install fish\n: 124:0;echo a\\\nsecret','zsh')),['brew install fish']);
  assert.deepEqual(Array.from(parseHistory('#1234\ngit status\n','bash')),['git status']);
  assert.deepEqual(Array.from(parseHistory('- cmd: brew install fish\n  when: 42\n- cmd: echo\\nsecret\n','fish')),['brew install fish']);
});
test('session history requires acceptance at a fresh nonprivate prompt',()=>{
  const text='brew install fish'; const metadata={private:false,ignore:[],acceptedHistoryHash:createHash('sha256').update(text).digest('hex')};
  assert.equal(acceptedSessionCommand(text,metadata),true);
  assert.equal(acceptedSessionCommand(text,{...metadata,private:true}),false);
  assert.equal(acceptedSessionCommand('brew install SECRET',metadata),false);
  assert.equal(acceptedSessionCommand(text,{...metadata,acceptedHistoryHash:undefined}),false);
  assert.equal(acceptedSessionCommand(text,{...metadata,ignore:['unsupported-history-filter']}),false);
});
test('shell metadata rejects malformed and oversized identities',()=>{
  const valid={id:'a'.repeat(32),token:'b'.repeat(64),generation:1,pid:123,shell:'zsh',cwd:process.cwd(),path:['','/usr/bin'],aliases:{ls:['eza','--icons']},functions:['greet'],options:{aliases:'on'},private:false,ignore:[],connected:true};
  assert.equal(validateSessionMessage(valid),true);
  for(const patch of [{generation:-1},{generation:Infinity},{path:[null]},{aliases:{ls:'eza'}},{cwd:'relative'},{token:'no'},{functions:Array(16385).fill('f')}]) {assert.equal(validateSessionMessage({...valid,...patch}),false);}
});
for (const shell of ['bash','zsh','fish']) {
 for (const filesEnabled of [false, true]) {
  test(`${shell} prompt metadata discovers custom history only when opted in (${filesEnabled})`,async t=>{
    if(process.platform==='win32'){t.skip('POSIX hook fixture');return;}
    const directory=await mkdtemp(join(tmpdir(),'shucked-hook-'));await chmod(directory,0o700);
    const policy=join(directory,'policy'); await writeFile(policy, `0\n${filesEnabled?1:0}\n`);
    const historyFile=join(directory, 'custom-history');
    const socket=join(directory,'state.sock');
    const server=createServer();
    try{
      await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(socket,resolve);});
      const received=new Promise((resolve,reject)=>{
        const timer=setTimeout(()=>reject(new Error('no snapshot')),3000);
        server.once('connection',connection=>{let message='';connection.on('data',chunk=>message+=chunk);connection.on('end',()=>{clearTimeout(timer);resolve(JSON.parse(message));});});
      });
      const suffix=shell==='zsh'?'zsh.zsh':shell==='fish'?'fish.fish':'bash.sh';const hook=fileURLToPath(new URL(`../shell-integration/${suffix}`,import.meta.url));
      const script=shell==='fish'?`source "$argv[1]"; alias ls 'eza --icons'; alias dangerous 'touch should-never-be-executed'; function demo; echo PRIVATE_FUNCTION_BODY; end; __shucked_capture`:`source "$1"; alias ls='eza --icons'; alias dangerous='touch should-never-be-executed'; function demo { echo PRIVATE_FUNCTION_BODY; }; __shucked_capture`;
      const arguments_=shell==='fish'?['--no-config','-c',script,hook]:['-c',script,shell,hook];
      const child=spawn(shell,arguments_,{env:{...process.env,HISTFILE:historyFile,fish_history:'custom',XDG_DATA_HOME:directory,SHUCKED_HISTORY_POLICY:policy,SHUCKED_SESSION_ID:'a'.repeat(32),SHUCKED_SESSION_TOKEN:'b'.repeat(64),SHUCKED_SESSION_SOCKET:socket,SHUCKED_CAPTURE:fileURLToPath(new URL('../shell-integration/capture.cjs',import.meta.url)),SHUCKED_NODE:process.execPath},stdio:['ignore','pipe','pipe']});
      const exited=new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',code=>code===0?resolve():reject(new Error(`shell exit ${code}`)));});
      const [message]=await Promise.all([received,exited]);
      assert.equal(message.shell,shell);if(shell==='fish'){assert.ok(message.functions.includes('ls'));}else{assert.deepEqual(message.aliases.ls,['eza','--icons']);}assert.equal(message.aliases.dangerous,undefined);assert.ok(message.functions.includes('demo'));assert.ok(message.functions.includes('dangerous'));assert.ok(!JSON.stringify(message).includes('PRIVATE_FUNCTION_BODY'));assert.equal(typeof message.private,'boolean');assert.equal(message.acceptedHistoryHash,undefined);assert.equal(message.historyFile,filesEnabled?(shell==='fish'?join(directory,'fish/custom_history'):historyFile):undefined);
    }finally{await new Promise(resolve=>server.close(resolve));await rm(directory,{recursive:true,force:true});}
  });
}

}
