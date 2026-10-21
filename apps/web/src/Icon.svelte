<script>
  // A symbolic icon from the sprite in index.html.
  let { name, size = 16 } = $props();
</script>

<svg class="i" width={size} height={size} aria-hidden="true"><use href="#i-{name}" /></svg>
