<script>
  // The Progress card's Rings page: nested rings for today, this week and this
  // term, with a row for each beside them; the rows are the accessible
  // controls and the rings answer the pointer too. Choosing a ring shows it
  // alone with its count in the middle; choosing it again or Escape brings
  // back all three. Each row's label and count lead and its percentage ends
  // it, so the rows reach across the card.
  //
  // Motion: the rings fade up as the arcs sweep in, outer first, each with
  // its numbers counting up beside it on the same length and curve, so every
  // number lands with its arc. It is all over within a second. A chosen ring
  // eases outward while the others sink back. `pulse` names a ring that just
  // closed; when its arc arrives it swells a little, sends out a ripple and
  // draws a check. Reduce Motion shows every value at once.
  import { untrack, tick } from 'svelte';
  import { Tween } from 'svelte/motion';
  import { cubicInOut } from 'svelte/easing';
  import { still, COUNT, COUNT_EASE } from './lib/motion.js';
  import { RINGS, percent } from './lib/rings.js';
  import Count from './Count.svelte';

  let { counts, pulse } = $props();
  let open = $state('');
  const SIZE = 112, STROKE = 10, GAP = 3, MID = SIZE / 2, LEAD = 80, STAGGER = 70;
  const radius = (i) => MID - STROKE / 2 - i * (STROKE + GAP);
  const delayOf = (i) => LEAD + i * STAGGER;

  // An arc draws from nothing when the page appears, then moves to new
  // counts from wherever it shows. It rests by its dash offset and moves by
  // an animation of it, so no script runs a frame at a time. An empty arc
  // hides, or its round cap would show as a dot.
  const anims = [];
  function arc(node, { i, p }) {
    let at = 0, first = true;
    const go = ({ i, p }) => {
      if (p === at && !first) return;
      const from = still() ? p : parseFloat(getComputedStyle(node).strokeDashoffset);
      const delay = first ? delayOf(i) : 0;
      first = false;
      at = p;
      for (const a of node.getAnimations()) a.cancel();
      node.style.strokeDashoffset = String(100 - p);
      node.style.opacity = p < .5 ? '0' : '';
      if (still()) return;
      const shown = (o) => (o > 99.5 ? 0 : 1);
      anims[i] = node.animate([
        { strokeDashoffset: String(from), opacity: shown(from) },
        { opacity: 1, offset: .05 },
        { strokeDashoffset: String(100 - p), opacity: shown(100 - p) },
      ], { duration: COUNT, delay, easing: COUNT_EASE, fill: 'backwards' });
    };
    node.style.strokeDashoffset = '100';
    go({ i, p });
    return { update: go };
  }
  const radii = RINGS.map((_, i) => new Tween(radius(i), { duration: 450, easing: cubicInOut }));
  $effect(() => { RINGS.forEach(([key], i) => radii[i].set(open === key ? radius(0) : radius(i), still() ? { duration: 0 } : undefined)); });

  // A closed ring celebrates when its arc arrives, not when its count changes.
  const born = untrack(() => pulse.n);
  const groups = $state([]);
  let burst = $state({ i: -1, n: 0 });
  function landed(i, then) {
    const a = anims[i];
    if (!a || a.playState === 'finished') then();
    else a.finished.then(then, () => landed(i, then));
  }
  $effect(() => {
    if (pulse.n <= born || still()) return;
    const i = RINGS.findIndex(([k]) => k === pulse.key);
    tick().then(() => landed(i, () => {
      burst = { i, n: burst.n + 1 };
      groups[i]?.animate([{ transform: 'none' }, { transform: 'scale(1.03)', offset: .35 }, { transform: 'none' }], { duration: 600, easing: 'ease-in-out' });
    }));
  });
  const toggle = (key) => (open = open === key ? '' : key);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="rings" onkeydown={(e) => { if (e.key === 'Escape' && open) { e.stopPropagation(); open = ''; } }}>
  <div class="dial">
    <svg viewBox="0 0 {SIZE} {SIZE}" width={SIZE} height={SIZE} aria-hidden="true">
      {#each RINGS as [key, , color], i (key)}
        {@const r = radii[i].current}
        {@const p = percent(counts[key])}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <g class="ring" class:gone={open && open !== key} style:--c="var(--ring-{color})" bind:this={groups[i]} onclick={() => toggle(key)}>
          <circle class="track" cx={MID} cy={MID} {r} stroke-width={STROKE} />
          <circle class="arc" cx={MID} cy={MID} {r} stroke-width={STROKE} pathLength="100" use:arc={{ i, p }} />
        </g>
      {/each}
      {#if burst.n}
        {@const r = radii[burst.i].current}
        {#key burst.n}
          <g class="burst" style:--c="var(--ring-{RINGS[burst.i][2]})" style:--grow={(r + 10) / r}>
            <circle class="ripple" cx={MID} cy={MID} {r} />
            {#if !open}<path class="tick" d="M48.5 56.5l5 5 10-11" pathLength="1" />{/if}
          </g>
        {/key}
      {/if}
    </svg>
    {#if open}
      {@const c = counts[open]}
      {@const [, label, color] = RINGS.find(([k]) => k === open)}
      {#key open}
        <div class="center" aria-hidden="true">
          <span class="center-label" style:color="var(--ring-{color})">{label}</span>
          <span class="center-count">{#if c.total}<Count value={c.done} delay={100} hold /> / {c.total}{:else}—{/if}</span>
        </div>
      {/key}
    {/if}
  </div>
  <div class="rows">
    {#each RINGS as [key, label, color], i (key)}
      {@const c = counts[key]}
      <button type="button" class="rl" class:on={open === key} class:dim={open && open !== key} style:--i={i} aria-pressed={open === key} onclick={() => toggle(key)}
        aria-label="{label}: {c.total ? `${c.done} of ${c.total} done` : 'nothing due'}">
        <span class="dot" style:background="var(--ring-{color})"></span>
        <span class="st"><span class="lb">{label}</span><span class="ct">{#if c.total}<Count value={c.done} delay={delayOf(i)} hold /> of {c.total}{:else}Nothing due{/if}</span></span>
        <span class="pc"><Count value={percent(c)} delay={delayOf(i)} />%</span>
      </button>
    {/each}
  </div>
</div>

<style>
  /* A row's own inset makes the space between the rings and its dot match
     the card's. The rings take the room the rows leave, from 120px down to
     84px: the rows keep their full width up to what the smallest rings
     leave, so they wrap only once the rings can shrink no more. The
     middle's type scales with the rings. */
  .rings { display: flex; align-items: center; gap: 4px; }
  .dial { position: relative; flex: 0 1 120px; min-width: 84px; container-type: inline-size; }
  svg { display: block; width: 100%; height: auto; overflow: visible; }
  .ring { cursor: pointer; transform-box: view-box; transform-origin: center; }
  .ring circle { fill: none; transform: rotate(-90deg); transform-origin: center; }
  .track { stroke: color-mix(in srgb, var(--c) var(--ring-mix), var(--card)); }
  .arc { stroke: var(--c); stroke-linecap: round; stroke-dasharray: 100 100; }
  .ring.gone { opacity: 0; pointer-events: none; }
  .burst { pointer-events: none; }
  .ripple, .tick { fill: none; stroke: var(--c); opacity: 0; }
  .ripple { stroke-width: 2.5; vector-effect: non-scaling-stroke; transform-box: view-box; transform-origin: center; }
  .tick { stroke-width: 2.6; stroke-linecap: round; stroke-linejoin: round; stroke-dasharray: 1; stroke-dashoffset: 1; }
  .center { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; pointer-events: none; }
  .center-label { font-size: calc(11cqi * 100 / 112); font-weight: 600; line-height: 1.3; }
  .center-count { display: flex; align-items: baseline; gap: .25em; font-family: var(--display); font-size: calc(20cqi * 100 / 112); line-height: 1.15; font-variant-numeric: tabular-nums; white-space: nowrap; }
  /* The rows' highlight reaches 8px into the card's inset so their text
     lines up with the heading's edge. */
  .rows { flex: 1 0 auto; max-width: calc(100% - 84px - 4px + 8px); display: flex; flex-direction: column; gap: 2px; margin-right: -8px; }
  .rl { display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 0 10px; min-height: 36px; padding: 2px 8px; border: 0; border-radius: 8px; background: none; color: inherit; text-align: left; }
  @media (hover: hover) { .rl:hover { background: var(--hover); } }
  .rl.on { background: var(--active); }
  .rl.dim { opacity: .5; }
  .rl:focus-visible { outline-offset: -2px; }
  .dot { width: 10px; height: 10px; border-radius: 50%; }
  .pc { display: inline-flex; justify-content: flex-end; min-width: 2.8em; font-family: var(--display); font-size: 1.08rem; font-weight: 500; font-variant-numeric: tabular-nums; line-height: 1.2; }
  .st { display: flex; flex-direction: column; min-width: 0; line-height: 1.2; }
  .lb { font-size: .86rem; font-weight: 600; }
  .ct { font-size: .76rem; color: var(--muted); font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }


  @media (prefers-reduced-motion: no-preference) {
    svg { animation: focus-in .4s var(--ease-open) backwards; }
    /* The others leave at once and come back after the chosen ring has moved. */
    .ring { transition: opacity .35s ease .15s, transform .5s cubic-bezier(.3, .7, .2, 1) .15s; }
    .ring.gone { transform: scale(.9); transition-duration: .3s, .45s; transition-delay: 0s; }
    .rl { animation: surface .4s var(--ease-open) backwards; animation-delay: calc(60ms + var(--i) * 50ms); transition: opacity .3s, background-color .15s; }
    .center { animation: rise .35s var(--ease-open) .1s backwards; }
    .ripple { animation: ripple .8s cubic-bezier(.2, .7, .3, 1); }
    .tick { animation: tick 1.4s cubic-bezier(.3, .7, .3, 1) .1s; }
  }
  @keyframes focus-in { from { opacity: 0; } }
  @keyframes surface { from { opacity: 0; transform: translateX(4px); } }
  @keyframes rise { from { opacity: 0; transform: translateY(3px); } }
  @keyframes ripple { 0% { opacity: .35; transform: scale(1); } 100% { opacity: 0; transform: scale(var(--grow)); } }
  @keyframes tick { 0% { opacity: 0; stroke-dashoffset: 1; } 15% { opacity: 1; } 35% { stroke-dashoffset: 0; } 80% { opacity: 1; stroke-dashoffset: 0; } 100% { opacity: 0; stroke-dashoffset: 0; } }
</style>
