<script>
  // A number that counts up to its value when it appears, and from its last
  // value when it changes, on the same length and curve as the rings' arcs.
  // Poppins's figures are proportional, so a count changes width as it runs.
  // `hold` sizes it for its final value from the start, so the text after it
  // stays still while it counts; when that final width differs from the
  // last, the number and what follows glide there on the count's curve. The
  // digits are hidden from assistive technology; the caller names the number.
  import { tick, untrack } from 'svelte';
  import { Tween } from 'svelte/motion';
  import { still, COUNT, COUNT_EASE, easeCount } from './lib/motion.js';

  let { value, delay = 0, hold = false } = $props();
  const n = new Tween(0, { duration: COUNT, easing: easeCount });
  let box = $state(), sized = $state(untrack(() => value));
  $effect(() => {
    const to = value;
    untrack(() => {
      n.set(to, still() ? { duration: 0 } : { delay });
      if (!hold || sized === to) return;
      const was = box?.getBoundingClientRect().width;
      sized = to;
      if (was == null || still()) return;
      tick().then(() => {
        const shift = was - box.getBoundingClientRect().width;
        if (Math.abs(shift) > .1) box.animate([{ marginLeft: `${shift}px` }, { marginLeft: '0px' }], { duration: COUNT, delay, easing: COUNT_EASE, fill: 'backwards' });
      });
    });
  });
</script>

{#if hold}
  <span class="count held" bind:this={box} aria-hidden="true"><span class="ghost">{sized}</span><span class="live">{Math.round(n.current)}</span></span>
{:else}
  <span class="count" aria-hidden="true">{Math.round(n.current)}</span>
{/if}

<style>
  .count { font-variant-numeric: tabular-nums; }
  /* The final value, unseen, sets the width and the baseline; the count
     takes no room and sits at its end, so a wider value overflows rather
     than pushing what follows. */
  .held { position: relative; display: inline-block; }
  .ghost { visibility: hidden; }
  .live { position: absolute; top: 0; right: 0; white-space: nowrap; }
</style>
