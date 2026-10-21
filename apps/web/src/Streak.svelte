<script>
  // The Progress card's Streak page: the account's run of days that ended
  // with nothing overdue, the best run, and the last four weeks as dots in
  // the calendar's week order. A guest sees an offer to sign in; an account
  // whose streak has not arrived yet sees the weeks empty.
  //
  // Motion: the flame rises while the numbers count up, the days fade in in
  // date order and today's ring settles, all within about a second. A new
  // day plays out in order: the ring glides to the new dot, which fills, and
  // the flame lifts as the count goes up. Falling behind turns today red
  // for two slow breaths. Reduce Motion keeps only the final state.
  import { untrack } from 'svelte';
  import { prefersReducedMotion } from 'svelte/motion';
  import { app } from './lib/state.svelte.js';
  import { dialogs } from './lib/ui.svelte.js';
  import { parse, addDays, startOfWeek, ymd } from './lib/dates.js';
  import Count from './Count.svelte';
  import Flame from './Flame.svelte';

  let { streak, now } = $props();
  const days = (n) => (n === 1 ? 'day' : 'days');

  // Four weeks ending with the current week.
  const CELL = { clean: 'Nothing overdue', missed: 'Something was left overdue', overdue: 'Overdue now', none: 'Before the streak began', future: '' };
  const cells = $derived.by(() => {
    const today = streak?.today ? parse(streak.today) : new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const first = addDays(startOfWeek(today, app.weekStart), -21), todayKey = ymd(today);
    const states = new Map((streak?.days || []).map((d) => [d.day, d.state]));
    const list = Array.from({ length: 28 }, (_, i) => {
      const date = addDays(first, i), key = ymd(date);
      return { key, date, i, row: Math.floor(i / 7), col: i % 7, today: key === todayKey, state: date > today ? 'future' : states.get(key) || 'none' };
    });
    return list;
  });
  const today = $derived(cells.find((c) => c.today));
  const kept = $derived(cells.filter((c) => c.state === 'clean').length);
  const title = (c) => streak && CELL[c.state] ? `${c.date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}: ${CELL[c.state]}` : undefined;

  // After the entrance, numbers wait for a new day's dot to fill.
  let entered = $state(false);
  $effect(() => { const t = setTimeout(() => (entered = true), 1500); return () => clearTimeout(t); });

  // A new day, in order: the ring glides over (0–450ms), the new dot fills
  // (250ms), and the flame lifts (300ms) as the count goes up (350ms).
  let flame = $state(), grid = $state();
  let last = untrack(() => streak?.today);
  $effect(() => {
    const day = streak?.today, was = last;
    last = day;
    if (!was || !day || day === was || prefersReducedMotion.current) return;
    grid?.querySelector('.cell.today')?.animate([{ transform: 'scale(.4)', opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: 450, delay: 250, easing: 'cubic-bezier(.22, 1, .36, 1)', fill: 'backwards' });
    flame?.animate([{ transform: 'none' }, { transform: 'translateY(-2px) scale(1.08)', offset: .4 }, { transform: 'none' }], { duration: 600, delay: 300, easing: 'ease-in-out' });
  });
</script>

{#snippet fire()}
  <span class="fl" bind:this={flame}>
    <Flame />
  </span>
{/snippet}

<div class="streak">
  <div class="big">
    {#if streak}
      <div class="headline" title="Days that ended with nothing overdue">
        {@render fire()}
        <span class="sr">{streak.current} {days(streak.current)}</span>
        <span class="count" aria-hidden="true">
          <span class="num"><Count value={streak.current} delay={entered ? 350 : 80} /></span><span class="unit">{days(streak.current)}</span>
        </span>
      </div>
      <span class="meta"><span class="sr">Best {streak.longest}</span><span aria-hidden="true">Best <Count value={streak.longest} delay={entered ? 390 : 120} /></span></span>
      {#if streak.overdue}<span class="late"><span class="sr">{streak.overdue} overdue</span><span aria-hidden="true"><Count value={streak.overdue} /> overdue</span></span>{/if}
    {:else if !app.account}
      {@render fire()}
      <h4 class="lead">Keep a Streak</h4>
      <button type="button" class="btn pill suggested" onclick={() => (dialogs.account = true)} disabled={!app.config?.phone_auth} title={app.config?.phone_auth ? '' : 'This server cannot send sign-in codes'}>Sign In</button>
    {:else}
      {@render fire()}
      <h4 class="lead">Streak</h4>
      <span class="meta">Connect to update</span>
    {/if}
  </div>
  <div class="weeks" bind:this={grid} role={streak ? 'img' : undefined} aria-label={streak ? `${kept} of the last 28 days ended with nothing overdue` : undefined} aria-hidden={streak ? undefined : 'true'}>
    {#each cells as c (c.key)}<span class="cell {c.state}" class:today={c.today} style:--i={c.i} title={title(c)}></span>{/each}
    {#if today}
      <span class="mark" class:late={today.state === 'overdue'} style:--row={today.row} style:--col={today.col}></span>
    {/if}
  </div>
</div>

<style>
  /* Stands as tall as the rings, so both pages are one height. */
  .streak { display: flex; align-items: center; gap: 14px; height: var(--size); }
  .big { flex: 1; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; }
  .headline { display: flex; align-items: center; gap: 6px; }
  .fl { position: relative; display: flex; margin-left: -3px; }
  .count { display: flex; align-items: baseline; gap: 5px; }
  .num { display: flex; font-family: var(--display); font-size: 2rem; font-weight: 500; line-height: 1.1; font-variant-numeric: tabular-nums; letter-spacing: -.02em; }
  .unit { font-size: .88rem; font-weight: 500; color: var(--text2); }
  .lead { margin-top: 4px; font-family: var(--display); font-size: 1.02rem; font-weight: 500; white-space: nowrap; }
  .meta { margin-top: 2px; font-size: .8rem; line-height: 1.2; color: var(--muted); font-variant-numeric: tabular-nums; }
  .late { margin-top: 1px; font-size: .8rem; line-height: 1.2; font-weight: 600; color: var(--red); }
  .big .btn { min-height: 30px; margin-top: 10px; padding: 0 16px; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }

  .weeks { --dot: 18px; --gap: 7px; position: relative; flex: none; display: grid; grid-template-columns: repeat(7, var(--dot)); grid-auto-rows: var(--dot); gap: var(--gap); }
  /* On the narrowest phones the dots give the text room. */
  @container (max-width: 300px) { .weeks { --dot: 15px; --gap: 5px; } }
  .cell { border-radius: 50%; background: var(--hover); }
  .cell.clean { background: var(--streak-dot); }
  .cell.missed { background: color-mix(in srgb, var(--muted) 30%, var(--card)); }
  .cell.overdue { background: color-mix(in srgb, var(--red) 18%, var(--card)); }
  .mark { position: absolute; top: 0; left: 0; width: var(--dot); height: var(--dot); border-radius: 50%; outline: 1.75px solid var(--streak-ring); outline-offset: 2px; pointer-events: none;
    translate: calc(var(--col) * (var(--dot) + var(--gap))) calc(var(--row) * (var(--dot) + var(--gap))); }
  .mark.late { outline-color: var(--red); }

  @media (prefers-reduced-motion: no-preference) {
    .fl { animation: rise .4s var(--ease-open) backwards; }
    .count, .meta, .lead, .btn { animation: surface .4s var(--ease-open) .06s backwards; }
    .late { animation: surface .4s var(--ease-open) backwards; }
    .cell { animation: dot-in .3s var(--ease-open) backwards; animation-delay: calc(40ms + var(--i) * 10ms); transition: background-color .3s ease; }
    .cell.today.overdue { animation: dot-in .3s var(--ease-open) calc(40ms + var(--i) * 10ms) backwards, breathe 2s ease-in-out .5s 2; }
    .mark { animation: land .35s var(--ease-open) .4s backwards; transition: translate .45s cubic-bezier(.45, 0, .2, 1), outline-color .3s; }
  }
  @keyframes rise { from { opacity: 0; transform: translateY(4px); } }
  @keyframes surface { from { opacity: 0; transform: translateY(3px); } }
  @keyframes dot-in { from { opacity: 0; transform: scale(.7); } }
  @keyframes land { from { opacity: 0; scale: 1.2; } }
  @keyframes breathe { 50% { opacity: .55; } }
</style>
