<script>
  // Class search results, grouped by course, with a round add button on
  // every section that turns into a check once the section is on the
  // schedule. The entry and the filter toggle are FindBar; the filters
  // themselves are a boxed list at the top while shown. Picking a row shows
  // the section in the detail pane and previews it on the week.
  import { app, schedule, select, toggleSection, ensureCatalog, institution } from './lib/state.svelte.js';
  import { finder } from './lib/ui.svelte.js';
  import { searchCatalog } from './lib/catalogClient.ts';
  import { fmtMeeting, DAYS, DAY_NAMES } from './lib/schedule.js';
  import { emptyFilters, isDefault } from './lib/filters.js';
  import Icon from './Icon.svelte';
  import { tick } from 'svelte';

  const pageSize = 40;
  let results = $state([]), offset = $state(0), resultOffset = $state(0), total = $state(0), searchError = $state(''), searching = $state(false);
  let resultList, pageNavigation = false;
  const f = finder.f;
  $effect(() => { app.q; f.open; f.days; f.time; f.mode; f.level; f.attr; offset = 0; });
  const ready = $derived(app.cat.status === 'ready');
  const active = $derived(!isDefault(f));
  const pct = $derived(Math.round(app.cat.progress * 100));
  const inSchedule = $derived(new Set(schedule()?.sections));
  const toggleDay = (d) => (f.days = f.days.includes(d) ? f.days.replace(d, '') : f.days + d);

  ensureCatalog();
  // Searching happens in the worker; only the visible results cross over.
  $effect(() => {
    const query = app.q, filters = $state.snapshot(f), start = offset;
    app.cat.hash; // A newer seat snapshot can change an open-seat result page.
    searchError = '';
    if (!ready) results = [];
    if (!ready) return;
    searching = true;
    let live = true;
    searchCatalog(query, filters, start, pageSize).then(async (r) => {
      if (!live) return;
      results = r.courses; total = r.total; resultOffset = r.offset; searching = false;
      if (pageNavigation) {
        pageNavigation = false;
        await tick();
        if (live) { resultList?.focus({ preventScroll: true }); resultList?.scrollIntoView({ block: 'start' }); }
      }
    }).catch((e) => { if (live) { searchError = e.message; searching = false; } });
    return () => { live = false; };
  });
  function pick(s) {
    app.cache[s.id] = s;
    select(s.id);
  }
  function add(s) {
    toggleSection(s);
  }
  function page(start) { pageNavigation = true; offset = start; }
  const seats = (s) => (s.available < 0 ? 'Seats unknown' : s.available === 0 ? 'Full' : s.capacity ? `${s.available} open` : 'Open');
</script>

{#if finder.filters}
  <div class="list filters">
    <label class="row"><span class="rt"><span class="t">Open seats only</span></span><input type="checkbox" class="switch" bind:checked={f.open}></label>
    <div class="row"><span class="rt"><span class="t">Days</span></span>
      <div class="linked days" role="group" aria-label="Days">
        {#each DAYS.slice(0, 5) as d, i (d)}<button class="btn" class:on={f.days.includes(d)} aria-pressed={f.days.includes(d)} onclick={() => toggleDay(d)}>{DAY_NAMES[i]}</button>{/each}
      </div>
    </div>
    <label class="row"><span class="rt"><span class="t">Time</span></span>
      <span class="combo"><select bind:value={f.time} aria-label="Time"><option value="any">Any</option><option value="morning">Mornings</option><option value="afternoon">Afternoons</option><option value="evening">Evenings</option></select><Icon name="down" /></span>
    </label>
    <label class="row"><span class="rt"><span class="t">Format</span></span>
      <span class="combo"><select bind:value={f.mode} aria-label="Format"><option value="any">Any</option><option>In person</option><option>Online</option><option>Hybrid</option></select><Icon name="down" /></span>
    </label>
    <label class="row"><span class="rt"><span class="t">Level</span></span>
      <span class="combo"><select bind:value={f.level} aria-label="Level"><option value="any">Any</option>{#each ['1', '2', '3', '4'] as l (l)}<option value={l}>{l}000s</option>{/each}<option value="grad">Graduate</option></select><Icon name="down" /></span>
    </label>
    {#if app.cat.attrs.length}
      <label class="row"><span class="rt"><span class="t">Attribute</span></span>
        <span class="combo"><select bind:value={f.attr} aria-label="Attribute"><option value="">Any</option>{#each app.cat.attrs as a (a)}<option>{a}</option>{/each}</select><Icon name="down" /></span>
      </label>
    {/if}
    {#if active}<button class="row" onclick={() => Object.assign(f, emptyFilters())}><span class="rt"><span class="t clear">Clear Filters</span></span></button>{/if}
  </div>
{/if}

{#if app.cat.status === 'missing'}
  <div class="status"><div class="glyph"><Icon name="search" /></div><h2>Classes Not Ready</h2><p>{app.online ? `${institution()?.name || 'This school'}’s classes are still being fetched. Checking again shortly` : 'Offline, and this term is not saved yet'}</p></div>
{:else if app.cat.status === 'error'}
  <div class="status"><div class="glyph"><Icon name="warn" /></div><h2>Classes Could Not Be Loaded</h2><div class="actions"><button class="btn pill" onclick={ensureCatalog}>Try Again</button></div></div>
{:else if !ready}
  <div class="loading" aria-live="polite">
    <div class="progress"><span style:width="{pct}%"></span></div>
    <p class="caption">Loading classes{pct ? ` · ${pct}%` : '…'}</p>
  </div>
{:else if searchError}<p role="alert">{searchError}</p>
{:else if (app.q || active) && !results.length}
  <div class="status"><div class="glyph"><Icon name="search" /></div><h2>No Results</h2></div>
{/if}

<ul class="results" bind:this={resultList} tabindex="-1" aria-label="Class search results" aria-busy={searching}>
  {#each results as c (c.code)}
    <li class="course">
      <div class="ch"><b>{c.code}</b><span>{c.title}</span></div>
      {#if c.offered?.length > 1}<div class="offered">{#each c.offered as t (t)}<span>{t}</span>{/each}</div>{/if}
      <ul class="list">
        {#each c.sections as s (s.id)}
          {@const added = inSchedule.has(s.id)}
          <li class="row sec" class:on={s.id === app.selected}>
            <button class="row-btn" onclick={() => pick(s)} aria-pressed={s.id === app.selected}>
              <span class="rt">
                <span class="t wrap"><b>{s.num}</b> <span class="dim">{s.type}{s.mode === 'Online' ? ' · online' : ''}</span> · {s.meetings.map(fmtMeeting).join('; ') || 'No meeting times'}</span>
                <span class="s">{s.instructors.join(', ') || 'Staff'} · <span class:full={s.available === 0}>{seats(s)}</span></span>
              </span>
            </button>
            <button class="add" class:on={added} onclick={() => add(s)} title={added ? 'Remove From Schedule' : 'Add to Schedule'} aria-label={added ? `Remove ${c.code} ${s.num} From Schedule` : `Add ${c.code} ${s.num} to Schedule`}><Icon name={added ? 'check' : 'plus'} /></button>
          </li>
        {/each}
      </ul>
    </li>
  {/each}
</ul>
{#if total > 0}
  <nav class="pages" aria-label="Search result pages">
    <button class="btn" disabled={searching || resultOffset === 0} onclick={() => page(Math.max(0, resultOffset - pageSize))}>Previous</button>
    <span class="caption" role="status">{resultOffset + 1}–{Math.min(resultOffset + pageSize, total)} of {total} sections</span>
    <button class="btn" disabled={searching || resultOffset + pageSize >= total} onclick={() => page(resultOffset + pageSize)}>Next</button>
  </nav>
{/if}

<style>
  .filters { margin-bottom: 12px; }
  .filters .row { min-height: 44px; }
  .days .btn { min-height: 30px; min-width: 0; padding: 0 9px; }
  .filters .clear { color: var(--accent-fg); }
  .loading { margin-top: 12px; }
  .loading p { margin-top: 6px; }
  .results { display: flex; flex-direction: column; gap: 14px; }
  .ch { display: flex; flex-direction: column; margin: 0 2px 6px; }
  .ch b { font-family: var(--display); font-weight: 500; font-size: 1rem; }
  .ch span { color: var(--text2); font-size: .88rem; }
  .offered { display: flex; flex-wrap: wrap; gap: 4px; margin: -2px 2px 6px; }
  .offered span { padding: 0 6px; border-radius: 4px; background: var(--hover); color: var(--text2); font-size: .74rem; line-height: 18px; }
  .sec { align-items: center; gap: 8px; min-height: 52px; padding: 7px 8px 7px 12px; }
  .sec.on { background: var(--tint); }
  .sec .t b { font-weight: 600; }
  .sec .full { color: var(--red); font-weight: 600; }
  /* Add or remove: a small round button, filled once the section is on the schedule. */
  .add { display: inline-flex; align-items: center; justify-content: center; flex: none; width: 30px; height: 30px; padding: 0; border: 0; border-radius: 50%; background: var(--hover); color: var(--text2); -webkit-tap-highlight-color: transparent; }
  @media (hover: hover) { .add:hover { background: var(--active); } }
  .add.on { background: var(--accent); color: var(--on-accent); }
  @media (pointer: coarse) { .add { width: 44px; height: 44px; } }
  .pages { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin: 14px 0; }
  .pages .caption { text-align: center; }
</style>
