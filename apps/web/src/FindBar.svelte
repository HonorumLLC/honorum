<script>
  // The class finder's entry and filter toggle: at the top of the finder
  // pane while editing classes on a wide screen, and of the Classes page on
  // phones. Both draw the same state, so the text and filters survive a
  // change of layout. autofocus puts the caret in the entry when it appears
  // and whenever the finder is opened with focus.
  import { app, closeClasses } from './lib/state.svelte.js';
  import { finder } from './lib/ui.svelte.js';
  import { isDefault } from './lib/filters.js';
  import Icon from './Icon.svelte';

  let { autofocus = true } = $props();
  let input;
  const active = $derived(!isDefault(finder.f));
  $effect(() => { app.focusFind; if (autofocus) input?.focus(); });
  // Esc clears the entry; a second Esc leaves the finder.
  function onkey(e) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    if (app.q) app.q = '';
    else { input.blur(); closeClasses(); }
  }
</script>

<div class="bar">
  <label class="entry">
    <Icon name="search" />
    <input type="search" placeholder="Course, title or instructor" bind:value={app.q} bind:this={input} onkeydown={onkey} aria-label="Search classes">
    <button type="button" class="clear" onclick={() => (app.q = '')} aria-label="Clear"><Icon name="close" /></button>
  </label>
  <button class="btn flat icon" class:on={finder.filters || active} aria-pressed={finder.filters} onclick={() => (finder.filters = !finder.filters)} title="Filters" aria-label="Filters"><Icon name="filter" /></button>
</div>

<style>
  .bar { display: flex; gap: 4px; align-items: center; min-width: 0; }
  .entry { flex: 1; min-width: 0; }
</style>
