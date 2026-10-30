<script>
  // Every task, by when it is due. The container names the view (the header
  // bar's Tasks button, the phone's Tasks page), so there is no heading: a
  // search entry spans the top, with the filter menu and Add. In the side
  // pane (framed) Progress and the search entry stay put while the list
  // beneath them scrolls, with whatever the pane adds after it.
  //
  // Detail fades with distance. Up Next shows overdue work and this week in
  // full under a heading a day; next week, Later and No Due Date fold into
  // a card each, showing their load rather than their rows, and open in
  // place. Shown, completed work stays where it was due; what was due before
  // today sits above the list, a scroll back from the present, and never
  // under Overdue. A search lists every match. A row
  // says only what its heading does not: the tick circle takes the class's
  // color, and a time shows only when it is not 11:59pm.
  import { app, taskCourses, taskCourseCode, taskColor, colorOf, syncLMS, lmsSync, lmsOf, lmsConnected, setTaskDone, campusZone } from './lib/state.svelte.js';
  import { upNext, taskHeadings, headingLabel, taskWhen, campusDue } from './lib/tasks.js';
  import { startOfWeek, addDays } from './lib/dates.js';
  import { campusNow } from './lib/occurrences.js';
  import { toast, taskView } from './lib/ui.svelte.js';
  import { still, unfold } from './lib/motion.js';
  import { cubicOut, cubicInOut } from 'svelte/easing';
  import { tick as settled, untrack } from 'svelte';
  import { fade } from 'svelte/transition';
  import { PROVIDER_NAMES } from './lib/providers.js';
  import Icon from './Icon.svelte';
  import Menu from './Menu.svelte';
  import Stats from './Stats.svelte';

  // onscreen: whether the list is on screen. The side pane keeps the list
  // while shut, so it opens without building it again; once the list has
  // left the screen it drops its search and folds. Progress is made when the
  // list first comes on screen, so its entrance plays there.
  let { framed = false, onscreen = true, children } = $props();
  let q = $state('');
  // Folds open for as long as the list is on screen, sliding open and shut
  // (see unfold in lib/motion.js).
  let opened = $state({});
  let progress = $state(untrack(() => onscreen));
  $effect(() => { if (onscreen) progress = true; else { q = ''; opened = {}; } });
  const syncing = $derived(lmsSync.owner === app.lms && lmsSync.pending);
  const syncError = $derived(lmsSync.owner === app.lms ? lmsSync.error : '');
  const courses = $derived(taskCourses());
  // A filter for a class that left the schedule is dropped rather than hiding everything.
  $effect(() => { if (taskView.filter && !courses.includes(taskView.filter)) taskView.filter = ''; });
  const words = $derived(q.trim().toLowerCase().split(/\s+/).filter(Boolean));
  const matches = (t) => !words.length || words.every((w) => `${t.title} ${t.course} ${taskCourseCode(t)} ${t.note || ''}`.toLowerCase().includes(w));
  const inFilter = (t) => !taskView.filter || taskCourseCode(t) === taskView.filter || t.course === taskView.filter;
  const shown = $derived(app.tasks.filter((t) => (taskView.showDone || !t.done) && inFilter(t) && matches(t)));
  const zone = $derived(campusZone());
  // Within each heading: by due date as grouped, or by class or title.
  const code = (t) => taskCourseCode(t) || t.course || '';
  const order = (list) => taskView.sort === 'class' ? [...list].sort((a, b) => code(a).localeCompare(code(b)) || (a.due || '9').localeCompare(b.due || '9'))
    : taskView.sort === 'title' ? [...list].sort((a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base' }))
    : list;
  const headed = (list) => taskHeadings(list, new Date(), app.weekStart, zone).map((h) => {
    const [name, date] = headingLabel(h, new Date(), zone);
    return { ...h, name, date, items: order(h.items) };
  });
  const split = $derived(upNext(shown, new Date(), app.weekStart, zone));
  const past = $derived(words.length ? [] : headed(split.past));
  const near = $derived(headed(words.length ? shown : split.near));
  const FOLDS = { next: 'Next Week', later: 'Later', undated: 'No Due Date' };
  const folds = $derived(words.length ? [] : split.folds.map((f) => ({
    ...f, name: FOLDS[f.key], headings: headed(f.items),
    load: f.key === 'next' ? load(f.items) : null,
    classes: f.key === 'next' ? null : classCounts(f.items),
  })));

  // A week fold's load: a column a day, a mark a task in its class's color.
  let narrowDay;
  function load(items) {
    const start = addDays(startOfWeek(campusNow(zone, new Date()), app.weekStart), 7);
    narrowDay ??= new Intl.DateTimeFormat(undefined, { weekday: 'narrow' });
    return Array.from({ length: 7 }, (_, i) => {
      const day = addDays(start, i);
      const due = items.filter((t) => { const d = campusDue(t.due, zone); return d.getFullYear() === day.getFullYear() && d.getMonth() === day.getMonth() && d.getDate() === day.getDate(); });
      return { letter: narrowDay.format(day), marks: due.slice(0, 6).map(taskColor) };
    });
  }
  // A far fold's load: how many tasks each class has.
  function classCounts(items) {
    const by = new Map();
    for (const t of items) {
      const c = code(t), entry = by.get(c) || { label: c || 'No Class', color: taskColor(t), n: 0 };
      entry.n++;
      by.set(c, entry);
    }
    return [...by.entries()].sort(([a], [b]) => (!a) - (!b) || a.localeCompare(b)).map(([, v]) => v);
  }

  const connected = $derived(lmsConnected());
  const sourceNames = { canvas: 'Canvas', feed: 'Calendar' };
  const lmsName = $derived(PROVIDER_NAMES[lmsOf()?.kind] || '');
  const sourceOf = (t) => sourceNames[t.source] || (t.source !== 'manual' ? t.source : '');
  const reasonOf = (t) => t.done && (t.reason === 'submitted' || t.reason === 'graded') ? t.reason[0].toUpperCase() + t.reason.slice(1) : '';
  async function sync() {
    try { await syncLMS(); } catch (e) { toast(e.message); }
  }

  const filterMenu = $derived([
    [{ label: 'Show Completed', check: true, checked: taskView.showDone, action: () => (taskView.showDone = !taskView.showDone) }],
    ...(courses.length > 1 ? [[
      { label: 'All Classes', radio: true, checked: !taskView.filter, action: () => (taskView.filter = '') },
      ...courses.map((c) => ({ label: c, radio: true, checked: taskView.filter === c, action: () => (taskView.filter = c) })),
    ]] : []),
    [
      { heading: 'Sort By' },
      { label: 'Due Date', radio: true, checked: taskView.sort === 'due', action: () => (taskView.sort = 'due') },
      { label: 'Class', radio: true, checked: taskView.sort === 'class', action: () => (taskView.sort = 'class') },
      { label: 'Title', radio: true, checked: taskView.sort === 'title', action: () => (taskView.sort = 'title') },
    ],
  ]);
  const filterLabel = $derived(taskView.filter && courses.length > 1 ? taskView.filter : '');

  // A ticked task nudges aside and fades as its row closes up, so the list
  // settles without a gap or a jump. The tick lands first. Rows leaving for any other
  // reason (a filter, a search, a sync) just go.
  // A group or the empty list's hint changing because of a tick moves with
  // the row; for any other reason it just changes.
  const ticked = new Set();
  let justTicked = false;
  function tick(t, checked) {
    if (checked) ticked.add(t.id); else ticked.delete(t.id);
    justTicked = true;
    setTimeout(() => (justTicked = false));
    setTaskDone(t, checked);
  }
  function swipe(node, { id }) {
    if (!ticked.delete(id) || still()) return { duration: 0 };
    const cs = getComputedStyle(node);
    const h = node.offsetHeight, pt = parseFloat(cs.paddingTop), pb = parseFloat(cs.paddingBottom);
    // A short nudge to the side as it fades, the row closing up at the
    // same time. Nothing clips: the contents stay centered in the closing
    // space and are gone before they could reach a neighbor.
    return {
      delay: 200, duration: 420,
      css: (t, u) => {
        const out = cubicOut(Math.min(1, u / .5)), f = cubicInOut(u);
        const lift = (h / 2 - pt) * (1 - f) - (h - pt - pb) / 2;
        return `min-height: 0; height: ${h * (1 - f)}px; padding-top: ${pt * (1 - f)}px; padding-bottom: ${pb * (1 - f)}px; opacity: ${1 - out}; transform: translate(${out * 24}px, ${lift}px);`;
      },
    };
  }  // A group whose last row was ticked closes with it, heading and all, so
  // the list below rises once rather than jumping when the group goes.
  function closeGroup(node) {
    if (!justTicked || still()) return { duration: 0 };
    const h = node.offsetHeight, mb = parseFloat(getComputedStyle(node).marginBottom);
    return {
      delay: 200, duration: 420,
      css: (t, u) => {
        const f = cubicInOut(u);
        return `height: ${h * (1 - f)}px; margin-bottom: ${mb * (1 - f)}px; opacity: ${1 - cubicOut(Math.min(1, u / .5))};`;
      },
    };
  }

  // The present stays where it is on screen as completed work comes and goes
  // above it, so earlier days are a scroll back; a list that opens with them
  // opens at the present, just under the toolbar.
  let present = $state(), bar = $state();
  const searching = $derived(words.length > 0);
  const scrollerOf = (el) => { for (let n = el?.parentElement; n; n = n.parentElement) if (/auto|scroll/.test(getComputedStyle(n).overflowY)) return n; };
  // In the scroller's own units, which CSS zoom leaves unscaled.
  const offset = (sc) => {
    const r = sc.getBoundingClientRect();
    return (present.getBoundingClientRect().top - r.top) * (sc.offsetHeight / r.height || 1) - (framed ? 0 : bar.offsetHeight);
  };
  $effect.pre(() => {
    taskView.showDone; taskView.filter; searching;
    untrack(() => {
      const sc = present && scrollerOf(present), was = sc ? offset(sc) : past.length ? 0 : null;
      settled().then(() => {
        const sc = present && scrollerOf(present);
        if (sc && was != null) sc.scrollTop += offset(sc) - was;
      });
    });
  });

  // The empty list's hint waits for the last row to close.
  const settle = (node) => (justTicked && !still() ? fade(node, { delay: 560, duration: 200 }) : { duration: 0 });

</script>

{#snippet row(t, kind)}
  {@const course = code(t)}
  {@const when = [taskWhen(t.due, kind, zone), reasonOf(t)].filter(Boolean).join(' · ')}
  <li class="row" class:done={t.done} out:swipe|global={{ id: t.id }}>
    <input type="checkbox" class="check tinted" style:--cc={taskColor(t)} checked={t.done} onchange={(e) => tick(t, e.target.checked)} title="Done" aria-label="Done">
    <button class="row-btn" onclick={() => (app.taskEditing = t.id)} title={course || 'Edit'}>
      <span class="rt"><span class="t wrap">{t.title}</span></span>
      {#if course}<span class="course">{course}</span>{/if}
      {#if when || t.url}<span class="when" class:late={!t.done && t.due && new Date(t.due) < new Date()}>{when}</span>{/if}
    </button>
    {#if t.url}<a class="btn flat icon round open" href={t.url} target="_blank" rel="noopener" title="Open in {sourceOf(t) || 'Browser'}" aria-label="Open {t.title} in {sourceOf(t) || 'the browser'}"><Icon name="open" /></a>{/if}
  </li>
{/snippet}

<section class="tasks" class:framed>
  {#if !app.tasks.length && !connected}
    <!-- Nothing yet and nowhere to bring tasks from: the week's own empty
         state, in the middle of the pane. -->
    <div class="blank" class:pane-scroll={framed}>
      <div class="status">
        <div class="glyph"><Icon name="tasks" /></div>
        <h2>No Tasks</h2>
        <div class="actions">
          <button class="btn pill suggested" onclick={() => (app.connect = PROVIDER_NAMES[lmsOf()?.kind] ? lmsOf().kind : 'pick')}>{lmsName ? `Connect ${lmsName}` : 'Import Tasks'}</button>
          <button class="btn pill" onclick={() => (app.taskEditing = 'new')}>Add Task</button>
        </div>
      </div>
      {@render children?.()}
    </div>
  {:else}
  <div class="head" class:pane-head={framed}>
    {#if progress}<Stats />{/if}
    <div class="toolbar" bind:this={bar}>
      {#if app.tasks.length}
        <label class="entry grow"><Icon name="search" /><input type="search" placeholder="Search tasks…" bind:value={q} onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); q = ''; } }} aria-label="Search tasks"><button type="button" class="clear" onclick={() => (q = '')} aria-label="Clear"><Icon name="close" /></button></label>
        <Menu icon="filter" label={filterLabel} tint={filterLabel ? colorOf(filterLabel) : ''} active={taskView.showDone || taskView.sort !== 'due'} tooltip="Filter and Sort" sections={filterMenu} />
      {:else}
        <span class="grow"></span>
      {/if}
      {#if connected}<button class="btn flat icon" class:spin={syncing} onclick={sync} disabled={syncing} aria-busy={syncing} title="Sync assignments" aria-label="Sync assignments"><Icon name="sync" /></button>{/if}
      <button class="btn flat icon" onclick={() => (app.taskEditing = 'new')} title="Add Task" aria-label="Add task"><Icon name="plus" /></button>
    </div>
  </div>
  <div class:pane-scroll={framed}>
    {#if !app.tasks.length}
      {#if !syncing && !syncError}<p class="hint">Nothing due</p>{/if}
    {/if}

    {#if past.length}
      <div class="past">
        {#each past as h (h.key)}
          <div class="group" out:closeGroup|global>
            <h3 class="group-title dated"><span>{h.name}</span>{#if h.date}<span class="date">{h.date}</span>{/if}</h3>
            <ul class="list">
              {#each h.items as t (t.id)}{@render row(t, h.kind)}{/each}
            </ul>
          </div>
        {/each}
      </div>
    {/if}

    <div class="present" bind:this={present}>
    {#each near as h (h.key)}
      <div class="group" out:closeGroup|global>
        <h3 class="group-title dated" class:overdue={h.kind === 'overdue'}><span>{h.name}</span>{#if h.date}<span class="date">{h.date}</span>{/if}</h3>
        <ul class="list">
          {#each h.items as t (t.id)}{@render row(t, h.kind)}{/each}
        </ul>
      </div>
    {/each}

    {#if !near.length && folds.length}<p class="hint week-clear" in:settle>Nothing due this week</p>{/if}
    {#if folds.length}
      <div class="group">
        <h3 class="group-title">Coming Up</h3>
        <div class="folds">
          {#each folds as f (f.key)}
            <ul class="list">
              <li class="fold-item">
                <button type="button" class="fold" aria-expanded={!!opened[f.key]} aria-label="{f.name}, {f.items.length} {f.items.length === 1 ? 'task' : 'tasks'}" onclick={() => (opened[f.key] = !opened[f.key])}>
                  <span class="ft">
                    <span class="fn">{f.name}<span class="n">{f.items.length}</span></span>
                    {#if f.load}
                      <span class="load" aria-hidden="true">
                        {#each f.load as day, i (i)}
                          <span class="day" class:empty={!day.marks.length}><span class="marks">{#each day.marks as color, j (j)}<i style:--cc={color}></i>{/each}</span><small>{day.letter}</small></span>
                        {/each}
                      </span>
                    {:else}
                      <span class="classes" aria-hidden="true">
                        <span class="share">{#each f.classes as c (c.label)}<i style:--cc={c.color} style:flex={c.n}></i>{/each}</span>
                        <span class="legend" style:--rows={Math.ceil(f.classes.length / 2)}>{#each f.classes as c (c.label)}<span><i style:--cc={c.color}></i><span class="lc">{c.label}</span><b>{c.n}</b></span>{/each}</span>
                      </span>
                    {/if}
                  </span>
                  <span class="chev"><Icon name="down" /></span>
                </button>
              </li>
              {#if opened[f.key]}
                <li class="fold-body" transition:unfold>
                  <ul>
                    {#each f.headings as h (h.key)}
                      <li class="sub" class:overdue={h.kind === 'overdue'}><span>{h.name}</span>{#if h.date}<span class="date">{h.date}</span>{/if}</li>
                      {#each h.items as t (t.id)}{@render row(t, h.kind)}{/each}
                    {/each}
                  </ul>
                </li>
              {/if}
            </ul>
          {/each}
        </div>
      </div>
    {/if}

    {#if !near.length && !folds.length && app.tasks.length}
      <p class="hint" in:settle>{q ? 'No tasks match' : `Nothing to do${taskView.filter ? ` for ${taskView.filter}` : ''}`}</p>
    {/if}
    </div>
    {@render children?.()}
  </div>
  {/if}
</section>

<style>
  .tasks { margin-bottom: 20px; }
  .tasks.framed { flex: 1; min-height: 0; display: flex; flex-direction: column; margin: 0; }
  .framed .pane-head { padding-bottom: 10px; }
  .framed .toolbar { margin-bottom: 0; }
  .framed .pane-scroll { padding-top: 0; }
  .blank { display: flex; flex-direction: column; }
  .framed .blank { padding-top: var(--pad); }
  .blank .status { flex: 1; margin: 0; }
  .toolbar { display: flex; align-items: center; gap: 6px; margin-bottom: 10px; }
  /* On a phone the page scrolls as a whole, and the toolbar stays at its
     top on the page's own background, over its 12px padding. At rest its
     background stops short of Progress's edge. */
  .tasks:not(.framed) .head { display: contents; }
  .tasks:not(.framed) .toolbar { position: sticky; top: -12px; z-index: 1; margin: -10px -12px 0; padding: 10px 12px 10px; background: var(--view); }
  .past { margin-bottom: 20px; }
  .grow { flex: 1; min-width: 0; }
  .dated { display: flex; align-items: baseline; gap: 8px; }
  .date { margin-left: auto; margin-right: 2px; font-weight: 500; }
  .group-title.overdue { color: var(--red); }
  .group { margin-bottom: 12px; }
  .group .group-title { margin-top: 0; }
  .row { position: relative; }
  .done .t { color: var(--muted); text-decoration: line-through; }
  .hint { margin: 4px 2px; font-size: .9rem; }
  .week-clear { margin-bottom: 12px; }

  /* The class's pastel draws the tick circle, darker and stronger so it holds up as a line. */
  .check.tinted { border-color: var(--cc); }
  .marks i, .classes i { background: var(--cc); }
  @supports (color: oklch(from red l c h)) {
    .check.tinted { border-color: oklch(from var(--cc) var(--class-ink-l) calc(c * var(--class-ink-k)) h); }
    .marks i, .classes i { background: oklch(from var(--cc) var(--class-ink-l) calc(c * var(--class-ink-k)) h); }
  }
  .check.tinted:checked { border-color: var(--accent); }
  /* The class stays in the row's accessible name and its tooltip; the color shows it. */
  .course { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .when { flex: none; font-size: .8rem; color: var(--text2); font-variant-numeric: tabular-nums; white-space: nowrap; }
  .when.late { color: var(--red); }
  /* Open in Canvas comes up over the row's end on hover or focus, in place
     of the time; touch has it in the editor. The time's slot keeps the
     button's room (7px in from the end, 34px wide) whether or not there is a
     time, so a long title stops the row's gap short of the button rather
     than running under it. */
  .open { position: absolute; top: 50%; right: 7px; translate: 0 -50%; background: var(--card); }
  @media (hover: hover) {
    .row:has(.open) .when { min-width: 29px; text-align: right; }
    .open { opacity: 0; scale: .8; }
    .row:hover .open, .open:focus-visible { opacity: 1; scale: 1; }
    .row:hover:has(.open) .when, .row:has(.open:focus-visible) .when { opacity: 0; }
    .open:hover { background: linear-gradient(var(--hover), var(--hover)), var(--card); }
  }
  @media (hover: none) { .open, .when:empty { display: none; } }

  .folds { display: grid; gap: 10px; }
  .fold-item { display: block; }
  .fold { display: flex; align-items: center; gap: 12px; width: 100%; min-height: 56px; padding: 10px 12px; border: 0; background: none; color: inherit; text-align: left; }
  @media (hover: hover) { .fold:hover { background: var(--hover); } }
  .fold:focus-visible { outline-offset: -2px; }
  .ft { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 6px; }
  .fn { font-weight: 500; }
  .n { margin-left: 6px; color: var(--muted); font-variant-numeric: tabular-nums; }
  .chev { display: flex; color: var(--muted); }
  .fold[aria-expanded='true'] .chev { transform: rotate(180deg); }
  .load { display: flex; align-items: flex-end; gap: 6px; }
  .day { display: flex; flex-direction: column; align-items: center; gap: 3px; width: 18px; }
  .marks { display: flex; flex-direction: column-reverse; gap: 2px; min-height: 26px; }
  .marks i { width: 10px; height: 5px; border-radius: 2px; }
  .day small { font-size: .66rem; font-weight: 600; line-height: 1; color: var(--muted); }
  .day.empty small { opacity: .55; }
  /* A far fold's load: one bar split by class, and a key down two even
     columns so names and counts line up however many classes there are. */
  .classes { display: flex; flex-direction: column; gap: 8px; margin-top: 2px; }
  .share { display: flex; gap: 2px; height: 6px; }
  .share i { min-width: 6px; border-radius: 3px; }
  .legend { display: grid; grid-template-columns: 1fr 1fr; grid-template-rows: repeat(var(--rows), auto); grid-auto-flow: column; gap: 3px 18px; font-size: .74rem; color: var(--text2); font-variant-numeric: tabular-nums; }
  .legend > span { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .legend i { flex: none; width: 8px; height: 8px; border-radius: 50%; }
  .lc { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .legend b { margin-left: auto; font-weight: 500; color: var(--muted); }
  /* An open fold's rows are a list of their own under the fold's row, so
     the fold can slide them open and shut as one. */
  .fold-body { display: block; }
  .fold-body > ul { border-radius: inherit; }
  .fold-body li + li { border-top: 1px solid var(--line); }
  .fold-body > ul > :last-child { border-bottom-left-radius: inherit; border-bottom-right-radius: inherit; }
  .sub { display: flex; gap: 8px; padding: 8px 12px 4px; font-size: .76rem; font-weight: 600; color: var(--muted); background: color-mix(in srgb, var(--card), var(--bg) 50%); }
  .sub.overdue { color: var(--red); }
  .fold-body .sub + .row { border-top: 0; }
  @media (prefers-reduced-motion: no-preference) {
    .check:checked { animation: pop .25s cubic-bezier(.22, 1, .36, 1); }
    @keyframes pop { from { transform: scale(.85); } }
    /* The chevron turns with its fold, on the fold's curve each way. */
    .chev { transition: transform var(--close) var(--ease-close); }
    .fold[aria-expanded='true'] .chev { transition: transform var(--open) var(--ease-open); }
    .open, .when { transition: opacity var(--quick-open), scale var(--quick-open) var(--ease-open); }
  }
</style>
