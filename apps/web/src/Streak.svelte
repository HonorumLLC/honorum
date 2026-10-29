<script>
  // The Progress card's heading: the account's run of days that ended with
  // nothing overdue, in one line of the card's title type, and this week's
  // days as dots in the calendar's week order at the card's edge. Overdue
  // work turns today's dot red; the best run and the overdue count are in
  // the tooltips and read to screen readers. A guest sees an offer to sign
  // in; an account whose streak has not arrived yet sees that it needs a
  // connection. Every state stands one height, so the rings below never move.
  //
  // Motion: the flame rises while the numbers count up, the days fade in in
  // date order and today's ring settles, all within about a second. A new
  // day plays out in order: the ring glides to the new dot, which fills, and
  // the flame lifts as the count goes up. A new week brings its days in
  // afresh. Falling behind turns today red for two slow breaths. Reduce
  // Motion keeps only the final state.
  import { untrack } from 'svelte';
  import { prefersReducedMotion } from 'svelte/motion';
  import { app } from './lib/state.svelte.js';
  import { dialogs } from './lib/ui.svelte.js';
  import { parse, addDays, startOfWeek, ymd } from './lib/dates.js';
  import Count from './Count.svelte';
  import Flame from './Flame.svelte';

  let { streak, now } = $props();
  const days = (n) => (n === 1 ? 'day' : 'days');

  // The current week, from the calendar's first day.
  const CELL = { clean: 'Nothing overdue', missed: 'Something was left overdue', overdue: 'Overdue now', none: 'Before the streak began', future: '' };
  const today = $derived(streak?.today ? parse(streak.today) : new Date(now.getFullYear(), now.getMonth(), now.getDate()));
  const first = $derived(startOfWeek(today, app.weekStart));
  const cells = $derived.by(() => {
    const states = new Map((streak?.days || []).map((d) => [d.day, d.state])), todayKey = ymd(today);
    return Array.from({ length: 7 }, (_, i) => {
      const date = addDays(first, i), key = ymd(date);
      return { key, date, i, today: key === todayKey, state: date > today ? 'future' : states.get(key) || 'none' };
    });
  });
  const mark = $derived(cells.find((c) => c.today));
  const kept = $derived(cells.filter((c) => c.state === 'clean').length);
  const past = $derived(cells.filter((c) => c.state !== 'future').length);
  function title(c) {
    if (!streak || !CELL[c.state]) return undefined;
    const day = c.date.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
    return `${day}: ${c.state === 'overdue' && streak.overdue ? `${streak.overdue} overdue` : CELL[c.state]}`;
  }
  const said = $derived(streak ? `${streak.current} day streak, best ${streak.longest}${streak.overdue ? `, ${streak.overdue} overdue` : ''}` : '');

  // After the entrance, numbers wait for a new day's dot to fill.
  let entered = $state(false);
  $effect(() => { const t = setTimeout(() => (entered = true), 1500); return () => clearTimeout(t); });

  // A new day, in order: the ring glides over (0–450ms), the new dot fills
  // (250ms), and the flame lifts (300ms) as the count goes up (350ms). A new
  // week's days arrive as they do at first, so only the flame lifts.
  let flame = $state(), week = $state();
  let last = untrack(() => streak?.today && { day: streak.today, first: ymd(first) });
  $effect(() => {
    const day = streak?.today, start = ymd(first), was = last;
    last = day && { day, first: start };
    if (!was || !day || day === was.day || prefersReducedMotion.current) return;
    if (start === was.first) week?.querySelector('.cell.today')?.animate([{ transform: 'scale(.4)', opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: 450, delay: 250, easing: 'cubic-bezier(.22, 1, .36, 1)', fill: 'backwards' });
    flame?.animate([{ transform: 'none' }, { transform: 'translateY(-2px) scale(1.08)', offset: .4 }, { transform: 'none' }], { duration: 600, delay: 300, easing: 'ease-in-out' });
  });
</script>

<div class="streak">
  <div class="line" title={streak ? `Days that ended with nothing overdue. Best: ${streak.longest} ${days(streak.longest)}` : undefined}>
    <span class="fl" class:off={!streak} bind:this={flame}><Flame size={20} /></span>
    {#if streak}
      <h3 class="text"><span class="sr">{said}</span><span aria-hidden="true"><Count value={streak.current} delay={entered ? 350 : 80} hold /> day streak</span></h3>
    {:else if app.account}
      <h3 class="text waiting">Connect to update your streak</h3>
    {:else}
      <h3 class="text">Keep a Streak</h3>
    {/if}
  </div>
  {#if streak}
    <div class="week" bind:this={week} role="img" aria-label="{kept} of this week’s {past} {days(past)} ended with nothing overdue">
      {#each cells as c (c.key)}<span class="cell {c.state}" class:today={c.today} style:--i={c.i} title={title(c)}></span>{/each}
      {#if mark}
        {#key ymd(first)}<span class="mark" class:late={mark.state === 'overdue'} style:--col={mark.i}></span>{/key}
      {/if}
    </div>
  {:else if !app.account}
    <button type="button" class="btn pill suggested" onclick={() => (dialogs.account = true)} disabled={!app.config?.phone_auth} title={app.config?.phone_auth ? '' : 'This server cannot send sign-in codes'}>Sign In</button>
  {/if}
</div>

<style>
  /* One height in every state: the Sign In pill's. The heading is the
     card's header, a hairline across the card from the rings below, so it
     need not share their columns. The week and the pill end where the
     percentages' ink does, inside the % sign's side bearing (42 of
     Poppins's 1000 units). */
  .streak { display: flex; align-items: center; justify-content: space-between; gap: 8px; min-height: 24px;
    margin: 0 calc(-1 * var(--pad)); padding: 0 calc(var(--pad) + 1.08rem * .042) var(--pad) var(--pad); border-bottom: 1px solid var(--line); }
  .line { flex: 1; min-width: 0; display: flex; align-items: center; gap: 5px; }
  /* The flame's widest point (5.5 of its 24 units in) meets the rings'
     outer edge, and its ink's middle (12.35 down) the line's. */
  .fl { position: relative; top: calc(20px * -.35 / 24); display: flex; margin-left: calc(20px * -5.5 / 24); }
  .fl.off { filter: grayscale(1); opacity: .45; }
  /* The card's title type. A strut as tall as Poppins's capitals (697 of
     its 1000 units) sets the line, with the text itself at line height 0,
     so the line's middle is the capitals' middle whatever a browser makes of
     the font's ascent and descent. The flame, the dots and the pill center
     on it. */
  .text { min-width: 0; font-family: var(--display); font-size: .95rem; font-weight: 500; line-height: 0; letter-spacing: -.01em; white-space: nowrap; }
  .text::before { content: ''; display: inline-block; height: .697em; }
  .text.waiting { color: var(--muted); }
  .btn { flex: none; min-height: 24px; padding: 0 11px; font-size: .78rem; font-weight: 600; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }

  .week { --dot: 10px; --gap: 5px; position: relative; flex: none; display: grid; grid-template-columns: repeat(7, var(--dot)); grid-auto-rows: var(--dot); gap: var(--gap); }
  /* On the narrowest phones the dots give the text room. */
  @container (max-width: 300px) { .week { --dot: 8px; --gap: 4px; } }
  .cell { border-radius: 50%; background: var(--hover); }
  .cell.clean { background: var(--streak-dot); }
  .cell.missed { background: color-mix(in srgb, var(--muted) 30%, var(--card)); }
  .cell.overdue { background: color-mix(in srgb, var(--red) 18%, var(--card)); }
  .mark { position: absolute; top: 0; left: 0; width: var(--dot); height: var(--dot); border-radius: 50%; outline: 1.5px solid var(--streak-ring); outline-offset: 1.5px; pointer-events: none;
    translate: calc(var(--col) * (var(--dot) + var(--gap))) 0; }
  .mark.late { outline-color: var(--red); }

  @media (prefers-reduced-motion: no-preference) {
    .fl { animation: rise .4s var(--ease-open) backwards; transition: filter .3s, opacity .3s; }
    .text, .btn { animation: surface .4s var(--ease-open) .06s backwards; }
    .cell { animation: dot-in .3s var(--ease-open) backwards; animation-delay: calc(40ms + var(--i) * 40ms); transition: background-color .3s ease; }
    .cell.today.overdue { animation: dot-in .3s var(--ease-open) calc(40ms + var(--i) * 40ms) backwards, breathe 2s ease-in-out .5s 2; }
    .mark { animation: land .35s var(--ease-open) .4s backwards; transition: translate .45s cubic-bezier(.45, 0, .2, 1), outline-color .3s; }
  }
  @keyframes rise { from { opacity: 0; transform: translateY(4px); } }
  @keyframes surface { from { opacity: 0; transform: translateY(3px); } }
  @keyframes dot-in { from { opacity: 0; transform: scale(.7); } }
  @keyframes land { from { opacity: 0; scale: 1.2; } }
  @keyframes breathe { 50% { opacity: .55; } }
</style>
