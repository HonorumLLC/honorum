<script>
  // The window: one view, the planner, plus the welcome pages and the
  // dialogs every part of the app can open. Every location has a URL so the
  // back button works: #classes, #classes/<section>, #tasks, #settings,
  // #settings/<page>, #connect.
  import { app, boot, refreshConfig, ensureCatalog, snapshot, schedulePersist, openClasses, closeClasses, firstDay } from './lib/state.svelte.js';
  import { addDays } from './lib/dates.js';
  import { toasts, dismissToast, alert, dialogs, SETTINGS_PAGES } from './lib/ui.svelte.js';
  import Icon from './Icon.svelte';
  import Dialog from './Dialog.svelte';
  import Planner from './Planner.svelte';
  import Welcome from './Welcome.svelte';
  import TaskDialog from './TaskDialog.svelte';

  boot().catch((e) => { app.storageError = e.message; app.loading = false; });

  $effect(() => {
    if (app.loading || !app.editable) return;
    $state.snapshot(snapshot()); // subscribe to everything worth saving
    schedulePersist();
  });
  // While a term has no readable publication, check back every 30 seconds.
  $effect(() => {
    if (app.loading || app.cat.status !== 'missing') return;
    const t = setInterval(async () => { await refreshConfig(true); if (app.online) ensureCatalog(); }, 30000);
    return () => clearInterval(t);
  });

  const mq = matchMedia('(max-width: 760px)');
  let narrow = $state(mq.matches);
  $effect(() => {
    const f = () => (narrow = mq.matches);
    mq.addEventListener('change', f);
    return () => mq.removeEventListener('change', f);
  });

  const route = $derived(dialogs.settings ? (dialogs.settingsPage ? `settings/${dialogs.settingsPage}` : 'settings') : app.connect ? 'connect' : app.view === 'classes' ? `classes${app.selected ? '/' + app.selected : ''}` : app.view === 'tasks' ? 'tasks' : '');
  let routed = false, lastRoute = '';
  $effect(() => {
    if (app.loading) return;
    const first = !routed;
    routed = true;
    if (first) onhash(); // Interpret navigation after boot handles sharing and alert links.
    // In a window, moving between Settings pages replaces the entry, so Back
    // closes Settings. On a phone a page covers the list of pages, so Back
    // returns to the list.
    const within = route.startsWith('settings') && lastRoute.startsWith('settings') && !matchMedia('(max-width: 600px)').matches;
    if (location.hash.slice(1) !== route) history[first || within ? 'replaceState' : 'pushState'](null, '', route ? '#' + route : location.pathname);
    lastRoute = route;
  });
  function onhash() {
    const [head, arg] = location.hash.slice(1).split('/');
    dialogs.settings = head === 'settings';
    if (dialogs.settings) dialogs.settingsPage = SETTINGS_PAGES.includes(arg) ? arg : '';
    app.connect = head === 'connect' ? app.connect || 'pick' : '';
    if (head === 'classes') { openClasses(false); app.selected = arg || ''; }
    else if (head === 'tasks') { app.view = 'tasks'; app.selected = ''; }
    else if (!head) closeClasses();
  }

  const weekTitle = $derived.by(() => {
    const first = firstDay(), last = addDays(first, 6);
    const d = (x, o) => x.toLocaleDateString(undefined, o);
    return first.getMonth() === last.getMonth() ? `${d(first, { month: 'short' })} ${first.getDate()} – ${last.getDate()}` : `${d(first, { month: 'short', day: 'numeric' })} – ${d(last, { month: 'short', day: 'numeric' })}`;
  });
  $effect(() => { document.title = narrow && app.view === 'classes' ? 'Classes · Honorum' : narrow && app.view === 'tasks' ? 'Tasks · Honorum' : 'Honorum'; });

  const shortcuts = [
    ['General', [['Edit classes', '/'], ['Show or hide tasks', 'F9'], ['Main menu', 'F10'], ['Settings', 'Ctrl+,'], ['Keyboard shortcuts', 'Ctrl+?']]],
    ['Week', [['Previous, next week', 'Alt+PgUp, Alt+PgDn'], ['This week', 'Alt+T'], ['Print', 'Ctrl+P']]],
    ['Anywhere', [['Close a pane, menu or dialog', 'Esc']]],
  ];

  function onkey(e) {
    if (!app.editable || document.querySelector('dialog[open]') || (e.target.closest('input, textarea, select') && !(e.ctrlKey || e.metaKey) && e.key !== 'Escape')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key;
    if (!ctrl && !e.altKey && k === '/') { e.preventDefault(); app.view === 'classes' ? closeClasses() : openClasses(); }
    else if (ctrl && k === ',') { e.preventDefault(); dialogs.settings = true; }
    else if (ctrl && (k === '?' || (e.shiftKey && k === '/'))) { e.preventDefault(); dialogs.shortcuts = true; }
    else if (k === 'Escape' && !e.target.closest('.entry')) { if (app.selected) app.selected = ''; else if (app.view === 'classes') closeClasses(); }
  }

</script>

<svelte:window onkeydown={onkey} onhashchange={onhash} />

{#if app.storageCleared}
  <div class="status" role="status">
    <p>Site data was cleared. Reload to start fresh.</p>
    <button class="btn suggested" onclick={() => location.reload()}>Reload</button>
  </div>
{:else}
{#if app.storageError}<div class="banner" role="alert">Changes could not be saved: {app.storageError}. Keep this tab open.</div>{/if}
<div class="app-content" inert={!app.editable}>
{#if !app.loading && !app.onboarded}
  <Welcome />
{/if}

{#if !app.loading && app.onboarded}
  <Planner {narrow} {weekTitle} />
{/if}

<TaskDialog />

{#if dialogs.settings}
  {#await import('./Settings.svelte') then { default: Settings }}<Settings />{/await}
{/if}
{#if app.connect}
  {#await import('./Connect.svelte') then { default: Connect }}<Connect />{/await}
{/if}
{#if dialogs.account}
  <!-- Sign-in needs the network, so its phone-number code is not in the offline shell. -->
  {#await import('./Account.svelte') then { default: Account }}<Account />{:catch}<Dialog bind:open={dialogs.account} kind="alert" title="Offline" body="Connect to the internet to sign in" buttons={[{ label: 'OK' }]} />{/await}
{/if}

<Dialog bind:open={dialogs.shortcuts} kind="plain" title="Keyboard Shortcuts">
  {#each shortcuts as [group, keys] (group)}
    <h3 class="group-title">{group}</h3>
    <ul class="list">
      {#each keys as [what, key] (what)}<li class="row"><span class="rt"><span class="t">{what}</span></span><kbd>{key}</kbd></li>{/each}
    </ul>
  {/each}
</Dialog>


<Dialog bind:open={alert.open} kind="alert" title={alert.title} body={alert.body} buttons={alert.buttons} />

{#if toasts.length}
  {#key toasts[0].id}
    <div class="toast" role="status">
      <span class="tt">{toasts[0].text}</span>
      {#if toasts[0].action?.href}
        <a class="btn flat" href={toasts[0].action.href} aria-label={toasts[0].action.ariaLabel || toasts[0].action.label} onclick={dismissToast}>{toasts[0].action.label}</a>
      {:else if toasts[0].action}
        <button class="btn flat" onclick={() => { toasts[0].action.run(); dismissToast(); }}>{toasts[0].action.label}</button>
      {/if}
      <button class="btn flat icon round" aria-label="Dismiss" title="Dismiss" onclick={dismissToast}><Icon name="close" /></button>
    </div>
  {/key}
{/if}

</div>

{/if}

<style>
  /* Below any storage banner, the rest of #app's column: the window fills it. */
  .app-content { flex: 1; min-height: 0; }
  .banner { flex: none; }

  .toast { position: fixed; z-index: 40; left: 12px; right: 12px; bottom: calc(16px + env(safe-area-inset-bottom)); width: fit-content; margin-inline: auto; display: flex; align-items: center; gap: 2px; padding: 4px 4px 4px 16px; border-radius: 999px; background: var(--toast-bg); color: var(--toast-fg); box-shadow: var(--shadow); }
  .toast .tt { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; overflow: hidden; font-size: .9rem; line-height: 1.3; }
  .toast .btn { color: inherit; font-weight: 600; }
  @media (hover: hover) { .toast .btn:hover { background: rgba(128, 128, 128, .25); } }
  @media (prefers-reduced-motion: no-preference) {
    .toast { animation: rise .16s ease-out; }
    @keyframes rise { from { transform: translateY(8px); opacity: 0; } }
  }
  @media print { .toast { display: none !important; } }
</style>
