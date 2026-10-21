<script module>
  import { SvelteMap } from 'svelte/reactivity';
  // Courses and descriptions already looked up this session, so reopening a
  // class renders them at once instead of shifting the pane as they arrive.
  const courses = new SvelteMap(), descs = new SvelteMap();
</script>

<script>
  // The selected section, exam sitting or personal event: what, when,
  // where, who, seats, the seat alert switch, its color, notes, and add or
  // remove.
  import { app, schedule, section, items, toggleSection, removeBlock, watch, unwatch, watchOf, COLORS, EVENT_COLOR, colorOf, setColor, blockColor, ensureCatalog, termInfo, removeDay } from './lib/state.svelte.js';
  import { fmtMeeting, fmtTime } from './lib/schedule.js';
  import { loadDescriptions } from './lib/catalog.js';
  import { getSections, getCourse } from './lib/catalogClient.ts';
  import { toast } from './lib/ui.svelte.js';
  import { google, isGoogle } from './lib/google.svelte.js';
  import { googleId } from './lib/googleEvents.js';
  import Icon from './Icon.svelte';
  import { slide } from 'svelte/transition';

  let { closable = true } = $props();
  const sec = $derived(section(app.selected));
  const item = $derived(items().find((i) => i.id === app.selected));
  const block = $derived(item?.block);
  // An exam sitting belongs to one of the schedule's sections.
  const exam = $derived(item?.exam);
  const examOf = $derived(exam ? section(item.of) : undefined);
  const examDay = $derived(exam ? new Date(exam.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' }) : '');
  const inSchedule = $derived(schedule()?.sections.includes(app.selected));
  // A Google Calendar event: shown as it is, and changed in Google Calendar.
  const outside = $derived(isGoogle(app.selected) ? google.events.find((e) => googleId(e) === app.selected) : null);
  const outsideCalendar = $derived(outside ? google.calendars.find((c) => c.id === outside.calendar) : null);
  const outsideWhen = $derived.by(() => {
    if (!outside) return '';
    const from = new Date(outside.start), to = new Date(outside.end);
    const zone = termInfo()?.timezone || undefined;
    const day = (d) => d.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', timeZone: zone });
    const time = (d) => d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit', timeZone: zone });
    return day(from) === day(to) ? `${day(from)}, ${time(from)} – ${time(to)}` : `${day(from)}, ${time(from)} – ${day(to)}, ${time(to)}`;
  });
  const w = $derived(watchOf(app.selected));
  const meetings = $derived(sec ? sec.meetings : block ? [block] : []);
  const settled = $derived(['ready', 'missing', 'error'].includes(app.cat.status));
  // The day that was clicked can be removed on its own, leaving the other weeks.
  const occurrence = $derived(app.selectedDate && (inSchedule || block) && !block?.date ? new Date(app.selectedDate + 'T00:00:00') : null);
  // The swatches color the event's class, or the event itself when it has none.
  const swatches = $derived(block && !block.course ? [EVENT_COLOR, ...COLORS] : COLORS);
  const shown = $derived(block ? blockColor(block) : colorOf(sec?.course));
  function paint(c) {
    if (block && !block.course) block.color = c === EVENT_COLOR ? '' : c;
    else setColor(block ? block.course : sec.course, c);
  }
  function hideExam() {
    const label = item.label;
    const undo = removeDay(item.id, exam.date);
    app.selected = '';
    toast(`${label} hidden`, { label: 'Undo', run: undo });
  }
  function removeThisDay() {
    const label = sec ? `${sec.course}-${sec.num}` : block.name;
    const when = occurrence.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' });
    const undo = removeDay(app.selected, app.selectedDate);
    app.selected = '';
    toast(`${label} removed from ${when}`, { label: 'Undo', run: undo });
  }

  // The full section, its other terms and its description come from the
  // catalog when it is available; the compact copy renders meanwhile.
  // The first lookup slides its rows in rather than snapping them into place.
  let busy = $state(false);
  const key = $derived(sec ? `${app.inst}/${app.term}/${sec.course}` : '');
  const course = $derived(key ? courses.get(key) : null);
  const desc = $derived(key ? descs.get(key) : null);
  const reveal = (node) => slide(node, { duration: matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 200 });
  $effect(() => {
    const id = app.selected;
    if (!id || isGoogle(id)) return;
    ensureCatalog();
    if (!app.cache[id]) getSections([id]).then(([s]) => { if (s && app.selected === id) app.cache[id] = s; }).catch(() => {});
    const code = section(id)?.course;
    if (!code) return;
    const key = `${app.inst}/${app.term}/${code}`;
    if (!courses.has(key)) getCourse(code).then((c) => courses.set(key, c)).catch(() => {});
    if (!descs.has(key)) loadDescriptions(app.inst, app.term, termInfo()?.descriptions_snapshot).then((d) => descs.set(key, d?.[code] || null));
  });
  const description = $derived(desc?.d ?? course?.description ?? '');
  const prereqs = $derived(desc?.p ?? course?.prereqs ?? '');
  const freshness = $derived(sec?.fetched_at ? `Last checked ${new Date(sec.fetched_at * 1000).toLocaleString()}` : 'Saved seat count; last check unknown');
  const seats = $derived(!sec ? '' : sec.available < 0 ? 'Seat count not published' : sec.capacity ? `${sec.available} of ${sec.capacity} seats open` : sec.available > 0 ? 'Seats open' : 'Full');

  async function toggleWatch() {
    busy = true;
    try {
      if (w) await unwatch(w);
      else {
        if (!app.account) {
          if (!app.config?.vapid || !('PushManager' in window)) throw new Error('Push notifications are unavailable in this browser');
          if ((await Notification.requestPermission()) !== 'granted') throw new Error('Notifications are blocked for this site');
        }
        await watch(sec);
      }
    } catch (e) {
      toast(e.message);
    }
    busy = false;
  }
  function remove() {
    block ? removeBlock(block.id) : toggleSection(app.selected);
    app.selected = '';
  }
  function add() {
    toggleSection(sec);
  }
</script>

{#if outside}
  <header>
    <span class="swatch" style:background={outsideCalendar?.color || 'var(--track)'}></span>
    <div class="rt">
      <h2 class="title">{outside.title}</h2>
      {#if outsideCalendar?.name}<p class="sub">{outsideCalendar.name}</p>{/if}
    </div>
    {#if closable}<button class="btn flat icon round" onclick={() => (app.selected = '')} aria-label="Close" title="Close"><Icon name="close" /></button>{/if}
  </header>
  <ul class="facts">
    <li><Icon name="clock" />{outsideWhen}</li>
    {#if outside.location}<li><Icon name="pin" />{outside.location}</li>{/if}
  </ul>
  {#if outside.link}<div class="actions"><a class="btn pill" href={outside.link} target="_blank" rel="noopener">Open in Google Calendar <Icon name="open" /></a></div>{/if}
{:else if isGoogle(app.selected)}
  <p class="hint">This event is no longer in Google Calendar</p>
{:else if exam}
  <header>
    <span class="swatch" style:background={item.color}></span>
    <div class="rt">
      <h2 class="title">{item.label}</h2>
      {#if examOf}<p class="sub">{examOf.title} · {examOf.course}-{examOf.num}</p>{/if}
    </div>
    {#if closable}<button class="btn flat icon round" onclick={() => (app.selected = '')} aria-label="Close" title="Close"><Icon name="close" /></button>{/if}
  </header>
  <ul class="facts">
    <li><Icon name="dates" />{examDay}</li>
    <li><Icon name="clock" />{fmtTime(exam.start)} – {fmtTime(exam.end)}</li>
    {#if exam.location}<li><Icon name="pin" />{exam.location}</li>{/if}
    <li class="dim">Published with the class by the school</li>
  </ul>
  <div class="actions">
    {#if examOf}<button class="btn pill" onclick={() => (app.selected = item.of)}>Show Class</button>{/if}
    <button class="btn pill" onclick={hideExam}>Hide This Exam</button>
  </div>
{:else if !sec && !block}
  <p class="hint">{settled ? 'Not in this term’s classes' : 'Loading…'}</p>
{:else}
  <header>
    <span class="swatch" style:background={item?.color || colorOf(sec?.course)}></span>
    <div class="rt">
      <h2 class="title">{sec ? `${sec.course}-${sec.num}` : block.name}</h2>
      {#if sec}<p class="sub">{sec.title}</p>{:else if block.course}<p class="sub">{block.course}</p>{/if}
    </div>
    {#if closable}<button class="btn flat icon round" onclick={() => (app.selected = '')} aria-label="Close" title="Close"><Icon name="close" /></button>{/if}
  </header>

  <ul class="facts">
    <li><Icon name="clock" />{meetings.map(fmtMeeting).join('; ') || 'No meeting times'}</li>
    {#if meetings.some((m) => m.location)}<li><Icon name="pin" />{[...new Set(meetings.map((m) => m.location).filter(Boolean))].join('; ')}</li>{/if}
    {#if sec}
      <li><Icon name="person" />{sec.instructors.join(', ') || 'Staff'}</li>
      <li class:full={sec.available === 0}><Icon name="seats" />{seats}{sec.waitlist ? `, ${sec.waitlist} waitlisted` : ''}</li>
      <li class="dim">{freshness}</li>
      {#if sec.credits}<li><Icon name="credits" />{sec.credits} credits{sec.mode ? ` · ${sec.mode}` : ''}</li>{/if}
      {#if sec.start && sec.start !== termInfo()?.start}<li><Icon name="dates" />{sec.start} to {sec.end}</li>{/if}
      {#if sec.exams?.length}<li><Icon name="dates" /><span>{sec.exams.map((e) => `Exam ${new Date(e.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })} ${fmtTime(e.start)} – ${fmtTime(e.end)}`).join('; ')}</span></li>{/if}
      <li><Icon name="hash" /><span>{sec.id}{#if sec.link}<span class="dim linked">· linked group {sec.link}</span>{/if}{#if sec.url} · <a href={sec.url} target="_blank" rel="noopener">source</a>{/if}</span></li>
    {/if}
  </ul>

  {#if sec}
    <label class="list watchrow">
      <span class="row">
        <Icon name="bell" />
        <span class="rt"><span class="t">Alert When a Seat Opens</span></span>
        <input type="checkbox" class="switch" checked={!!w} disabled={busy} onchange={toggleWatch}>
      </span>
    </label>
    {#if sec.attributes?.length || course?.offered?.length > 1}
      <div class="chips" in:reveal>
        {#each sec.attributes || [] as a (a)}<span>{a}</span>{/each}
        {#if course?.offered?.length > 1}{#each course.offered as t (t)}<span class="term">{t}</span>{/each}{/if}
      </div>
    {/if}
    {#if description || prereqs}
      <details class="desc" in:reveal>
        <summary>{description ? 'Description' : 'Prerequisites'}</summary>
        {#if description}<p>{description}</p>{/if}
        {#if prereqs}<p class="pre">{prereqs}</p>{/if}
      </details>
    {/if}
  {:else if block.note}
    <p class="note">{block.note}</p>
  {/if}

  {#if inSchedule || block}
    <div class="colors" role="radiogroup" aria-label="Color">
      {#each swatches as c (c)}<button role="radio" aria-checked={shown === c} class:on={shown === c} style:background={c} onclick={() => paint(c)} aria-label={c === EVENT_COLOR ? 'No color' : 'Color'}></button>{/each}
    </div>
  {/if}

  {#if occurrence}
    <div class="list occ">
      <div class="row">
        <Icon name="dates" />
        <span class="rt"><span class="t">{occurrence.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' })}</span></span>
        <button class="btn flat" onclick={removeThisDay}>Remove This Day</button>
      </div>
    </div>
  {/if}

  <div class="actions">
    {#if block}<button class="btn pill" onclick={() => (app.editing = block.id)}>Edit</button>{/if}
    {#if inSchedule || block}
      <button class="btn pill" onclick={remove}>Remove From Schedule</button>
    {:else}
      <button class="btn pill suggested" onclick={add}>Add to Schedule</button>
    {/if}
  </div>
{/if}

<style>
  header { display: flex; align-items: flex-start; gap: 12px; }
  .swatch { flex: none; width: 36px; height: 36px; margin-top: 2px; border-radius: 9px; }
  header h2 { font-size: 1.25rem; white-space: normal; }
  .sub { margin-top: 1px; color: var(--text2); }
  .facts { margin: 16px 0 14px; }
  .facts li { display: flex; align-items: flex-start; gap: 12px; padding: 5px 0; }
  .facts :global(.i) { flex: none; margin-top: 3px; color: var(--muted); }
  .facts .full { color: var(--red); font-weight: 600; }
  .linked { margin-left: 4px; }
  .facts .full :global(.i) { color: inherit; }
  .watchrow { display: block; cursor: pointer; }
  .watchrow :global(.i) { color: var(--muted); }
  .chips { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 12px; }
  .chips span { padding: 2px 8px; border-radius: 6px; background: var(--hover); color: var(--text2); font-size: .78rem; }
  .chips .term { background: var(--tint); color: var(--accent-fg); }
  .desc { margin-top: 12px; }
  .desc summary { cursor: pointer; color: var(--accent-fg); font-weight: 500; }
  .desc p { margin-top: 6px; color: var(--text2); font-size: .9rem; white-space: pre-line; }
  .desc .pre { font-size: .82rem; }
  /* The description slides open and shut where the browser can ease to its height. */
  @supports (interpolate-size: allow-keywords) {
    @media (prefers-reduced-motion: no-preference) {
      .desc { interpolate-size: allow-keywords; }
      .desc::details-content { block-size: 0; opacity: 0; overflow: clip; transition: block-size var(--close) var(--ease-close), opacity var(--close) var(--ease-close), content-visibility var(--close) allow-discrete; }
      .desc[open]::details-content { block-size: auto; opacity: 1; transition-duration: var(--open); transition-timing-function: var(--ease-open); }
    }
  }
  .note { margin-top: 12px; color: var(--text2); white-space: pre-line; }
  .colors { display: flex; gap: 6px; margin-top: 14px; }
  .colors button { width: 24px; height: 24px; border: 2px solid transparent; border-radius: 50%; }
  .colors button.on { border-color: var(--text); }
  .occ { margin-top: 14px; }
  .occ :global(.i) { color: var(--muted); }
  .actions { display: flex; justify-content: center; gap: 8px; margin-top: 22px; }
</style>
