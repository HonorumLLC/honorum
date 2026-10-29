<script>
  // Progress at the top of Tasks: the account's streak heads the card, and
  // the rings below count today, this week and this term, recounted for the
  // class the task filter picks. The card appears once there is a task. Each
  // part animates in when it appears (see Streak.svelte and Rings.svelte).
  import { app, taskCode, termBounds, termRecord, campusZone } from './lib/state.svelte.js';
  import { RINGS, ringCounts, recordTerm, percent } from './lib/rings.js';
  import { taskView, toast } from './lib/ui.svelte.js';
  import { untrack } from 'svelte';
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
  // card, changing the filter or the day never does.
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
</script>

{#if app.tasks.length}
  <section class="stats" aria-label="Progress">
    <Streak {streak} {now} />
    <Rings {counts} {pulse} />
  </section>
{/if}

<style>
  /* One inset on every side, and between the heading and the rings. The
     parts' container queries measure inside it. */
  .stats { --pad: 12px; container-type: inline-size; display: flex; flex-direction: column; gap: 12px; margin-bottom: 12px; padding: var(--pad); border-radius: var(--radius); background: var(--card); box-shadow: 0 0 0 1px var(--line); }
</style>
