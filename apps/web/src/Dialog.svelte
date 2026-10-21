<script>
  // Every modal surface. kind 'action': a heading, the form, then Cancel and
  // the suggested affirmative button (cancel first; Return confirms, Esc
  // cancels; the affirmative button stays insensitive until the form is valid). kind
  // 'plain': a heading and a close button. kind 'alert': centered heading,
  // one line of body text and a row of buttons. On phones every dialog is a
  // sheet along the bottom edge.
  //
  // pages ([{ id, label, icon, detail }]) turns a plain dialog into a
  // preferences dialog: a sidebar lists the pages beside the one named by the
  // bound `page`, which the caller renders, the way the window pairs a pane
  // with its content. On phones the sheet opens on that list, each page
  // saying where it stands (`detail`), and a page covers it until Back
  // returns. An empty `page` is the list there and the first page in a
  // window. Like a GNOME preferences dialog, a paged dialog keeps one size
  // (`height`) on every page: short pages leave room and long pages scroll.
  import Icon from './Icon.svelte';

  let { open = $bindable(false), kind = 'action', title = '', body = '', ok = '', okDisabled = false, dismissible = true, buttons = [], width = '24rem', height = '27rem', pages = [], page = $bindable(''), onok, onclose, children, extra } = $props();
  const titleId = $props.id();
  const paged = $derived(pages.length > 0);
  const current = $derived(pages.find((p) => p.id === page) || pages[0]);
  let el, bodyEl = $state(), navEl = $state(), backEl = $state();
  // Whether the list of pages and the page share the dialog or take turns.
  const mq = matchMedia('(max-width: 600px)');
  let collapsed = $state(mq.matches);
  $effect(() => {
    if (!paged) return;
    const f = () => (collapsed = mq.matches);
    mq.addEventListener('change', f);
    return () => mq.removeEventListener('change', f);
  });
  $effect(() => {
    if (!el) return;
    if (open && !el.open) {
      el.showModal();
      // Opening focuses the first control, which is rarely the page in view.
      el.querySelector('[aria-current="page"]')?.focus();
    } else if (!open && el.open) el.close();
  });
  // Each page starts at its top.
  $effect(() => { page; if (bodyEl) bodyEl.scrollTop = 0; });
  // Where a page covers the list, focus follows: to Back on the way in and to
  // the page's row on the way out.
  let lastPage = page;
  $effect(() => {
    const from = lastPage;
    lastPage = page;
    if (collapsed && open && from !== page) (page ? backEl : navEl?.querySelector(`[data-page="${from}"]`))?.focus();
  });
  function closed() {
    open = false;
    onclose?.();
  }
  function submit(e) {
    e.preventDefault();
    if (kind === 'action' && !okDisabled) onok?.();
  }
  function press(b) {
    el.close();
    b.action?.();
  }
  // Clicking the backdrop dismisses, like clicking outside a GNOME dialog.
  function backdrop(e) {
    if (dismissible && e.target === el) el.close();
  }
</script>

{#snippet closer()}
  <button type="button" class="btn flat icon round" aria-label="Close" title="Close" onclick={() => el.close()}><Icon name="close" /></button>
{/snippet}

<dialog aria-labelledby={titleId} bind:this={el} class={kind} class:paged style:--w={width} style:--h={height} onclose={closed} onclick={backdrop} oncancel={(e) => { if (!dismissible) e.preventDefault(); }}>
  {#if open}
    <form class="frame" class:shown={paged && !!page} onsubmit={submit}>
      {#if kind === 'alert'}
        <div class="alert-body">
          <h2 id={titleId} class="display">{title}</h2>
          {#if body}<p>{body}</p>{/if}
          {@render children?.()}
        </div>
        <div class="footer center">
          {#each buttons as b (b.label)}
            <button type="button" class="btn pill" class:suggested={b.style === 'suggested'} class:destructive={b.style === 'destructive'} onclick={() => press(b)}>{b.label}</button>
          {/each}
        </div>
      {:else if paged}
        <div class="side">
          <header><h2 id={titleId} class="display">{title}</h2>{@render closer()}</header>
          <nav aria-label="{title} pages" bind:this={navEl}>
            {#each pages as p (p.id)}
              {@const on = !collapsed && p === current}
              <button type="button" class="row" class:on aria-current={on ? 'page' : undefined} aria-labelledby="{titleId}-{p.id}" aria-describedby={collapsed && p.detail ? `${titleId}-${p.id}-detail` : undefined} data-page={p.id} onclick={() => (page = p.id)}>
                <span class="fi"><Icon name={p.icon} /></span>
                <span class="rt"><span class="t" id="{titleId}-{p.id}">{p.label}</span>{#if collapsed && p.detail}<span class="s" id="{titleId}-{p.id}-detail">{p.detail}</span>{/if}</span>
                {#if collapsed}<span class="chev"><Icon name="next" /></span>{/if}
              </button>
            {/each}
          </nav>
        </div>
        <div class="main">
          <header>
            <button type="button" class="btn flat icon round back" aria-label="Back" title="Back" bind:this={backEl} onclick={() => (page = '')}><Icon name="back" /></button>
            <h3 id="{titleId}-page" class="display">{current?.label}</h3>
            {@render closer()}
          </header>
          <div class="body" bind:this={bodyEl} role="region" aria-labelledby="{titleId}-page">{@render children?.()}</div>
        </div>
      {:else}
        <header>
          <h2 id={titleId} class="display">{title}</h2>
          {#if kind === 'plain'}{@render closer()}{/if}
        </header>
        <div class="body" bind:this={bodyEl}>{@render children?.()}</div>
        {#if kind === 'action'}
          <div class="footer">
            {@render extra?.()}
            <span class="grow"></span>
            <button type="button" class="btn pill" disabled={!dismissible} onclick={() => el.close()}>Cancel</button>
            <button type="submit" class="btn pill suggested" disabled={okDisabled}>{ok}</button>
          </div>
        {/if}
      {/if}
    </form>
  {/if}
</dialog>

<style>
  dialog { width: min(var(--w), calc(100vw - 24px)); max-height: calc(100dvh - 48px); margin: auto; padding: 0; border: 0; border-radius: 14px; background: var(--card); color: var(--text); box-shadow: var(--shadow); overflow: hidden; }
  dialog::backdrop { background: rgba(20, 12, 6, .4); }
  .frame { display: flex; flex-direction: column; max-height: calc(100dvh - 48px); }
  header { display: flex; align-items: center; gap: 8px; padding: 18px 20px 8px; }
  header h2 { flex: 1; min-width: 0; font-size: 1.15rem; line-height: 1.3; }
  .body { flex: 1; min-height: 0; overflow-y: auto; padding: 6px 20px 8px; }
  .footer { display: flex; align-items: center; gap: 8px; padding: 10px 20px 18px; }
  .footer.center { justify-content: center; padding-top: 4px; }
  .grow { flex: 1; }
  .alert-body { padding: 24px 24px 12px; text-align: center; }
  .alert-body h2 { font-size: 1.15rem; }
  .alert-body p { margin-top: 6px; color: var(--text2); }
  /* A preferences dialog takes the window's grounds: the list of pages is a
     pane and the page is content, so its boxed lists read as cards. The rows
     need no icon chip while the page is beside them. */
  dialog.paged { background: var(--bg); }
  dialog.paged .frame { flex-direction: row; height: min(var(--h), 100dvh - 48px); }
  .side { flex: none; display: flex; flex-direction: column; width: 13rem; box-shadow: inset -1px 0 var(--line); }
  .side header { padding: 16px 18px 10px; }
  .side header .btn, .back { display: none; }
  nav { display: flex; flex-direction: column; gap: 2px; min-height: 0; padding: 0 8px 8px; overflow-y: auto; }
  nav .row { gap: 10px; min-height: 36px; padding: 0 10px; border-radius: 8px; color: var(--text2); font-weight: 500; }
  nav .row.on { background: var(--active); color: var(--accent-fg); }
  nav .fi { width: auto; height: auto; background: none; color: inherit; }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--view); }
  .main header { display: grid; grid-template-columns: 34px 1fr 34px; gap: 12px; padding: 8px 12px; text-align: center; }
  .main h3 { grid-column: 2; font-size: 1.05rem; line-height: 1.3; }
  .main header .btn:last-child { grid-column: 3; }
  /* A page with nothing to list centers its placeholder; rows never shrink to fit. */
  .main .body { display: flex; flex-direction: column; }
  .main .body > :global(*) { flex: none; }
  .main .body > :global(.status) { flex: 1 0 auto; }
  @media (prefers-reduced-motion: no-preference) {
    dialog[open] { animation: rise .14s ease-out; }
    @keyframes rise { from { opacity: 0; transform: translateY(6px); } }
  }
  @media (max-width: 600px) {
    dialog { width: 100vw; max-width: none; max-height: 94dvh; margin: auto 0 0; border-radius: 14px 14px 0 0; }
    .frame { max-height: 94dvh; }
    /* The sheet opens on the pages as a boxed list; the chosen page covers it. */
    dialog.paged .frame { flex-direction: column; height: min(var(--h), 94dvh); }
    .side { flex: 1; width: auto; min-height: 0; padding-bottom: env(safe-area-inset-bottom); overflow-y: auto; box-shadow: none; }
    .side header { padding: 18px 20px 8px; }
    .side header .btn, .back { display: inline-flex; }
    .main, .shown .side { display: none; }
    .shown .main { display: flex; background: none; }
    nav { flex: none; gap: 0; margin: 6px 20px 20px; padding: 0; border-radius: var(--radius); background: var(--card); box-shadow: 0 0 0 1px var(--line); }
    nav .row { gap: 12px; min-height: 60px; padding: 8px 12px; border-radius: 0; color: inherit; font-weight: 400; }
    nav .row + .row { border-top: 1px solid var(--line); }
    nav .fi { width: 36px; height: 36px; background: var(--tint); color: var(--accent-fg); }
    .footer { padding-bottom: calc(18px + env(safe-area-inset-bottom)); }
    .body:last-child { padding-bottom: calc(8px + env(safe-area-inset-bottom)); }
    dialog[open] { animation-name: sheet; }
    @keyframes sheet { from { transform: translateY(40px); } }
  }
  @media (max-width: 600px) and (prefers-reduced-motion: no-preference) {
    .shown .main { animation: push .16s ease-out; }
    @keyframes push { from { opacity: 0; transform: translateX(16px); } }
  }
</style>
