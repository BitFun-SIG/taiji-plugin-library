import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser, startSourceServer } from './helpers/browser-account-harness.mjs';
const modulePath = '/@fs' + fileURLToPath(new URL('../../shared/dialog-queue/HostDialogQueue.ts', import.meta.url));

test('real IndexedDB retains an ambiguous submission after closing the browser page', {timeout:60000}, async () => {
 const server=await startSourceServer();const browser=await launchBrowser();
 try {
  const context=await browser.createIncognitoBrowserContext();let page=await context.newPage();
  await page.goto(server.origin);
  const id=await page.evaluate(async path=>{
   const {HostDialogQueue}=await import(path);
   const q=new HostDialogQueue('browser-account/host/session','session',async request=>{
    if(request.action==='submit')throw new Error('Lost acknowledgement');
    return {sessionId:'session',queueEpoch:'host-epoch',revision:0,activeTurnId:'running',items:[],capacity:20,used:0,receipt:null};
   });
   try {await q.submit({content:'offline follow up',agentType:'Standard',attachments:[],metadata:{}});}catch{}
   return q.getSnapshot().pending[0].request.message.turnId;
  },modulePath);
  await page.close();page=await context.newPage();await page.goto(server.origin);
  const result=await page.evaluate(async ({path,id})=>{
   const {HostDialogQueue}=await import(path);const mutations=[];
   const q=new HostDialogQueue('browser-account/host/session','session',async request=>{
    if(request.action==='submit')mutations.push(request);
    return {sessionId:'session',queueEpoch:'host-epoch',revision:2,activeTurnId:null,items:[],capacity:20,used:0,
     receipt:request.action==='get'?{turnId:id,status:'completed',displayContent:'offline follow up'}:null};
   });
   await q.refresh();const pending=q.getSnapshot().pending;
   await q.retry(pending[0]);return {restoredId:pending[0].request.message.turnId,pending:q.getSnapshot().pending.length,mutations:mutations.length};
  },{path:modulePath,id});
  assert.deepEqual(result,{restoredId:id,pending:0,mutations:0});
 } finally {await browser.close();await server.close();}
});

test('mobile running composer keeps send and stop independently available alongside the queue', {timeout:60000}, async () => {
 const server=await startSourceServer();const browser=await launchBrowser();
 try {
  const page=await browser.newPage();await page.setViewport({width:390,height:844,isMobile:true,hasTouch:true});
  await page.goto(server.origin+'/?lang=zh-CN');
  await page.evaluate(async()=>{
   const {mountHostQueueFixture}=await import('/tests/fixtures/host-queue.tsx');
   window.queueFixture=mountHostQueueFixture();
  });
  await page.waitForSelector('.host-message-queue li');
  const actions=await page.$$eval('.chat-page__send-btn', buttons=>buttons.map(button=>({disabled:button.disabled,stop:button.classList.contains('is-stop')})));
  assert.deepEqual(actions,[{disabled:false,stop:true},{disabled:false,stop:false}]);
  await page.click('.chat-page__send-btn:not(.is-stop)');
  assert.ok((await page.evaluate(()=>window.queueFixture.calls)).includes('send'));
  assert.ok(!(await page.evaluate(()=>window.queueFixture.calls)).includes('stop'));
  assert.equal(await page.$eval('body', body=>body.scrollWidth<=window.innerWidth),true);
  await page.screenshot({path:'/tmp/mobile-host-message-queue.png',fullPage:true});
 }finally{await browser.close();await server.close();}
});
