<script>
  // The seat alerts, in the side pane beneath the tasks.
  import { app, section, select, unwatch, watch } from './lib/state.svelte.js';
  import { toast } from './lib/ui.svelte.js';
  import Icon from './Icon.svelte';

  const watches = $derived(app.watches.filter((w) => w.inst === app.inst && w.term === app.term));
  const iosTip = /iPhone|iPad/.test(navigator.userAgent) && !matchMedia('(display-mode: standalone)').matches;
  const seatText = (id) => {
    const s = section(id) || app.seats[id];
    if (!s || s.available < 0) return 'Checking';
    return s.available === 0 ? 'Full' : s.capacity ? `${s.available} open` : 'Open';
  };
  // Removing an alert can be undone.
  async function removeAlert(w) {
    const snap = $state.snapshot(w);
    try { await unwatch(w); } catch (e) { toast(e.message); return; }
    const cut = snap.label.lastIndexOf('-');
    const sec = { id: snap.section, course: snap.label.slice(0, cut), num: snap.label.slice(cut + 1), title: snap.title };
    toast(`Alert for ${snap.label} removed`, { label: 'Undo', run: () => watch(sec).catch((e) => toast(e.message)) });
  }
</script>

{#if watches.length}
  <section class="group">
    <div class="gh"><h2 class="title">Seat Alerts</h2></div>
    {#if iosTip}<p class="note">Add Honorum to the Home Screen to receive notifications on this device</p>{/if}
    <ul class="list">
      {#each watches as w (w.id)}
        <li class="row">
          <button class="row-btn" onclick={() => select(w.section)}>
            <span class="rt"><span class="t">{w.label}</span><span class="s">{w.title || ''}</span></span>
          </button>
          <span class="seat" class:full={seatText(w.section) === 'Full'}>{seatText(w.section)}</span>
          <button class="btn flat icon round" onclick={() => removeAlert(w)} title="Remove alert" aria-label="Remove alert for {w.label}"><Icon name="trash" /></button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .group { margin-bottom: 20px; }
  .gh { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; margin-bottom: 6px; }
  .note { margin-bottom: 8px; padding: 8px 12px; border-radius: 8px; background: var(--tint); font-size: .85rem; }
  .seat { flex: none; color: var(--muted); font-size: .85rem; white-space: nowrap; }
  .seat.full { color: var(--red); font-weight: 600; }
</style>
