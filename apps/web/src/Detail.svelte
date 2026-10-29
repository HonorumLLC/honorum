<script module>
  import { SvelteMap } from 'svelte/reactivity';
  // Courses and descriptions already looked up this session, so reopening a
  // class renders them at once instead of shifting the pane as they arrive.
  const courses = new SvelteMap(), descs = new SvelteMap();
</script>

<script>
  // The selected section, exam sitting or personal event, in as few words
  // as possible: a way back, its code, when and where, then the seats with
  // their alert bell, its color, and everything else folded into Details;
  // last, skip the day, edit, remove or add. No subtitles.
  import { app, schedule, section, items, toggleSection, removeBlock, watch, unwatch, watchOf, COLORS, EVENT_COLOR, colorOf, setColor, blockColor, ensureCatalog, termInfo, removeDay } from './lib/state.svelte.js';
  import { fmtMeeting, fmtTime } from './lib/schedule.js';
  import { loadDescriptions } from './lib/catalog.js';
  import { getSections, getCourse } from './lib/catalogClient.ts';
  import { toast } from './lib/ui.svelte.js';
  import { google, isGoogle } from './lib/google.svelte.js';
  import { googleId } from './lib/googleEvents.js';
  import Icon from './Icon.svelte';

  let { closable = true } = $props();
  const sec = $derived(section(app.selected));
  const item = $derived(items().find((i) => i.id === app.selected));
  const block = $derived(item?.block);
  // An exam sitting belongs to one of the schedule's sections.
  const exam = $derived(item?.exam);
  const examOf = $derived(exam ? section(item.of) : undefined);
  const examDay = $derived(exam ? new Date(exam.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' }) : '');
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
  let busy = $state(false);
  const key = $derived(sec ? `${app.inst}/${app.term}/${sec.course}` : '');
  const course = $derived(key ? courses.get(key) : null);
  const desc = $derived(key ? descs.get(key) : null);
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
  const places = $derived([...new Set(meetings.map((m) => m.location).filter(Boolean))].join('; '));
  const when = $derived([meetings.map(fmtMeeting).join('; ') || 'No meeting times', places].filter(Boolean).join(' · '));
  const examsText = (list) => list.map((e) => `Exam ${new Date(e.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })} ${fmtTime(e.start)} – ${fmtTime(e.end)}`).join('; ');
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

{#snippet head(color, title)}
  <header>
    {#if closable}<button class="btn flat icon round back" onclick={() => (app.selected = '')} aria-label="Back to Tasks" title="Back to Tasks"><Icon name="back" /></button>{/if}
    <span class="dot" style:background={color}></span>
    <h2 class="title">{title}</h2>
  </header>
{/snippet}

{#if outside}
  {@render head(outsideCalendar?.color || 'var(--track)', outside.title)}
  <p class="when">{[outsideWhen, outside.location].filter(Boolean).join(' · ')}</p>
  {#if outside.link}<div class="actions"><a class="btn pill" href={outside.link} target="_blank" rel="noopener">Open in Google Calendar <Icon name="open" /></a></div>{/if}
{:else if isGoogle(app.selected)}
  <p class="hint">This event is no longer in Google Calendar</p>
{:else if exam}
  {@render head(item.color, item.label)}
  <p class="when">{[examDay, `${fmtTime(exam.start)} – ${fmtTime(exam.end)}`, exam.location].filter(Boolean).join(' · ')}</p>
  <div class="actions">
    {#if examOf}<button class="btn pill" onclick={() => (app.selected = item.of)}>Show Class</button>{/if}
    <button class="btn pill" onclick={hideExam} aria-label="Hide This Exam">Hide</button>
  </div>
{:else if !sec && !block}
  <p class="hint">{settled ? 'Not in this term’s classes' : 'Loading…'}</p>
{:else}
  {@render head(item?.color || colorOf(sec?.course), sec ? `${sec.course}-${sec.num}` : block.name)}
  <p class="when">{when}</p>
  {#if block?.note}<p class="note">{block.note}</p>{/if}

  {#if sec || inSchedule || block}
    <div class="list">
      {#if sec}
        <div class="row seats" class:full={sec.available === 0} title={freshness}>
          <span class="t">{seats}</span>
          <button class="btn flat icon round bell" class:on={!!w} aria-pressed={!!w} disabled={busy} onclick={toggleWatch} aria-label="Alert When a Seat Opens" title="Alert When a Seat Opens"><Icon name="bell" /></button>
        </div>
      {/if}
      {#if inSchedule || block}
        <div class="row colors" role="radiogroup" aria-label="Color">
          {#each swatches as c (c)}<button role="radio" aria-checked={shown === c} class:on={shown === c} style:background={c} onclick={() => paint(c)} aria-label={c === EVENT_COLOR ? 'No color' : 'Color'}></button>{/each}
        </div>
      {/if}
      {#if sec}
        <details class="more">
          <summary class="row"><span class="t">Details</span><span class="chev"><Icon name="next" /></span></summary>
          <div class="facts">
            <p>{sec.title}</p>
            <p>{sec.instructors.join(', ') || 'Staff'}</p>
            {#if sec.waitlist}<p>{sec.waitlist} waitlisted</p>{/if}
            {#if sec.exams?.length}<p>{examsText(sec.exams)}</p>{/if}
            {#if sec.credits}<p>{sec.credits} credits{sec.mode ? ` · ${sec.mode}` : ''}</p>{/if}
            {#if sec.start && sec.start !== termInfo()?.start}<p>{sec.start} to {sec.end}</p>{/if}
            <p>{sec.id}{sec.link ? ` · linked group ${sec.link}` : ''}{#if sec.url}{' · '}<a href={sec.url} target="_blank" rel="noopener">source</a>{/if}</p>
            {#if sec.attributes?.length || course?.offered?.length > 1}
              <div class="chips">
                {#each sec.attributes || [] as a (a)}<span>{a}</span>{/each}
                {#if course?.offered?.length > 1}{#each course.offered as t (t)}<span class="term">{t}</span>{/each}{/if}
              </div>
            {/if}
            {#if description}<p class="desc">{description}</p>{/if}
            {#if prereqs}<p class="desc pre">{prereqs}</p>{/if}
          </div>
        </details>
      {/if}
    </div>
  {/if}

  <div class="actions">
    {#if occurrence}<button class="btn pill" onclick={removeThisDay}>Skip {occurrence.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}</button>{/if}
    {#if block}<button class="btn pill" onclick={() => (app.editing = block.id)}>Edit</button>{/if}
    {#if inSchedule || block}
      <button class="btn pill remove" onclick={remove} aria-label="Remove From Schedule">Remove</button>
    {:else}
      <button class="btn pill suggested" onclick={add}>Add to Schedule</button>
    {/if}
  </div>
{/if}

<style>
  header { display: flex; align-items: center; gap: 8px; min-height: 34px; }
  .back { margin-left: -9px; }
  .dot { flex: none; width: 12px; height: 12px; border-radius: 50%; }
  header h2 { min-width: 0; font-size: 1.2rem; line-height: 1.25; white-space: normal; overflow-wrap: anywhere; }
  .when { margin-top: 8px; color: var(--text2); }
  .note { margin-top: 6px; color: var(--text2); white-space: pre-line; }
  .list { margin-top: 14px; }
  .list .row { min-height: 46px; padding: 0 6px 0 12px; }
  .seats .t { flex: 1; min-width: 0; }
  .seats.full .t { color: var(--red); font-weight: 600; }
  .bell { color: var(--muted); }
  .bell.on { color: var(--accent-fg); }
  .bell.on :global(.i) { fill: currentColor; }
  .colors { gap: 8px; }
  /* The chosen color wears a ring clear of it, which shows on the palest swatch. */
  .colors button { flex: none; width: 22px; height: 22px; padding: 0; border: 0; border-radius: 50%; }
  .colors button.on { outline: 2px solid var(--text); outline-offset: 2px; }
  .more summary { list-style: none; cursor: pointer; }
  .more summary::-webkit-details-marker { display: none; }
  .more summary .t { flex: 1; }
  .more .chev { padding: 0 8px; }
  .more .chev :global(.i) { transition: rotate var(--quick-open) var(--ease-open); }
  .more[open] .chev :global(.i) { rotate: 90deg; }
  .facts { display: flex; flex-direction: column; gap: 4px; padding: 0 12px 12px; color: var(--text2); font-size: .9rem; }
  .chips { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .chips span { padding: 2px 8px; border-radius: 6px; background: var(--hover); color: var(--text2); font-size: .78rem; }
  .chips .term { background: var(--tint); color: var(--accent-fg); }
  .desc { margin-top: 4px; white-space: pre-line; }
  .desc.pre { font-size: .82rem; }
  /* Details slides open and shut where the browser can ease to its height. */
  @supports (interpolate-size: allow-keywords) {
    @media (prefers-reduced-motion: no-preference) {
      .more { interpolate-size: allow-keywords; }
      .more::details-content { block-size: 0; opacity: 0; overflow: clip; transition: block-size var(--close) var(--ease-close), opacity var(--close) var(--ease-close), content-visibility var(--close) allow-discrete; }
      .more[open]::details-content { block-size: auto; opacity: 1; transition-duration: var(--open); transition-timing-function: var(--ease-open); }
    }
  }
  .actions { display: flex; gap: 8px; margin-top: 14px; }
  .actions .btn { flex: 1; min-width: 0; }
  .remove { color: var(--red); }
</style>
