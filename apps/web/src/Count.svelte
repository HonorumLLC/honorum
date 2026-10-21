<script>
  // A number that counts up to its value when it appears, and from its last
  // value when it changes, on the same length and curve as the rings' arcs.
  // The digits are hidden from assistive technology; the caller names the
  // number.
  import { Tween } from 'svelte/motion';
  import { still, COUNT, easeCount } from './lib/motion.js';

  let { value, delay = 0 } = $props();
  const n = new Tween(0, { duration: COUNT, easing: easeCount });
  $effect(() => { n.set(value, still() ? { duration: 0 } : { delay }); });
</script>

<span class="count" aria-hidden="true">{Math.round(n.current)}</span>

<style>
  .count { font-variant-numeric: tabular-nums; }
</style>
