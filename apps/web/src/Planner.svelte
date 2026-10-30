<script>
  // The whole window. Most of the year it is a planner: a header bar with
  // the brand and week navigation at the start, the week in the middle, and
  // every action at the end (Add Event and Edit Classes as one linked unit,
  // the tasks pane's toggle and the primary menu), above the week grid and the side pane on the right with
  // every task. Editing classes is a mode of the same window: a pane with
  // the search entry and its results slides in on the left, the schedule
  // menu and Done take the end, and the side pane stays as it was. Phones switch between Week, Classes and Tasks with
  // a bottom bar; details open as a sheet.
  import { app, closures, previewItems, schedule, mySchedules, institution, selectTerm, section, items, itemOf, colorOf, termInfo, firstDay, newSchedule, setPrimary, deleteSchedule, applySections, autoArrange, shareLink, taskCourseCode, taskColor, openClasses, closeClasses, ensureCatalog, select, setPane } from './lib/state.svelte.js';
  import { untrack } from 'svelte';
  import { cancelSolver } from './lib/solverClient.js';
  import { conflictIds, itemsConflict } from './lib/schedule.js';
  import { addDays, ymd } from './lib/dates.js';
  import { toICS, download } from './lib/export.js';
  import { google, isGoogle } from './lib/google.svelte.js';
  import { googleItems } from './lib/googleEvents.js';
  import { toast, dialogs } from './lib/ui.svelte.js';
  import { fade, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { still, drawer } from './lib/motion.js';
  import { MARK, WORDMARK } from './wordmark.js';
  import Icon from './Icon.svelte';
  import Menu from './Menu.svelte';
  import Dialog from './Dialog.svelte';
  import Week from './Week.svelte';
  import Alerts from './Alerts.svelte';
  import Tasks from './Tasks.svelte';
  import Detail from './Detail.svelte';
  import Find from './Find.svelte';
  import FindBar from './FindBar.svelte';
  import EventDialog from './EventDialog.svelte';

  // The term's catalog loads when classes are edited or a class is opened, not
  // on every visit: the week draws from each schedule's saved copies, and exam
  // sittings arrive with seat counts.

  let { narrow = false, weekTitle = '' } = $props();
  const sched = $derived(schedule());
  const term = $derived(termInfo());
  const list = $derived(arr.previewing && arr.scheduleId === sched?.id ? previewItems(arr.combos[arr.idx]) : items());
  const conflicts = $derived(conflictIds(list));
  const first = $derived(firstDay());
  // Editing classes is a mode on wide screens and the Classes page on
  // phones. Tasks is a page on phones only; a wide window opens the pane.
  const editing = $derived(app.view === 'classes');
  const tasksPage = $derived(narrow && app.view === 'tasks');
  $effect(() => { if (!narrow && app.view === 'tasks') { app.view = 'week'; setPane(true); } });
  const many = $derived(mySchedules().length > 1);
  const termLabel = $derived([term?.name, institution()?.name].filter(Boolean).join(' · '));
  const title = $derived(narrow ? (editing ? 'Classes' : tasksPage ? 'Tasks' : weekTitle) : editing ? 'Edit Classes' : longTitle);

  // Entering and leaving the mode moves nothing that stays: swapped header
  // controls fade in where the old ones were, and the panes slide, their
  // contents at full width so nothing rewraps on the way.
  const appear = (node) => (still() ? { duration: 0 } : fade(node, { duration: 150 }));
  const reveal = (node) => (still() ? { duration: 0 } : slide(node, { duration: 200, easing: cubicOut }));
  // The panes open and close as drawers (see lib/motion.js) and keep their
  // contents while shut. The side pane is made with the window. The class
  // finder is made the first time classes are edited, since it loads the
  // catalog, and slides in then unless the window opened editing.
  const finderAtStart = untrack(() => app.view === 'classes');
  let finderMade = $state(finderAtStart);
  $effect.pre(() => { if (editing) finderMade = true; });
  // Whether the side pane's list is on screen: from when it starts to open
  // until it has finished closing.
  let infoShown = $state(false);

  // A section picked in the finder but not yet added is drawn as a ghost.
  const preview = $derived.by(() => {
    if (!app.selected || !sched || sched.sections.includes(app.selected)) return null;
    const sec = section(app.selected);
    return sec?.meetings ? { ...itemOf(sec, colorOf(sec.course)), ghost: true } : null;
  });
  // Events from the Google calendars the student shows, read-only beside the plan.
  const googleList = $derived(googleItems(google.events, google.calendars, first, term?.timezone));
  const gridItems = $derived([...list, ...(preview ? [preview] : []), ...googleList]);
  const gridConflicts = $derived(preview && list.some((it) => !it.block && !it.exam && itemsConflict(preview, it)) ? new Set([...conflicts, preview.id]) : conflicts);

  // Tasks with due dates, colored like their class, for the week grid.
  const dueTasks = $derived(app.tasks.filter((t) => t.due).map((t) => ({ id: t.id, title: t.title, due: t.due, done: t.done, code: taskCourseCode(t) || t.course, color: taskColor(t) })));
  // The phone's day, as an offset into the shown week: the day that was
  // picked, kept while paging through weeks so Wednesday stays Wednesday;
  // otherwise today when in the week, else the first day.
  let picked = $state(null);
  $effect(() => { app.inst; app.term; picked = null; });
  const todayOff = $derived(Math.round((new Date(new Date().setHours(0, 0, 0, 0)) - first) / 86400e3));
  const dayOff = $derived(picked ?? (todayOff >= 0 && todayOff < 7 ? todayOff : 0));
  const longTitle = $derived.by(() => {
    const last = addDays(first, 6);
    const d = (x, o) => x.toLocaleDateString(undefined, o);
    return first.getMonth() === last.getMonth() ? `${d(first, { month: 'long' })} ${first.getDate()} – ${last.getDate()}, ${last.getFullYear()}` : `${d(first, { month: 'short', day: 'numeric' })} – ${d(last, { month: 'short', day: 'numeric', year: 'numeric' })}`;
  });

  // Rearranging: every conflict-free combination of the schedule's classes,
  // previewed on the week one at a time, kept or undone.
  let arr = $state({ open: false, openOnly: false, busy: false, combos: [], idx: 0, previewing: false, scheduleId: '' });
  let arrangementGeneration = 0;
  $effect(() => { if (arr.scheduleId && arr.scheduleId !== sched?.id) undoArrangement(); });
  async function findArrangements() {
    const generation = ++arrangementGeneration;
    arr.open = false; arr.busy = true; arr.scheduleId = sched.id;
    try {
      const combos = await autoArrange(arr.openOnly);
      if (generation !== arrangementGeneration || arr.scheduleId !== app.current) return;
      if (!combos) return;
      if (!combos.length) { toast('No combination of these classes fits without overlapping'); return; }
      Object.assign(arr, { combos, idx: 0, previewing: true });
    } catch (e) { if (generation === arrangementGeneration) toast(e.message); }
    finally { if (generation === arrangementGeneration) arr.busy = false; }
  }
  function stepArrangement(d) { arr.idx = (arr.idx + d + arr.combos.length) % arr.combos.length; }
  function keepArrangement() {
    if (arr.previewing && arr.scheduleId === app.current) applySections(arr.combos[arr.idx], arr.scheduleId);
    undoArrangement();
  }
  function undoArrangement() {
    arrangementGeneration++; cancelSolver();
    Object.assign(arr, { busy: false, previewing: false, combos: [], scheduleId: '' });
  }

  // Schedule actions.
  let rename = $state({ open: false, name: '' });
  let share = $state({ open: false, link: '' });
  let termDlg = $state(false);
  const overdue = $derived(app.tasks.filter((t) => !t.done && t.due && new Date(t.due) < new Date()).length);
  function doRename() {
    const n = rename.name.trim();
    if (n) sched.name = n;
    rename.open = false;
  }
  function remove() {
    const name = sched.name;
    const undo = deleteSchedule();
    toast(`“${name}” deleted`, { label: 'Undo', run: undo });
  }
  async function doShare() {
    try {
      const link = await shareLink();
      try { await navigator.clipboard.writeText(link); toast('Share link copied'); } catch { share = { open: true, link }; }
    } catch (e) {
      toast(`Could not share: ${e.message}`);
    }
  }
  const exportItems = () => list.map((i) => ({ ...i, sub: '' }));
  const exportICS = () => download(`${sched.name}.ics`, 'data:text/calendar;charset=utf-8,' + encodeURIComponent(toICS(exportItems(), term)));
  const print = () => list.length && window.print();
  const pickSchedule = (id) => { app.current = id; app.selected = ''; };

  // The schedule menu, shown only while editing classes: on a wide screen
  // the term's schedules to switch between, then the schedule's own actions,
  // rearranging, the school and term, and deletion.
  const scheduleMenu = $derived(!sched ? [] : [
    ...(narrow ? [] : [mySchedules().map((s) => ({ key: s.id, label: s.name, radio: true, checked: s.id === app.current, action: () => pickSchedule(s.id) }))]),
    [
      { label: 'New Schedule', action: () => newSchedule() },
      { label: 'Duplicate', action: () => newSchedule(`${sched.name} copy`, sched) },
      { label: 'Rename…', action: () => (rename = { open: true, name: sched.name }) },
      { label: 'Make Primary', disabled: !!sched.primary, action: () => setPrimary(sched.id) },
    ],
    [{ label: 'Rearrange Sections…', disabled: !sched.sections.length || arr.previewing, action: () => (arr = { ...arr, open: true, openOnly: false }) }],
    [{ label: 'School and Term…', action: () => (termDlg = true) }],
    [{ label: 'Delete Schedule', destructive: true, action: remove }],
  ]);
  // Ways the schedule leaves the app: a link, a calendar file, or paper.
  const shareMenu = $derived([[
    { label: 'Share Link', disabled: !sched, action: doShare },
    { label: 'Export Calendar File', disabled: !list.length, action: exportICS },
    { label: 'Print…', disabled: !list.length, action: print },
  ]]);
  // The primary menu at the end of the header bar: sharing, then the items
  // the HIG expects there. F10 opens it.
  const mainMenu = $derived([...shareMenu, [
    { label: 'Settings', action: () => (dialogs.settings = true) },
    { label: 'Keyboard Shortcuts', action: () => (dialogs.shortcuts = true) },
  ]]);
  let primary = $state(null), scheduleMenuRef = $state(null);
  // Tooltips carry the shortcut where there is a keyboard.
  const hint = (k) => (narrow ? '' : ` (${k})`);

  function onkey(e) {
    if (!app.editable || document.querySelector('dialog[open]') || (e.target.closest('input, textarea, select') && !(e.ctrlKey || e.metaKey))) return;
    if (e.key === 'F9' && !narrow && !editing) { e.preventDefault(); setPane(!app.pane); }
    else if (e.key === 'F10' && !e.shiftKey && !e.altKey && !e.ctrlKey) { e.preventDefault(); primary?.toggle(); }
    else if (e.altKey && e.key === 'PageUp') { e.preventDefault(); app.weekOffset--; }
    else if (e.altKey && e.key === 'PageDown') { e.preventDefault(); app.weekOffset++; }
    else if (e.altKey && e.key.toLowerCase() === 't') { e.preventDefault(); app.weekOffset = 0; }
  }
</script>

<svelte:window onkeydown={onkey} />

{#snippet nav()}
  <!-- Previous, Today and Next as one linked unit. -->
  <div class="nav linked">
    <button class="btn icon" onclick={() => app.weekOffset--} title="Previous Week{hint('Alt+PgUp')}" aria-label="Previous Week"><Icon name="prev" /></button>
    {#if narrow}
      <button class="btn icon" onclick={() => (app.weekOffset = 0)} disabled={!app.weekOffset} title="This Week" aria-label="This Week"><Icon name="today" /></button>
    {:else}
      <button class="btn" onclick={() => (app.weekOffset = 0)} disabled={!app.weekOffset} title="This Week{hint('Alt+T')}">Today</button>
    {/if}
    <button class="btn icon" onclick={() => app.weekOffset++} title="Next Week{hint('Alt+PgDn')}" aria-label="Next Week"><Icon name="next" /></button>
  </div>
{/snippet}

{#snippet tabs()}
  <!-- One tab per schedule of the term, the primary one starred. A right
       click picks the tab and opens the schedule menu for it. -->
  {#each mySchedules() as s (s.id)}
    <button class="tab" class:on={s.id === app.current} role="tab" aria-selected={s.id === app.current} title={s.primary ? 'Primary schedule' : undefined} onclick={() => pickSchedule(s.id)} oncontextmenu={(e) => { e.preventDefault(); pickSchedule(s.id); scheduleMenuRef?.toggle(); }}>
      {#if s.primary}<Icon name="star" />{/if}<span class="lbl">{s.name}</span>
    </button>
  {/each}
{/snippet}

{#snippet newTab()}
  <button class="tab add" onclick={() => newSchedule()} title="New Schedule" aria-label="New Schedule"><Icon name="plus" /></button>
{/snippet}

<div class="window">
  <header class="headerbar" class:narrow>
    <div class="start">
      <div class="brand" role="img" aria-label="Honorum">
        <svg class="mark" viewBox={MARK.viewBox} aria-hidden="true"><path d={MARK.d} fill="var(--accent)" fill-rule="evenodd" /></svg>
        <svg class="wm" viewBox={WORDMARK.viewBox} aria-hidden="true"><path d={WORDMARK.d} fill="currentColor" /></svg>
      </div>
      {#if !narrow && sched && !editing}
        <div class="tools" in:appear>{@render nav()}</div>
      {/if}
    </div>
    {#key editing}
      <div class="titlebox" in:appear>
        <h1 class="title">{title}</h1>
        {#if sched && editing && narrow}<span class="subtitle">{termLabel}</span>{:else if sched && many && !tasksPage && !editing}<span class="subtitle">{sched.name}</span>{/if}
      </div>
    {/key}
    <div class="end">
      {#if sched && editing && narrow}
        <Menu bind:this={scheduleMenuRef} tooltip="Schedule Menu" sections={scheduleMenu} />
      {:else if sched && editing}
        <div class="tools" in:appear>
          <Menu bind:this={scheduleMenuRef} icon="" label={sched.name} tooltip="Schedule Menu" sections={scheduleMenu} />
          <button class="btn flat done" onclick={closeClasses} title="Back to the Week (Esc)">Done</button>
        </div>
      {:else if !narrow && sched}
        <div class="tools" in:appear>
          <!-- The week's own edits as one linked unit, like the week controls. -->
          <div class="group linked">
            <button class="btn" onclick={() => (app.editing = 'new')} title="Add Event" aria-label="Add Event"><Icon name="plus" /><span class="lbl">Event</span></button>
            <!-- Reaching for the button starts reading the saved catalog. -->
            <button class="btn icon" onpointerenter={ensureCatalog} onfocus={ensureCatalog} onclick={() => openClasses()} title="Edit Classes (/)" aria-label="Edit Classes"><Icon name="edit" /></button>
          </div>
          <button class="btn flat pane-toggle" class:on={app.pane && !app.selected} aria-pressed={app.pane && !app.selected} onclick={() => { if (app.selected) { app.selected = ''; setPane(true); } else setPane(!app.pane); }} title="Show or Hide Tasks (F9)"><Icon name="pane" /><span>Tasks</span></button>
        </div>
      {/if}
      <Menu bind:this={primary} icon="menu" tooltip="Main Menu" sections={mainMenu} />
    </div>
  </header>
  {#if !sched}
    <div class="status fill">
      <svg class="mark big" viewBox={MARK.viewBox} aria-hidden="true"><path d={MARK.d} fill="var(--accent)" fill-rule="evenodd" /></svg>
      <h2>No School Selected</h2>
      <div class="actions"><button class="btn pill" onclick={() => (termDlg = true)}>Choose a School</button></div>
    </div>
  {:else if tasksPage}
    <div class="page"><Tasks /></div>
  {:else if narrow && editing}
    <div class="page classes">
      <FindBar autofocus={false} />
      <div class="tabstrip"><div class="tabs" role="tablist" aria-label="Schedules">{@render tabs()}</div>{@render newTab()}</div>
      <Alerts />
      <Find />
    </div>
  {:else}
    {#if narrow}
      <div class="toolbar">
        {@render nav()}
        <span class="grow"></span>
        <button class="btn flat icon" onclick={() => (app.editing = 'new')} title="Add Event" aria-label="Add Event"><Icon name="plus" /></button>
      </div>
    {/if}
    <!-- Overlaps are settled while choosing classes; the week marks them on the classes themselves. -->
    {#if conflicts.size && editing}
      <div class="banner error" transition:reveal><span>Some classes overlap</span><button class="btn" onclick={() => (arr = { ...arr, open: true, openOnly: false })} disabled={arr.previewing}>Rearrange Sections…</button></div>
    {/if}
    <div class="split">
      {#if !narrow && finderMade}
        <aside class="pane find" aria-label="Find classes" use:drawer={{ open: editing, side: -1, appear: !finderAtStart }}><div class="pane-body"><div class="pane-head find-head"><FindBar /></div><div class="pane-scroll"><Find /></div></div></aside>
      {/if}
      <div class="content">
        {#if gridItems.length}
          <Week items={gridItems} tasks={dueTasks} conflicts={gridConflicts} selected={app.selected} start={first} weekStart={app.weekStart} {term} closures={closures()} taskTimes={app.taskTimes} mode={narrow ? 'day' : 'week'} day={dayOff} onday={(off) => (picked = off)} onselect={select} ontask={(id) => (app.taskEditing = id)} />
        {:else}
          <div class="status fill">
            <div class="glyph"><Icon name="week" /></div>
            <h2>Nothing Scheduled</h2>
            {#if !editing}<div class="actions"><button class="btn pill suggested" onclick={() => openClasses()}>Add Classes</button></div>{/if}
          </div>
        {/if}
      </div>
      {#if !narrow}
        <aside class="pane info" aria-label="Side pane" use:drawer={{ open: editing || app.pane, side: 1, onshown: (on) => (infoShown = on) }}>
          <div class="pane-body">
            {#key app.selected}
              {#if app.selected}
                <div class="pane-scroll"><Detail /></div>
              {:else}
                <Tasks framed onscreen={infoShown}><Alerts /></Tasks>
              {/if}
            {/key}
          </div>
        </aside>
      {/if}
    </div>
    {#if arr.busy}<div class="actionbar" role="status"><span>Finding arrangements…</span><button class="btn" onclick={undoArrangement}>Cancel</button></div>{/if}
    {#if arr.previewing}
      <div class="actionbar" role="region" aria-label="Rearranged sections">
        <button class="btn flat icon" onclick={() => stepArrangement(-1)} title="Previous option" aria-label="Previous option"><Icon name="prev" /></button>
        <span class="count" aria-live="polite">Option {arr.idx + 1} of {arr.combos.length}</span>
        <button class="btn flat icon" onclick={() => stepArrangement(1)} title="Next option" aria-label="Next option"><Icon name="next" /></button>
        <span class="grow"></span>
        <button class="btn pill" onclick={undoArrangement}>Cancel</button>
        <button class="btn pill suggested" onclick={keepArrangement}>Keep This One</button>
      </div>
    {/if}
  {/if}
  {#if narrow && sched}
    <nav class="viewbar" aria-label="Views">
      <div class="views" role="tablist">
        <button role="tab" class:on={app.view === 'week'} aria-selected={app.view === 'week'} onclick={closeClasses}><Icon name="week" />Week</button>
        <button role="tab" class:on={editing} aria-selected={editing} onpointerdown={ensureCatalog} onclick={() => openClasses(false)}><Icon name="book" />Classes</button>
        <button role="tab" class:on={tasksPage} aria-selected={tasksPage} onclick={() => { app.view = 'tasks'; app.selected = ''; }} aria-label={overdue ? `Tasks, ${overdue} overdue` : 'Tasks'}><Icon name="tasks" />Tasks{#if overdue}<span class="badge" aria-hidden="true">{overdue}</span>{/if}</button>
      </div>
    </nav>
  {/if}
</div>

{#if narrow}
  <Dialog open={!!app.selected} kind="plain" title={section(app.selected) ? 'Class' : isGoogle(app.selected) ? 'Google Event' : app.selected.includes(':exam:') ? 'Exam' : 'Event'} onclose={() => (app.selected = '')}><Detail closable={false} /></Dialog>
{/if}
<EventDialog date={sched && first ? ymd(addDays(first, dayOff)) : ''} />

<Dialog bind:open={termDlg} kind="plain" title="School and Term">
  <div class="list">
    {#if app.config}
      <label class="row"><span class="rt"><span class="t">School</span></span>
        <span class="combo"><select value={app.inst} onchange={(e) => selectTerm(e.target.value, '')} aria-label="School">{#each app.config.institutions as i (i.id)}<option value={i.id}>{i.name}</option>{/each}</select><Icon name="down" /></span>
      </label>
      <label class="row"><span class="rt"><span class="t">Term</span></span>
        <span class="combo"><select value={app.term} onchange={(e) => selectTerm(app.inst, e.target.value)} aria-label="Term">{#each institution()?.terms || [] as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</select><Icon name="down" /></span>
      </label>
    {:else}
      <div class="row"><span class="rt"><span class="t">Offline</span></span></div>
    {/if}
  </div>
  <div class="foot"></div>
</Dialog>

<Dialog bind:open={rename.open} title="Rename Schedule" ok="Rename" okDisabled={!rename.name.trim()} onok={doRename}>
  <!-- svelte-ignore a11y_autofocus -->
  <label class="field"><span>Name</span><input bind:value={rename.name} autofocus onfocus={(e) => e.target.select()}></label>
</Dialog>

<Dialog bind:open={arr.open} title="Rearrange Sections" ok="Find Combinations" onok={findArrangements}>
  <p class="text">Tries other section times so nothing on the schedule overlaps. Each option is previewed on the week, to keep or undo</p>
  <div class="list"><label class="row"><span class="rt"><span class="t">Open Seats Only</span></span><input type="checkbox" class="switch" bind:checked={arr.openOnly}></label></div>
</Dialog>

<Dialog bind:open={share.open} kind="plain" title="Share Link">
  <p class="text">Anyone with this link can import a copy of the schedule</p>
  <!-- svelte-ignore a11y_autofocus -->
  <input class="input" readonly value={share.link} autofocus onfocus={(e) => e.target.select()} aria-label="Share link">
</Dialog>

<style>
  /* The window fills #app (see app.css) rather than 100vh, which on phones runs under the browser's toolbar. */
  .window { display: flex; flex-direction: column; height: 100%; padding: 0 env(safe-area-inset-right) 0 env(safe-area-inset-left); }
  /* Header bar: three alignment points; navigation at the start, actions
     at the end, 4px apart; 12px between groups. The start and end keep their controls, so when the window is too
     narrow for a centered title the title slides toward the shorter side. */
  .headerbar { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: 12px; flex: none; min-height: 48px; padding: 0 6px; box-shadow: inset 0 -1px var(--line); }
  .start, .end { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .end { justify-content: flex-end; gap: 4px; }
  .tools { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .end .tools { gap: 4px; }
  /* Add Event and Edit Classes on one ground, set apart from the pane's toggle. */
  .group { margin-right: 8px; }
  .group .btn:first-child, .pane-toggle { padding: 0 12px 0 10px; }
  .titlebox { display: flex; flex-direction: column; align-items: center; min-width: 0; padding: 4px 0; }
  .brand { display: flex; align-items: center; gap: 8px; padding: 0 4px 0 6px; color: var(--text); }
  .brand .mark { display: block; width: 23px; height: 23px; }
  .status .mark.big { width: 72px; height: 72px; }
  .brand .wm { display: block; height: 12px; width: auto; }
  .title { max-width: 100%; font-size: 1rem; line-height: 1.25; text-align: center; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  /* The subtitle names the schedule when the term has several, or the term while editing classes. */
  .subtitle { max-width: 100%; margin-top: 1px; font-size: .78rem; font-weight: 500; line-height: 1.25; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .done { padding: 0 14px; color: var(--accent-fg); font-weight: 600; }
  .toolbar { display: flex; align-items: center; gap: 12px; flex: none; padding: 4px 8px; box-shadow: inset 0 -1px var(--line); }
  /* Today keeps its ground while already on this week, so the unit stays whole. */
  .nav .btn:disabled { opacity: 1; color: var(--muted); }
  /* The term's schedules as chips on the phone's Classes page; a wide screen lists them in the schedule menu. */
  .tabs { display: flex; min-width: 0; }
  .tab { display: inline-flex; align-items: center; justify-content: center; gap: 6px; min-width: 0; padding: 0 12px; border: 0; background: none; color: var(--text2); font-weight: 500; white-space: nowrap; -webkit-tap-highlight-color: transparent; }
  .tab :global(.i) { width: 14px; height: 14px; color: var(--accent-fg); }
  .tab .lbl { overflow: hidden; text-overflow: ellipsis; }
  @media (hover: hover) { .tab:hover { background: var(--hover); } }
  .tab.add :global(.i) { width: 16px; height: 16px; color: var(--text); }
  .tabstrip { display: flex; align-items: center; gap: 6px; flex: none; margin: 0 -12px; padding: 0 12px; overflow-x: auto; }
  .tabstrip .tabs { gap: 6px; }
  .tabstrip .tab { flex: none; min-height: 36px; border-radius: 999px; background: var(--hover); font-size: .85rem; }
  .tabstrip .tab.on { background: var(--tint); color: var(--accent-fg); }
  .tabstrip .tab.add { width: 36px; padding: 0; border-radius: 50%; background: none; }
  @media (pointer: coarse) { .tabstrip .tab { min-height: 40px; } .tabstrip .tab.add { width: 40px; } }
  .page { flex: 1; min-height: 0; overflow-y: auto; padding: 12px; background: var(--view); }
  .page.classes { display: flex; flex-direction: column; gap: 12px; }
  .split { position: relative; flex: 1; display: flex; min-height: 0; overflow: clip; }
  .content { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow: auto; scrollbar-gutter: stable; padding: 0 12px 16px; background: var(--view); isolation: isolate; }
  /* A pane's frame is the room the week gives up; its body is laid over the
     frame from the window's edge, and holds a head and a scrolling part
     (app.css .pane-head and .pane-scroll). The body keeps its size and place
     while the frame changes, and contains its own layout and painting, so
     moving it only moves its layer. It may lead the week's edge by a frame
     when the page is busy, so it sits above the week, whose layers stay
     inside it. */
  .pane { --w: 23rem; flex: none; width: var(--w); }
  .pane-body { --pad: 14px; --measure: calc(var(--w) - 1px - 2 * var(--pad)); position: absolute; z-index: 1; top: 0; bottom: 0; display: flex; flex-direction: column; width: var(--w); background: var(--bg); contain: strict; will-change: transform; }
  .pane.find .pane-body { left: 0; border-right: 1px solid var(--line); }
  .pane.info .pane-body { right: 0; border-left: 1px solid var(--line); }
  /* Shut, the body waits past the window's edge without drawing its contents. */
  .pane:global(.shut) { width: 0; }
  .pane:global(.shut) .pane-body { content-visibility: hidden; }
  .pane.find:global(.shut) .pane-body { transform: translateX(-100%); }
  .pane.info:global(.shut) .pane-body { transform: translateX(100%); }
  .find-head { padding-bottom: 12px; }
  /* The phone's bottom bar (app.css .viewbar) counts overdue tasks on Tasks. */
  .badge { position: absolute; top: 5px; left: calc(50% + 6px); min-width: 16px; padding: 0 4px; border-radius: 999px; background: var(--red-bg); color: var(--on-red); font-size: .66rem; font-weight: 600; line-height: 16px; }
  .actionbar { display: flex; align-items: center; gap: 6px; flex: none; padding: 7px 12px; box-shadow: inset 0 1px var(--line); }
  .actionbar .count { padding: 0 6px; font-weight: 500; font-variant-numeric: tabular-nums; }
  .grow { flex: 1; }
  .text { margin-bottom: 12px; color: var(--text2); }
  .foot { height: 12px; }
  @media (max-width: 1100px) {
    .pane { --w: 20rem; }
    .pane-body { --pad: 12px; }
    /* Not enough room for the wordmark and the Event label beside the week: the symbol and icons only. */
    .brand .wm, .group .lbl { display: none; }
    .group .btn:first-child { width: 34px; padding: 0; }
  }
  @media (max-width: 760px) {
    /* The phone's header bar: the mark, the view's title, and adjacent
       buttons with larger glyphs. It sits below the status bar when installed. */
    .headerbar { gap: 8px; min-height: calc(48px + env(safe-area-inset-top)); padding: env(safe-area-inset-top) 4px 0 8px; }
    .headerbar .end { gap: 0; }
    .headerbar .start :global(.i), .headerbar .end :global(.i), .toolbar :global(.i:not(.caret)) { width: 20px; height: 20px; stroke-width: 1.6; }
    .brand { padding: 0; }
    .brand .wm { display: none; }
    .titlebox { padding: 3px 0; }
    .content { padding: 0 6px 8px; }
    .banner { margin: 8px 8px 0; border-radius: 8px; }
  }
  @media print {
    .headerbar, .toolbar, .viewbar, .pane, .banner, .actionbar { display: none !important; }
    .window { height: auto; }
    .split, .content { display: block; overflow: visible; }
  }
</style>
