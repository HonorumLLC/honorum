import { test } from 'node:test';
import assert from 'node:assert/strict';
import { meetingsOn, campusNow } from './occurrences.js';
const item = { section: { start: '2026-09-02', end: '2026-09-20' }, meetings: [{days:'MWF',start:540,end:600}] };
test('a meeting must fall within its date interval, including midweek starts',()=>{
 assert.deepEqual(meetingsOn(item,'2026-08-31'),[]);
 assert.equal(meetingsOn(item,'2026-09-02').length,1);
});
test('closures and makeup weekdays preserve explicit user exclusions',()=>{
 const closures={'2026-09-07':{name:'Holiday'},'2026-09-08':{name:'Monday schedule',follows:'M'}};
 assert.equal(meetingsOn(item,'2026-09-07',{},closures).length,0);
 assert.equal(meetingsOn(item,'2026-09-08',{},closures).length,1);
 assert.equal(meetingsOn({...item,removed:['2026-09-08']},'2026-09-08',{},closures).length,0);
});
test('campus clock follows the school timezone across daylight saving',()=>{
 assert.equal(campusNow('America/Chicago',new Date('2026-07-01T14:00:00Z')).getHours(),9);
 assert.equal(campusNow('America/Chicago',new Date('2026-12-01T15:00:00Z')).getHours(),9);
});
test('campus clock switches schools and dates without retaining an old offset', () => {
 const instant = new Date('2026-07-01T02:30:00Z');
 for (const zone of ['America/Chicago', 'Asia/Tokyo', 'America/Chicago']) {
  const date = campusNow(zone, instant);
  assert.equal(date.getDate(), zone === 'Asia/Tokyo' ? 1 : 30);
  assert.equal(date.getHours(), zone === 'Asia/Tokyo' ? 11 : 21);
 }
 assert.equal(campusNow('', instant), instant);
 assert.throws(() => campusNow('invalid/timezone', instant), RangeError);
 assert.equal(campusNow('America/Chicago', instant).getHours(), 21);
});
test('a closure from partway through the day spares earlier classes',()=>{
 const item={id:'a',section:{id:'a'},meetings:[{days:'R',start:540,end:615},{days:'R',start:900,end:975}]};
 const closures={'2026-10-29':{name:'Concert',follows:'',after:900}};
 assert.deepEqual(meetingsOn(item,'2026-10-29',{},closures).map((m)=>m.start),[540]);
 assert.equal(meetingsOn(item,'2026-11-05',{},closures).length,2);
});
