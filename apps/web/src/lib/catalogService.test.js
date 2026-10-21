import { test } from 'node:test';
import assert from 'node:assert/strict';
import { searchPage, createCatalogService } from './catalogService.js';
import { indexCatalog } from './catalog.js';
import { emptyFilters } from './filters.js';
const value = () => ({inst:'demo',term:{id:'fall'},hash:'old',courses:[{code:'CS 1',subject:'CS',title:'Test',sections:[{id:'a',available:1,meetings:[]}]}]});
test('filters include a qualifying course after the former 400-course cutoff',()=>{
 const cat=indexCatalog({courses:Array.from({length:401},(_,i)=>({code:`C ${i}`,subject:'C',title:'Course',sections:[{id:String(i),available:i===400?1:0,meetings:[]}]}))});
 assert.equal(searchPage(cat,'',{...emptyFilters(),open:true}).courses[0].code,'C 400');
});
const seatValue = () => ({inst:'demo',term:'fall',seats:{}});
const reply = url => ({ok:true,json:async()=>url.includes('.seats.')?seatValue():{...value(),hash:'new'}});
test('cached catalogs answer before revalidation finishes',async()=>{
 const messages=[]; const releases=[];
 const handle=createCatalogService({post:m=>messages.push(m),read:async k=>k.startsWith('structure:')?{hash:'old',value:value()}:null,write:async()=>{},fetcher:url=>new Promise(r=>releases.push(()=>r(reply(url))))});
 const loading=handle({type:'load',inst:'demo',term:'fall',structureHash:'new',seatsHash:'seats'});
 await new Promise(r=>setImmediate(r));
 handle({type:'sections',key:'demo/fall',id:1,ids:['a']});
 assert.equal(messages.find(m=>m.id===1).sections[0].id,'a');
 releases.forEach(release=>release());await loading;
});
test('failed loads settle both queued and subsequent queries',async()=>{
 const messages=[];
 const handle=createCatalogService({post:m=>messages.push(m),read:async()=>null,write:async()=>{},fetcher:async()=>({ok:false,status:404})});
 const loading=handle({type:'load',inst:'demo',term:'fall',structureHash:'s',seatsHash:'t'});
 handle({type:'sections',key:'demo/fall',id:1,ids:['a']});
 await loading;
 handle({type:'sections',key:'demo/fall',id:2,ids:['a']});
 assert.ok(messages.find(m=>m.id===1).error);assert.ok(messages.find(m=>m.id===2).error);
});

test('section pages bound large courses and expose every filtered result exactly once', () => {
 const cat=indexCatalog({courses:[{code:'C 1',subject:'C',title:'Course',sections:Array.from({length:103},(_,i)=>({id:String(i),available:i%2,meetings:[]}))},{code:'C 2',subject:'C',title:'Course',sections:[{id:'last',available:1,meetings:[]}]}]});
 for (const filters of [emptyFilters(),{...emptyFilters(),open:true}]) {
  const ids=[];
  for(let offset=0;;offset+=40){
   const page=searchPage(cat,'',filters,offset,40);
   const rows=page.courses.flatMap(c=>c.sections);
   assert.ok(rows.length<=40);ids.push(...rows.map(s=>s.id));
   if(offset+40>=page.total)break;
  }
  const expected=cat.courses.flatMap(c=>c.sections).filter(s=>!filters.open||s.available>0).map(s=>s.id);
  assert.deepEqual(ids,expected);assert.equal(new Set(ids).size,ids.length);
 }
 assert.equal(searchPage(cat,'',emptyFilters(),0,10000).courses.flatMap(c=>c.sections).length,100);
});

test('readiness does not wait for IndexedDB and indexing does not expand the persisted record', async () => {
 const messages=[],raw=value(),saved={};
 const handle=createCatalogService({post:m=>messages.push(m),read:async()=>null,write:async(k,v)=>{saved[k]=v;await new Promise(()=>{});},fetcher:async url=>({ok:true,json:async()=>url.includes('.seats.')?seatValue():raw})});
 await handle({type:'load',inst:'demo',term:'fall',structureHash:'old',seatsHash:'seats'});
 assert.ok(messages.some(m=>m.type==='ready'));
 assert.equal(saved['structure:demo:fall'].value,raw);assert.equal(raw.byId,undefined);assert.equal(raw.courses[0].sections[0].course,undefined);
});

test('immutable structures and seats refresh independently and retain offline seat filters', async () => {
 const stored=new Map(),messages=[],requests=[];
 const structure={...value(),v:3,hash:'structure',courses:[{code:'CS 1',subject:'CS',title:'Test',sections:[{id:'a',meetings:[]},{id:'b',meetings:[]}]}]};
 const seatData={v:1,inst:'demo',term:'fall',seats:{a:[20,2,0],b:[20,0,1]}};
 let offline=false;
 const handle=createCatalogService({post:m=>messages.push(m),read:async k=>structuredClone(stored.get(k)),write:async(k,v)=>stored.set(k,structuredClone(v)),fetcher:async(url,opts)=>{
  requests.push(url);assert.equal(opts.cache,undefined);
  if(offline)throw new Error('offline');
  return{ok:true,json:async()=>structuredClone(url.includes('.structure.')?structure:seatData)};
 }});
 const load={type:'load',inst:'demo',term:'fall',structureHash:'structure',seatsHash:'seats1'};
 await handle(load);
 assert.equal(requests.length,2);
 handle({type:'search',key:'demo/fall',id:1,q:'',f:{...emptyFilters(),open:true},offset:0,limit:40});
 assert.deepEqual(messages.find(m=>m.id===1).courses.flatMap(c=>c.sections).map(s=>s.id),['a']);
 requests.length=0;seatData.seats.b=[20,3,0];
 await handle({...load,seatsHash:'seats2'});
 assert.deepEqual(requests,['/catalog/demo/fall.seats.seats2.json']);
 offline=true;requests.length=0;
 await handle({...load,seatsHash:'seats2'});
 assert.equal(requests.length,0);
 handle({type:'search',key:'demo/fall',id:2,q:'',f:{...emptyFilters(),open:true},offset:0,limit:40});
 assert.equal(messages.find(m=>m.id===2).total,2);
 assert.equal(stored.get('structure:demo:fall').value.courses[0].sections[0].available,undefined);
});

test('structural search becomes ready while seat downloads remain pending', async () => {
 const messages=[];let release;
 const raw={...value(),v:3,hash:'structure'};delete raw.courses[0].sections[0].available;
 const handle=createCatalogService({post:m=>messages.push(m),read:async()=>null,write:async()=>{},fetcher:async url=>url.includes('.seats.')?new Promise(resolve=>release=resolve):({ok:true,json:async()=>raw})});
 const loading=handle({type:'load',inst:'demo',term:'fall',structureHash:'structure',seatsHash:'seats'});
 await new Promise(r=>setImmediate(r));
 assert.ok(messages.some(m=>m.type==='ready'));
 handle({type:'sections',key:'demo/fall',id:3,ids:['a']});
 assert.equal(messages.find(m=>m.id===3).sections[0].available,-1);
 handle({type:'search',key:'demo/fall',id:4,q:'',f:{...emptyFilters(),open:true},offset:0,limit:40});
 assert.equal(messages.find(m=>m.id===4).total,0);
 release({ok:true,json:async()=>({inst:'demo',term:'fall',seats:{a:[10,3,0]}})});await loading;
 handle({type:'sections',key:'demo/fall',id:1,ids:['a']});
 assert.equal(messages.find(m=>m.id===1).sections[0].available,3);
});

test('a term without a readable publication is reported missing without other requests', async () => {
 for (const hashes of [{structureHash:'',seatsHash:''},{structureHash:'expired',seatsHash:'expired'}]) {
  const messages=[],requests=[];
  const handle=createCatalogService({post:m=>messages.push(m),read:async()=>null,write:async()=>{},fetcher:async url=>{requests.push(url);return{ok:false,status:404};}});
  await handle({type:'load',inst:'demo',term:'fall',...hashes});
  assert.equal(messages.filter(m=>m.type==='missing').length,1);
  assert.ok(requests.every(url=>url.includes('.expired.json')));
 }
});

test('an unreadable newer publication keeps the saved catalog usable', async () => {
 const messages=[];
 const handle=createCatalogService({post:m=>messages.push(m),read:async key=>key==='structure:demo:fall'?{hash:'older',value:value()}:null,write:async()=>{},fetcher:async()=>({ok:false,status:404})});
 await handle({type:'load',inst:'demo',term:'fall',structureHash:'expired',seatsHash:'expired'});
 handle({type:'sections',key:'demo/fall',id:1,ids:['a']});
 assert.equal(messages.find(m=>m.id===1).sections[0].id,'a');
 assert.equal(messages.some(m=>m.type==='error'||m.type==='missing'),false);
});

test('a first download fetches seats after the searchable structure, a revalidation fetches both at once', async () => {
 for (const saved of [null, {hash:'old',value:value()}]) {
  const requests=[],releases=[];
  const handle=createCatalogService({post:()=>{},read:async k=>k.startsWith('structure:')?saved:null,write:async()=>{},fetcher:url=>{requests.push(url.includes('.seats.')?'seats':'structure');return new Promise(r=>releases.push(()=>r(reply(url))));}});
  const loading=handle({type:'load',inst:'demo',term:'fall',structureHash:'new',seatsHash:'seats'});
  await new Promise(r=>setImmediate(r));
  assert.deepEqual(requests,saved?['structure','seats']:['structure']);
  releases.shift()();
  await new Promise(r=>setImmediate(r));
  assert.deepEqual(requests,['structure','seats']);
  releases.forEach(release=>release());await loading;
 }
});
