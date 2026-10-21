<script>
  // The calendar grid. mode 'week': seven day headers (buttons that pick the
  // day the side pane shows), a row of the week's due tasks like all-day
  // events (or, with taskTimes, the tasks on the hours ending at their due
  // time), then the hours. mode 'day' (phones): a seven-day strip and one column.
  import { meetingsOn, campusNow } from './lib/occurrences.js';
  import { onMount, untrack } from 'svelte';
  import { DAYS, DAY_NAMES, fmtTime, fmtHour } from './lib/schedule.js';
  import { layout, columns } from './lib/layout.js';
  import { addDays, activeInWeek, ymd } from './lib/dates.js';
  import Icon from './Icon.svelte';

  // start is the first day shown; day is the picked day as an offset from it.
  // closures: date → { name, follows }: days the school cancels classes, or runs another weekday's schedule.
  let { items = [], tasks = [], conflicts = new Set(), selected = '', mode = 'week', day = 0, start, weekStart = 'mon', term = {}, closures = {}, taskTimes = false, onday, onselect, ontask } = $props();
  const visible = $derived(items.filter((it) => activeInWeek(it, start, term)));
  const lay = $derived(layout(visible));
  // Columns are DAYS indices (0 = Monday) in display order; offsetOf gives days from start.
  const order = $derived(weekStart === 'sun' ? [6, 0, 1, 2, 3, 4, 5] : [0, 1, 2, 3, 4, 5, 6]);
  const offsetOf = (i) => (weekStart === 'sun' ? (i + 1) % 7 : i);
  // Due times, like meetings, are read on the campus clock.
  const dueAt = (t) => campusNow(term?.timezone, new Date(t.due));
  const dayIdx = (t) => { const d = dueAt(t); return Math.round((new Date(d.getFullYear(), d.getMonth(), d.getDate()) - start) / 86400e3); };
  const weekTasks = $derived(tasks.map((t) => ({ ...t, day: dayIdx(t) })).filter((t) => t.day >= 0 && t.day < 7).sort((a, b) => a.due.localeCompare(b.due)));
  // Weekend columns appear when something falls on them: a class or event
  // meeting on Saturday or Sunday, or a task due this weekend.
  const weekend = $derived(items.some((it) => it.meetings.some((m) => /[SU]/.test(m.days))) || weekTasks.some((t) => offsetOf(5) === t.day || offsetOf(6) === t.day));
  const cols = $derived(weekend ? order : order.filter((i) => i < 5));
  const dateOf = (i) => addDays(start, offsetOf(i));
  const dueOn = (i) => weekTasks.filter((t) => t.day === offsetOf(i));
  // The due row appears when a task is due this week and tasks are not on the hours.
  const anyDue = $derived(!taskTimes && cols.some((i) => dueOn(i).length));
  // On the hours a task is a deadline: a half-hour block ending when it is due.
  const TASK_MIN = 30;
  const dueMinute = (t) => { const d = dueAt(t); return d.getHours() * 60 + d.getMinutes(); };
  const taskSlot = (t) => { const end = Math.max(TASK_MIN, dueMinute(t)); return { start: end - TASK_MIN, end }; };
  const SHOW = 3;
  let showAll = $state(false); // whether every column's due row is unfolded

  // All 24 hours are drawn; the view opens scrolled to the first class (8am at the latest).
  const ROW = 56; // px per hour
  const HOURS = 24;
  let gridEl = $state(null), stickyEl = $state(null);
  function scrollParent(el) {
    for (let p = el.parentElement; p; p = p.parentElement) {
      const o = getComputedStyle(p).overflowY;
      if (o === 'auto' || o === 'scroll') return p;
    }
    return null;
  }
  function scrollToHour(h) {
    if (!gridEl) return;
    const p = scrollParent(gridEl);
    const head = stickyEl?.offsetHeight || 0;
    if (p) p.scrollTop = gridEl.getBoundingClientRect().top - p.getBoundingClientRect().top + p.scrollTop - head + h * ROW;
    else window.scrollTo(0, gridEl.getBoundingClientRect().top + window.scrollY - head + h * ROW);
  }
  $effect(() => {
    mode; // a new grid on mount and when switching between day and week
    if (gridEl) untrack(() => scrollToHour(lay.startH));
  });
  let instant = $state(new Date());
  const now = $derived(campusNow(term?.timezone, instant));
  onMount(() => { const t = setInterval(() => (instant = new Date()), 60000); return () => clearInterval(t); });
  const todayOffset = $derived(Math.round((new Date(now.getFullYear(), now.getMonth(), now.getDate()) - start) / 86400e3));
  const nowMin = $derived(now.getHours() * 60 + now.getMinutes());
  const isToday = (i) => offsetOf(i) === todayOffset;
  // The day's meetings, minus any the person removed on that date. On a day
  // that follows another weekday's schedule, classes of that weekday meet;
  // a closure from partway through the day cancels only the later classes.
  const closedOn = (i) => closures[ymd(dateOf(i))] || null;
  const allDay = (i) => closedOn(i) && !closedOn(i).follows && !closedOn(i).after;
  const closureNote = (c) => c.follows ? `${c.name}: classes follow the ${DAY_NAMES[DAYS.indexOf(c.follows)]} schedule` : c.after ? `${c.name}: no classes from ${fmtTime(c.after)}` : `${c.name}: no classes`;
  const forDay = (i) => {
    const d = ymd(dateOf(i));
    const entries = visible.flatMap((it) => meetingsOn(it, d, term, closures).map((m) => ({ it, m })));
    if (taskTimes) for (const t of dueOn(i)) entries.push({ task: t, m: taskSlot(t) });
    return columns(entries);
  };
  const anyClosed = $derived(cols.some((i) => closedOn(i)));
  const y = (m) => (m.start / 60) * ROW;
  const height = (m) => ((m.end - m.start) / 60) * ROW;
  const isPast = (i, m) => offsetOf(i) < todayOffset || (isToday(i) && m.end <= nowMin);

  // Day mode.
  // A picked weekend day stays picked across weeks; a week without weekend columns shows its first day instead.
  const dayI = $derived(cols.includes(order[day]) ? order[day] : cols[0]);
</script>

{#snippet event(i, it, m, col, n)}
  <button class="ev pastel" class:conflict={conflicts.has(it.id)} class:on={it.id === selected} class:past={isPast(i, m)} class:ghost={it.ghost} class:exam={!!it.exam} class:google={!!it.google} aria-pressed={it.id === selected}
    style="top:{y(m)}px;height:{height(m) - 2}px;left:calc({(col * 100) / n}% + 2px);width:calc({100 / n}% - 4px);--c:{it.color};--gc:{it.google?.calendar.color || '#9e958d'}"
    onclick={() => onselect(it.id, ymd(dateOf(i)))} title="{it.label} · {fmtTime(m.start)} – {fmtTime(m.end)}{m.location ? ' · ' + m.location : ''}">
    <b>{it.label}</b>
    <span>{fmtTime(m.start)} – {fmtTime(m.end)}</span>
    {#if height(m) >= 60}
      {#if conflicts.has(it.id)}<span class="ov"><Icon name="warn" size={12} />Overlaps</span>{:else if m.location}<span>{m.location}</span>{/if}
    {/if}
  </button>
{/snippet}

{#snippet taskBox(i, t, m, col, n)}
  <button class="ev pastel due-at" class:done={t.done} class:past={isPast(i, m)}
    style="top:{y(m)}px;height:{height(m) - 2}px;left:calc({(col * 100) / n}% + 2px);width:calc({100 / n}% - 4px);--c:{t.color}"
    onclick={() => ontask(t.id)} title="{t.title} · {t.code} · due {fmtTime(m.end)}{t.done ? ' · done' : ''}">
    <span class="ring" aria-hidden="true"></span><b>{t.title}</b>
  </button>
{/snippet}

{#snippet entry(i, e)}
  {#if e.task}{@render taskBox(i, e.task, e.m, e.col, e.n)}{:else}{@render event(i, e.it, e.m, e.col, e.n)}{/if}
{/snippet}

{#snippet hours()}
  <div class="times">{#each { length: HOURS } as _, h}<div style="top:{h * ROW}px">{fmtHour(h)}</div>{/each}</div>
{/snippet}

{#if mode === 'day'}
  <div class="day">
  <div class="strip" role="tablist" aria-label="Day" style="--days:{cols.length}" bind:this={stickyEl}>
    {#each cols as i (i)}
      <button role="tab" aria-selected={i === dayI} class:on={i === dayI} class:today={isToday(i)} class:closed={allDay(i)} onclick={() => onday(offsetOf(i))}>
        <span class="wd">{DAY_NAMES[i]}</span><span class="dn">{dateOf(i).getDate()}</span>
        <span class="mark" class:some={dueOn(i).length} class:open={dueOn(i).some((t) => !t.done)}></span>
      </button>
    {/each}
  </div>
  {#if closedOn(dayI)}<p class="none"><span class="holiday" class:swap={!!closedOn(dayI).follows}>{closedOn(dayI).name}{#if closedOn(dayI).after} · from {fmtTime(closedOn(dayI).after)}{/if}</span></p>
  {:else if !forDay(dayI).length}<p class="none dim">No classes on {dateOf(dayI).toLocaleDateString(undefined, { weekday: 'long' })}</p>{/if}
  <div class="grid one" style="height:{HOURS * ROW}px" bind:this={gridEl}>
    {@render hours()}
    <div class="col" class:today={isToday(dayI)} class:closed={allDay(dayI)} style="background-size:100% {ROW}px">
      {#if closedOn(dayI)?.after}<div class="cut" style="top:{(closedOn(dayI).after / 60) * ROW}px"></div>{/if}
      {#each forDay(dayI) as e}{@render entry(dayI, e)}{/each}
      {#if isToday(dayI)}<div class="now" style="top:{(nowMin / 60) * ROW}px"></div>{/if}
    </div>
  </div>
  </div>
{:else}
  <div class="cal" style="--days:{cols.length}">
    <div class="sticky" bind:this={stickyEl}>
      <div class="grid head">
        <div></div>
        {#each cols as i (i)}
          <div class="dh" class:today={isToday(i)}>
            <span class="wd">{DAY_NAMES[i]}</span><span class="dn">{dateOf(i).getDate()}</span>
          </div>
        {/each}
      </div>
      {#if anyClosed}
        <div class="grid due closed-row">
          <div class="lbl"></div>
          {#each cols as i (i)}
            <div class="cell" class:today={isToday(i)}>{#if closedOn(i)}<span class="holiday" class:swap={!!closedOn(i).follows} title={closureNote(closedOn(i))}>{closedOn(i).name}</span>{/if}</div>
          {/each}
        </div>
      {/if}
      {#if anyDue}
        <div class="grid due">
          <div class="lbl">Due</div>
          {#each cols as i (i)}
            <div class="cell" class:today={isToday(i)}>
              {#each dueOn(i).slice(0, showAll ? undefined : SHOW) as t (t.id)}
                <button class="task pastel" class:done={t.done} style="--c:{t.color}" onclick={() => ontask(t.id)} title="{t.title} · {t.code}{t.done ? ' · done' : ''}">{t.title}</button>
              {/each}
              {#if dueOn(i).length > SHOW && !showAll}<button class="more" onclick={() => (showAll = true)} title="Show every task due this week">{dueOn(i).length - SHOW} more</button>{/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
    <div class="grid" style="height:{HOURS * ROW}px" bind:this={gridEl}>
      {@render hours()}
      {#each cols as i (i)}
        <div class="col" class:today={isToday(i)} class:closed={allDay(i)} style="background-size:100% {ROW}px">
          {#if closedOn(i)?.after}<div class="cut" style="top:{(closedOn(i).after / 60) * ROW}px"></div>{/if}
          {#each forDay(i) as e}{@render entry(i, e)}{/each}
          {#if isToday(i)}<div class="now" style="top:{(nowMin / 60) * ROW}px"></div>{/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .cal, .day { flex: none; font-size: .78rem; min-width: 0; }
  .cal, .day { --days: 7; }
  .grid { position: relative; display: grid; grid-template-columns: 3rem repeat(var(--days), minmax(0, 1fr)); }
  .grid.one { grid-template-columns: 3rem minmax(0, 1fr); }
  .sticky { position: sticky; top: 0; z-index: 2; background: var(--view); box-shadow: inset 0 -1px var(--line); }
  /* Day headers: weekday and date on one line, today's date in a filled circle. */
  .dh { display: flex; align-items: center; gap: 4px; min-width: 0; height: 32px; margin: 4px 2px; padding: 0 6px; color: var(--muted); line-height: 1; }
  .dh .wd { font-weight: 600; white-space: nowrap; }
  .dh .dn, .strip .dn { display: inline-flex; align-items: center; justify-content: center; min-width: 24px; height: 24px; padding: 0 4px; border-radius: 999px; font-family: var(--display); font-weight: 500; font-size: .9rem; color: var(--text); font-variant-numeric: tabular-nums; }
  /* The due row: a chip per task in its class color, like all-day events. */
  .due { box-shadow: inset 0 1px var(--line); }
  .due .lbl { padding: 5px 8px 0 0; color: var(--muted); font-size: .7rem; line-height: 1; text-align: right; }
  .due .cell { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: 3px 2px; box-shadow: inset 1px 0 var(--line); }
  .due .cell.today { background: var(--today); }
  .task { display: block; width: 100%; padding: 0 6px; border: 0; border-radius: 4px; background: var(--c); color: var(--event-ink); font-size: .72rem; font-weight: 600; line-height: 18px; text-align: left; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  @media (hover: hover) { .task:hover { filter: brightness(.95); } }
  .task.done { text-decoration: line-through; opacity: .55; }
  .task:focus-visible { outline-offset: -2px; }
  .more { padding: 0 6px; border: 0; background: none; color: var(--muted); font-size: .7rem; line-height: 18px; text-align: left; }
  /* Days the school cancels classes: a named chip in the header and a hatched column. */
  .closed-row .cell { justify-content: center; }
  .holiday { display: inline-block; max-width: 100%; padding: 0 8px; border-radius: 999px; background: var(--tint); color: var(--accent-fg); font-size: .72rem; font-weight: 600; line-height: 18px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .holiday.swap { background: var(--hover); color: var(--text2); }
  .col.closed { background-image: repeating-linear-gradient(-45deg, transparent 0 6px, var(--hover) 6px 7px), linear-gradient(var(--line) 1px, transparent 1px); background-size: auto, 100% 56px; }
  .cut { position: absolute; left: 0; right: 0; bottom: 0; border-top: 1px dashed var(--line); background: repeating-linear-gradient(-45deg, transparent 0 6px, var(--hover) 6px 7px); pointer-events: none; }
  .strip button.closed .dn { color: var(--muted); text-decoration: line-through; }
  /* The hours. */
  .times { position: relative; }
  .times div { position: absolute; right: 8px; margin-top: 3px; color: var(--muted); font-size: .7rem; line-height: 1; font-variant-numeric: tabular-nums; }
  .col { position: relative; background-image: linear-gradient(var(--line) 1px, transparent 1px); background-position: 0 -1px; box-shadow: inset 1px 0 var(--line); }
  .col.today { background-color: var(--today); }
  .ev { position: absolute; display: flex; flex-direction: column; align-items: flex-start; gap: 1px; padding: 4px 7px; border: 0; border-radius: 6px; background: var(--c); color: var(--event-ink); text-align: left; line-height: 1.3; overflow: hidden; cursor: pointer; }
  .ev b { max-width: 100%; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .ev span { max-width: 100%; color: var(--event-ink2); font-size: .72rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .ev .ov { display: inline-flex; align-items: center; gap: 3px; color: #a3231b; font-weight: 600; }
  @media (hover: hover) { .ev:hover { filter: brightness(.96); } }
  .ev.on, .ev:focus-visible { outline: 2px solid var(--accent-fg); outline-offset: -2px; }
  .ev.past { opacity: .55; }
  .ev.conflict { box-shadow: inset 0 0 0 1.5px #c9463c; }
  .ev.ghost { background: color-mix(in srgb, var(--c) 55%, transparent); outline: 2px dashed var(--accent-fg); outline-offset: -2px; }
  /* A task on the hours: a deadline, so its block ends at the due time. The
     same check circle as the task rows says what it is (and whether it is
     done), so color is never the only cue. */
  .ev.due-at { flex-direction: row; align-items: center; gap: 6px; padding: 2px 7px 2px 6px; }
  .ring { position: relative; flex: none; width: 14px; height: 14px; border: 1.5px solid var(--event-ink2); border-radius: 50%; }
  .ring::after { content: ''; position: absolute; left: 3.5px; top: 0.5px; width: 3.5px; height: 6.5px; border: solid var(--event-ink); border-width: 0 1.5px 1.5px 0; transform: rotate(45deg) scale(0); }
  .due-at.done .ring { background: var(--event-ink2); border-color: var(--event-ink2); }
  .due-at.done .ring::after { border-color: var(--c); transform: rotate(45deg) scale(1); }
  .ev.due-at.done b { text-decoration: line-through; }
  .ev.due-at.done { opacity: .6; }
  /* A Google Calendar event: a light wash of its calendar's color with a bar
     of it along the edge, so it reads as coming from elsewhere. */
  .ev.google { background: color-mix(in srgb, var(--gc) 30%, #f4f1ec); box-shadow: inset 3px 0 0 var(--gc); padding-left: 9px; }
  /* An exam sitting: the class's color with a ruled edge, so it reads as a date to remember. */
  .ev.exam { box-shadow: inset 0 0 0 1.5px var(--event-ink2); }
  .ev.exam b { text-decoration: underline; text-decoration-thickness: 1px; text-underline-offset: 2px; }
  .now { position: absolute; left: 0; right: 0; z-index: 1; height: 2px; background: var(--now); pointer-events: none; }
  .now::before { content: ''; position: absolute; left: -4px; top: -3px; width: 8px; height: 8px; border-radius: 50%; background: var(--now); }
  /* Day mode. */
  .strip { position: sticky; top: 0; z-index: 2; display: grid; grid-template-columns: repeat(var(--days), 1fr); gap: 2px; padding: 6px 0 4px; background: var(--view); box-shadow: inset 0 -1px var(--line); }
  .strip button { display: flex; flex-direction: column; align-items: center; gap: 3px; padding: 6px 0 4px; border: 0; border-radius: 8px; background: none; color: var(--muted); line-height: 1; }
  .strip .wd { font-size: .7rem; font-weight: 500; }
  .strip .dn { width: 30px; height: 30px; font-size: .95rem; }
  .strip button.on .dn { background: var(--active); }
  .dh.today .wd, .strip button.today .wd { color: var(--accent-fg); }
  .dh.today .dn, .strip button.today .dn { background: var(--accent); color: var(--on-accent); }
  .strip .mark { width: 5px; height: 5px; border-radius: 50%; background: transparent; }
  .strip .mark.some { background: var(--track); }
  .strip .mark.open { background: var(--accent-fg); }
  .grid.one { font-size: .78rem; box-shadow: inset 0 1px var(--line); }
  .none { margin: 12px 4px 4px; }
  .cal > .grid:last-child, .day > .grid:last-child { margin-bottom: 12px; }
</style>
