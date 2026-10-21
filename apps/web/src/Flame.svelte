<script>
  // The streak's flame, the web counterpart of the iOS flame.fill: a warm
  // gradient from its base to its tip with a lighter core.
  let { size = 28 } = $props();
  const id = $props.id();
</script>

<svg class="flame" viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
  <defs><linearGradient id="{id}-flame" x1="0" y1="1" x2="0" y2="0"><stop offset=".15" style:stop-color="var(--flame)" /><stop offset="1" style:stop-color="var(--flame-tip)" /></linearGradient></defs>
  <path fill="url(#{id}-flame)" d="M12 21a6.5 6.5 0 0 0 6.5-6.5c0-4.3-3.3-6.6-4.4-10.8-2.2 2.1-3.3 4.4-3.3 6.5-1.1-1.1-1.6-2.2-1.6-3.3C7 9.6 5.5 12.3 5.5 14.5A6.5 6.5 0 0 0 12 21z" />
  <path class="core" d="M12 19.4a3.3 3.3 0 0 0 3.3-3.3c0-2.2-1.6-3.3-2.3-5.4-1.1 1-1.7 2.2-1.8 3.4-.5-.4-.9-1-1-1.6-1 1-1.5 2.4-1.5 3.6A3.3 3.3 0 0 0 12 19.4z" />
</svg>

<style>
  .flame { position: relative; display: block; flex: none; overflow: visible; }
  .core { fill: var(--flame-core); opacity: .85; }
</style>
