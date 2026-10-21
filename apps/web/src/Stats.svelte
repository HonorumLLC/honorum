<script>
  // Progress at the top of Tasks, on two pages under one title: Rings, for
  // today, this week and this term, recounted for the class the task filter
  // picks; and Streak, the account's run of days that ended with nothing
  // overdue. The card appears once there is a task and stays open on the
  // page last shown. Each page animates in when it appears (see Rings.svelte
  // and Streak.svelte); the leaving page fades out over it. Reduce Motion
  // swaps them at once.
  import { app, taskCode, termBounds, termRecord, campusZone } from './lib/state.svelte.js';
  import { RINGS, ringCounts, recordTerm, percent } from './lib/rings.js';
  import { taskView, toast } from './lib/ui.svelte.js';
  import { still } from './lib/motion.js';
  import { untrack, tick } from 'svelte';
  import { fade } from 'svelte/transition';
  import Rings from './Rings.svelte';
  import Streak from './Streak.svelte';

  // The minute clock moves the rings on at midnight.
  let now = $state(new Date());
  $effect(() => { const timer = setInterval(() => (now = new Date()), 60000); return () => clearInterval(timer); });
  const counts = $derived.by(() => {
    const record = recordTerm(termRecord(), app.tasks, { ...termBounds(), now, codeOf: taskCode });
    return ringCounts(app.tasks, { now, weekStart: app.weekStart, record, only: taskView.filter, codeOf: taskCode, zone: campusZone() });
  });

  // Closing a ring by finishing work says so and celebrates it; opening the
  // page, changing the filter or the day never does.
  let seen = null;
  let pulse = $state({ key: '', n: 0 });
  $effect(() => {
    const view = [taskView.filter, app.dataOwner, app.inst, app.term, now.toDateString()].join('|');
    const shown = Object.fromEntries(RINGS.map(([k]) => [k, counts[k].total ? percent(counts[k]) : -1]));
    const closed = seen?.view === view && [...RINGS].reverse().find(([k]) => shown[k] === 100 && seen.shown[k] >= 0 && seen.shown[k] < 100);
    seen = { view, shown };
    if (!closed) return;
    untrack(() => { toast(closed[3]); pulse = { key: closed[0], n: pulse.n + 1 }; });
  });

  const streak = $derived(app.account ? app.streak : null);
  const leave = (node) => (still() ? { duration: 0 } : fade(node, { duration: 90 }));
  async function onkey(e) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    const tabs = e.currentTarget;
    taskView.stats = taskView.stats === 'rings' ? 'streak' : 'rings';
    await tick();
    tabs.querySelector('[aria-selected="true"]')?.focus();
  }
</script>

{#if app.tasks.length}
  <section class="stats" aria-label="Progress" style:--size="112px">
    <div class="head">
      <h3 class="title">Progress</h3>
      <div class="seg" role="tablist" aria-label="Progress pages" tabindex="-1" style:--at={taskView.stats === 'rings' ? 0 : 1} onkeydown={onkey}>
        <span class="thumb" aria-hidden="true"></span>
        <button type="button" role="tab" id="stats-rings" aria-selected={taskView.stats === 'rings'} aria-controls="stats-page" tabindex={taskView.stats === 'rings' ? 0 : -1} onclick={() => (taskView.stats = 'rings')}>Rings</button>
        <button type="button" role="tab" id="stats-streak" aria-selected={taskView.stats === 'streak'} aria-controls="stats-page" tabindex={taskView.stats === 'streak' ? 0 : -1} onclick={() => (taskView.stats = 'streak')}>Streak</button>
      </div>
    </div>
    <div id="stats-page" class="panel" role="tabpanel" aria-labelledby={taskView.stats === 'rings' ? 'stats-rings' : 'stats-streak'}>
      {#if taskView.stats === 'rings'}
        <div class="page" out:leave><Rings {counts} {pulse} /></div>
      {:else}
        <div class="page" out:leave><Streak {streak} {now} /></div>
      {/if}
    </div>
  </section>
{/if}

<style>
  /* One inset on every side, and between the heading and the page. The
     pages' container queries measure inside it. */
  .stats { --pad: 12px; container-type: inline-size; display: flex; flex-direction: column; gap: 12px; margin-bottom: 12px; padding: var(--pad); border-radius: var(--radius); background: var(--card); box-shadow: 0 0 0 1px var(--line); }
  .head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .head .title { font-size: .95rem; }
  /* Two equal tabs; the thumb slides to the chosen one. */
  .seg { position: relative; display: inline-grid; grid-template-columns: 1fr 1fr; padding: 2px; border-radius: 8px; background: var(--hover); }
  .thumb { position: absolute; top: 2px; bottom: 2px; left: 2px; width: calc(50% - 2px); border-radius: 6px; background: var(--card); box-shadow: 0 1px 2px rgba(0, 0, 0, .12); transform: translateX(calc(var(--at) * 100%)); }
  .seg button { position: relative; min-height: 26px; padding: 0 10px; border: 0; border-radius: 6px; background: none; color: var(--text2); font-size: .78rem; font-weight: 600; }
  .seg button[aria-selected='true'] { color: var(--text); }
  .seg button:focus-visible { outline-offset: -2px; }
  /* The leaving page shares the arriving one's cell while it fades. */
  .panel { display: grid; }
  .page { grid-area: 1 / 1; min-width: 0; }
  @media (prefers-reduced-motion: no-preference) {
    .thumb { transition: transform .4s cubic-bezier(.45, 0, .2, 1); }
    .seg button { transition: color .2s; }
  }
</style>
