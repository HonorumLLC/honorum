<script>
  // Classes found in Canvas, to confirm before adding: each Canvas course
  // matched to the catalog with its sections, and a menu for each choice
  // left, like the lab a lecture needs. A class with a section is on; one
  // that is only a choice turns on once chosen. Without its own actions, the
  // caller's button adds `count` classes through `add`.
  import { fmtMeeting } from './lib/schedule.js';
  import Icon from './Icon.svelte';

  let { found, busy = false, onadd, cancel = '', oncancel = null, actions = true, count = $bindable(0) } = $props();

  const KINDS = { LEC: 'Lecture', LAB: 'Lab', DIS: 'Discussion', REC: 'Recitation', SEM: 'Seminar', STU: 'Studio', TUT: 'Tutorial' };
  const sec = (id) => found.sections[id] || {};
  const line = (s) => [s.num, ...(s.meetings || []).map(fmtMeeting)].join(' · ');
  // What a choice picks: its sections' kind when they share one.
  const kindOf = (group) => {
    const kinds = [...new Set(group.map((id) => sec(id).type))];
    return kinds.length === 1 && KINDS[kinds[0]] ? KINDS[kinds[0]] : 'Section';
  };

  let picks = $state([]);
  $effect.pre(() => { picks = found.classes.map((c) => ({ on: c.sections.length > 0, chosen: c.options.map(() => '') })); });
  const ids = (i) => [...found.classes[i].sections, ...(picks[i]?.chosen || []).filter(Boolean)];
  const on = $derived(found.classes.filter((c, i) => picks[i]?.on && ids(i).length).length);
  $effect.pre(() => { count = on; });
  export const add = () => onadd(found.classes.map((c, i) => ({ course: c.course, code: c.code, ids: ids(i) })).filter((c, i) => picks[i].on && c.ids.length));
</script>

<div class="list found">
  {#each found.classes as c, i (c.course.id)}
    {#if picks[i]}
      <label class="row">
        <span class="rt">
          <span class="t wrap">{c.code} · {c.title}</span>
          {#each c.sections as id (id)}<span class="s">{line(sec(id))}</span>{/each}
        </span>
        <input type="checkbox" class="switch" bind:checked={picks[i].on} aria-label="Add {c.code}">
      </label>
      {#each c.options as group, g (g)}
        <label class="row">
          <span class="rt"><span class="t">{c.code} {kindOf(group)}</span></span>
          <span class="combo">
            <select bind:value={picks[i].chosen[g]} onchange={(e) => { if (e.currentTarget.value) picks[i].on = true; }} aria-label="{c.code} {kindOf(group)}">
              <option value="">Choose…</option>
              {#each group as id (id)}<option value={id}>{line(sec(id))}</option>{/each}
            </select><Icon name="down" />
          </span>
        </label>
      {/each}
    {/if}
  {/each}
</div>
{#if actions}<div class="actions">
  {#if oncancel}<button type="button" class="btn pill" onclick={oncancel} disabled={busy}>{cancel}</button>{/if}
  <button type="button" class="btn pill suggested" disabled={!on || busy} onclick={add}>{on ? `Add ${on} ${on === 1 ? 'Class' : 'Classes'}` : 'Add Classes'}</button>
</div>{/if}

<style>
  .found { width: 100%; text-align: left; }
  .found .combo { max-width: 60%; }
  .actions { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; margin-top: 20px; }
</style>
