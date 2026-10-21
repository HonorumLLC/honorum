<script>
  // A personal event: once on a date, or weekly on chosen days, belonging
  // to one of the schedule's classes (and colored like it) or given a color
  // of its own, with notes. Opens when app.editing holds an event id or
  // 'new'. date: the calendar's picked day, the default for a new event.
  import { untrack } from 'svelte';
  import { app, schedule, taskCourses, colorOf, COLORS, EVENT_COLOR } from './lib/state.svelte.js';
  import { DAYS, DAY_NAMES, rid } from './lib/schedule.js';
  import { ymd } from './lib/dates.js';
  import Dialog from './Dialog.svelte';

  let { date = '' } = $props();
  let open = $state(false);
  const blank = { name: '', repeat: false, date: '', days: {}, start: '12:00', end: '13:00', location: '', course: '', color: '', note: '' };
  let ev = $state({ ...blank });
  const courses = $derived(taskCourses());
  // The swatch row: the neutral event color first, then the class palette.
  const swatches = [EVENT_COLOR, ...COLORS];
  const shown = $derived(ev.course ? colorOf(ev.course) : ev.color || EVENT_COLOR);
  const hhmm = (m) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  const mins = (t) => (t || '0:0').split(':').reduce((h, m) => h * 60 + +m, 0);
  const valid = $derived((ev.repeat ? Object.values(ev.days).some(Boolean) : !!ev.date) && mins(ev.end) > mins(ev.start));
  const isNew = $derived(app.editing === 'new');

  $effect(() => {
    if (!app.editing) return;
    const id = app.editing;
    // Returning to the tab restores the schedule from storage, and sync may
    // replace it too; neither should wipe the form while someone is typing.
    const b = untrack(() => schedule()?.blocks.find((x) => x.id === id));
    const day = untrack(() => date) || ymd(new Date());
    const next = b
      ? { name: b.name, repeat: !b.date, date: b.date || '', days: Object.fromEntries([...b.days].map((d) => [d, true])), start: hhmm(b.start), end: hhmm(b.end), location: b.location || '', course: b.course || '', color: b.color || '', note: b.note || '' }
      : { ...blank, date: day, days: { [DAYS[(new Date(day + 'T00:00:00').getDay() + 6) % 7]]: true } };
    length = mins(next.end) - mins(next.start); // from the plain object: reading ev here would re-run this effect on every edit
    ev = next;
    open = true;
  });
  // Moving the start keeps the event's length, so the end follows; the end
  // set by hand becomes the new length.
  let length = 60;
  function setStart(v) {
    ev.start = v;
    const end = mins(v) + length;
    ev.end = end < 24 * 60 ? hhmm(end) : '23:59';
  }
  function setEnd(v) {
    ev.end = v;
    if (mins(v) > mins(ev.start)) length = mins(v) - mins(ev.start);
  }
  function save() {
    const days = ev.repeat ? [...DAYS].filter((d) => ev.days[d]).join('') : DAYS[(new Date(ev.date + 'T00:00:00').getDay() + 6) % 7];
    // A class decides the color, so a label picked earlier is not kept beside it.
    const data = { name: ev.name.trim() || 'Busy', days, date: ev.repeat ? '' : ev.date, start: mins(ev.start), end: mins(ev.end), location: ev.location.trim(), course: ev.course, color: ev.course ? '' : ev.color, note: ev.note.trim() };
    const s = schedule();
    const existing = s.blocks.find((b) => b.id === app.editing);
    existing ? Object.assign(existing, data) : s.blocks.push({ id: rid(), ...data });
    open = false;
  }
</script>

<Dialog bind:open title={isNew ? 'Add Event' : 'Edit Event'} ok={isNew ? 'Add' : 'Save'} okDisabled={!valid} onok={save} onclose={() => (app.editing = '')}>
  <!-- svelte-ignore a11y_autofocus -->
  <label class="field"><span>Name</span><input bind:value={ev.name} placeholder="Work, lunch, club meeting…" autofocus></label>
  <div class="field">
    <span>Repeats</span>
    <div class="linked fill" role="radiogroup" aria-label="Repeats">
      <button type="button" class="btn" role="radio" aria-checked={!ev.repeat} class:on={!ev.repeat} onclick={() => (ev.repeat = false)}>Once</button>
      <button type="button" class="btn" role="radio" aria-checked={ev.repeat} class:on={ev.repeat} onclick={() => (ev.repeat = true)}>Every Week</button>
    </div>
  </div>
  {#if ev.repeat}
    <div class="field">
      <span>Days</span>
      <div class="linked fill" role="group" aria-label="Days">
        {#each DAYS as d, i (d)}<button type="button" class="btn" class:on={!!ev.days[d]} aria-pressed={!!ev.days[d]} onclick={() => (ev.days[d] = !ev.days[d])}>{DAY_NAMES[i]}</button>{/each}
      </div>
    </div>
  {:else}
    <label class="field"><span>Date</span><input type="date" bind:value={ev.date} required></label>
  {/if}
  <div class="fields">
    <label class="field"><span>Starts</span><input type="time" value={ev.start} oninput={(e) => setStart(e.currentTarget.value)} required></label>
    <label class="field"><span>Ends</span><input type="time" value={ev.end} oninput={(e) => setEnd(e.currentTarget.value)} required></label>
  </div>
  <label class="field"><span>Location</span><input bind:value={ev.location} placeholder="Optional"></label>
  <label class="field"><span>Class</span>
    <select bind:value={ev.course}>
      <option value="">No class</option>
      {#each courses as c (c)}<option value={c}>{c}</option>{/each}
      {#if ev.course && !courses.includes(ev.course)}<option value={ev.course}>{ev.course}</option>{/if}
    </select>
  </label>
  <div class="field">
    <span>Color</span>
    {#if ev.course}
      <p class="same"><span class="swatch" style:background={shown}></span>Same as {ev.course}</p>
    {:else}
      <div class="colors" role="radiogroup" aria-label="Color">
        {#each swatches as c (c)}<button type="button" role="radio" aria-checked={shown === c} class:on={shown === c} style:background={c} onclick={() => (ev.color = c === EVENT_COLOR ? '' : c)} aria-label={c === EVENT_COLOR ? 'No color' : 'Color'}></button>{/each}
      </div>
    {/if}
  </div>
  <label class="field"><span>Notes</span><textarea bind:value={ev.note} rows="3" placeholder="Optional"></textarea></label>
</Dialog>

<style>
  .colors { display: flex; gap: 6px; padding: 4px 0; }
  .colors button { width: 24px; height: 24px; border: 2px solid transparent; border-radius: 50%; }
  .colors button.on { border-color: var(--text); }
  .same { display: flex; align-items: center; gap: 8px; min-height: 36px; color: var(--text2); }
  .swatch { flex: none; width: 20px; height: 20px; border-radius: 6px; }
  textarea { resize: vertical; }
</style>
