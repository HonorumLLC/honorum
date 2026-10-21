<script>
  // The first run, as a short setup assistant that ends with the student's
  // classes on the week: the school, the classes or their assignments, and
  // whether to sign in. At a Canvas school the classes come from Canvas: one
  // token, and Honorum finds the classes and their assignments together for
  // the student to confirm. Elsewhere, assignments come from the school's
  // calendar link and the last page opens the class search. Sign-in is offered
  // from the first page too: an account that has planned before brings the
  // finished tour with its preferences, and the planner appears as soon as
  // they arrive.
  //
  // Every page has one frame, so nothing moves between them: Back and the
  // progress at the top, then the glyph and title, then content that starts
  // at the top and scrolls, then a footer whose main button never moves. The
  // way out (Skip, Not Now) has one place beside it, kept empty when a page
  // has none. In a window the frame is a card of fixed size.
  import { app, selectTerm, warmCatalog, lmsOf, lmsConnected, connectCanvas, findCanvasClasses, addFoundClasses, openClasses, institution } from './lib/state.svelte.js';
  import { dialogs } from './lib/ui.svelte.js';
  import { PROVIDER_NAMES } from './lib/providers.js';
  import { MARK } from './wordmark.js';
  import Icon from './Icon.svelte';
  import CanvasClasses from './CanvasClasses.svelte';

  const schools = $derived(app.config?.institutions || []);
  const canSignIn = $derived(!!app.config?.phone_auth && !app.account);
  const lms = $derived(lmsOf());
  const canvasSchool = $derived(lms?.kind === 'canvas');
  const lmsName = $derived(PROVIDER_NAMES[lms?.kind] || '');
  // Signing in on the first page skips the account page; signing in from
  // the account page moves on from it.
  let askedAccount = $state(false);
  // The account comes after the classes or assignments, once there is
  // something to keep. The class search leaves the tour, so it comes last.
  const pages = $derived(['school', canvasSchool ? 'classes' : 'assignments', ...(app.config?.phone_auth && (!app.account || askedAccount) ? ['account'] : []), ...(canvasSchool ? [] : ['search'])]);
  let at = $state(0);
  const page = $derived(pages[Math.min(at, pages.length - 1)]);
  const last = $derived(at + 1 >= pages.length);
  // Pages fade in as the tour moves. The first is simply there, so the
  // app's first paint is not held back by a fade.
  let turned = $state(false);
  const turn = (by) => { turned = true; at += by; };
  const finish = () => (app.onboarded = true);
  const next = () => (last ? finish() : turn(1));

  // A school is chosen with a tap and confirmed with Continue, like every
  // other page. With one school there is nothing to choose.
  let pick = $state(''), picked = $state('');
  const pickable = $derived(schools.filter((i) => i.name.toLowerCase().includes(pick.toLowerCase())));
  function choose(i) {
    picked = i.id;
    selectTerm(i.id, '');
    warmCatalog();
  }
  // The class search takes over once the tour is done.
  function search() {
    finish();
    openClasses();
  }
  const connect = () => (app.connect = lmsName ? lms.kind : 'pick');
  const signIn = () => { if (page === 'account') askedAccount = true; dialogs.account = true; };
  // A connected calendar ends the assignments page and an account the
  // account page.
  $effect(() => { if (page === 'assignments' && lmsConnected()) next(); });
  $effect(() => { if (page === 'account' && app.account) next(); });

  // Classes from Canvas: ask for the token, find, confirm.
  let step = $state('ask'); // ask | finding | review | none
  let token = $state(''), address = $state(''), error = $state('');
  let found = $state(null), classes = $state(), count = $state(0);
  const uid = $props.id();
  const canvasReady = $derived(!!(app.lms.canvas.token || app.lms.canvas.shared));
  const base = $derived((address.trim() || app.lms.canvas.base || lms?.url || '').replace(/\/+$/, ''));
  const foundTerm = $derived(institution()?.terms.find((t) => t.id === found?.term)?.name || 'This Term');
  async function findClasses() {
    error = '';
    if (!canvasReady) {
      if (!/^https:\/\/\S+\.\S+/.test(base)) return (error = 'Enter a Canvas address starting with https://');
      if (token.trim().length < 20) return (error = 'That does not look like a Canvas token');
    }
    step = 'finding';
    try {
      if (!canvasReady) await connectCanvas(base, token.trim());
      token = '';
      found = await findCanvasClasses();
      step = found.classes.length ? 'review' : 'none';
    } catch (err) {
      step = 'ask';
      error = /rejected the token/i.test(err.message) ? 'Canvas rejected that token'
        : /host not found|bad url|only https/i.test(err.message) ? 'That Canvas address could not be reached'
        : err.message;
    }
  }
  function add(picks) {
    addFoundClasses(found.term, picks, found.sections);
    step = 'ask';
    next();
  }
  function back() {
    error = '';
    if (page === 'classes' && step !== 'ask') step = 'ask';
    else turn(-1);
  }

  // Each page's glyph, title, main button and way out. The main button is
  // [label, action, disabled]; Return in a field presses it.
  const view = $derived.by(() => {
    const skip = ['Skip', next];
    if (page === 'school') return { title: 'Welcome to Honorum', primary: ['Continue', next, schools.length > 1 && !picked], secondary: canSignIn && ['Sign In', signIn] };
    if (page === 'assignments') return { glyph: 'tasks', title: 'Import Assignments', primary: [lmsName ? `Connect ${lmsName}` : 'Connect Assignments', connect], secondary: skip };
    if (page === 'account') return { glyph: 'person', title: 'Sync Across Devices', primary: ['Sign In', signIn], secondary: ['Not Now', next] };
    if (page === 'search') return { glyph: 'book', title: 'Add Your Classes', primary: ['Search Classes', search], secondary: ['Skip', finish] };
    if (step === 'finding') return { glyph: 'book', title: 'Add Your Classes', primary: ['Finding…', null, true], secondary: ['Skip', null, true], busy: true };
    if (step === 'review') return { glyph: 'book', title: 'Add Your Classes', primary: [count ? `Add ${count} ${count === 1 ? 'Class' : 'Classes'}` : 'Add Classes', () => classes.add(), !count], secondary: skip };
    if (step === 'none') return { glyph: 'book', title: 'No Classes Found', primary: ['Continue', next] };
    return { glyph: 'book', title: 'Add Your Classes', primary: ['Find Classes', findClasses], secondary: skip };
  });
  // The footer gets a hairline only while content scrolls beneath it.
  let edge = $state(false);
  function edges(el) {
    const measure = () => (edge = el.scrollHeight - el.scrollTop - el.clientHeight > 1);
    const watch = new ResizeObserver(measure);
    watch.observe(el);
    for (const child of el.children) watch.observe(child);
    el.addEventListener('scroll', measure, { passive: true });
    return () => { watch.disconnect(); el.removeEventListener('scroll', measure); };
  }
  function submit(e) {
    e.preventDefault();
    if (!view.primary[2]) view.primary[1]();
  }
</script>

<section class="welcome" aria-label="Welcome">
  <form class="card" onsubmit={submit}>
    <div class="bar">
      {#if at > 0}
        <button type="button" class="btn flat icon" onclick={back} disabled={view.busy} title="Back" aria-label="Back"><Icon name="back" /></button>
        <div class="steps" role="progressbar" aria-label="Setup" aria-valuemin="1" aria-valuemax={pages.length - 1} aria-valuenow={at}>
          {#each pages.slice(1) as p, i (p)}<span class:on={i < at}></span>{/each}
        </div>
      {/if}
    </div>
    {#key page}
      <header class="head" class:turned>
        {#if view.glyph}<div class="glyph"><Icon name={view.glyph} /></div>
        {:else}<svg class="mark" viewBox={MARK.viewBox} aria-hidden="true"><path d={MARK.d} fill="var(--accent)" fill-rule="evenodd" /></svg>{/if}
        <h2>{view.title}</h2>
      </header>
    {/key}
    {#if page === 'school' && schools.length > 1}
      <div class="tools">
        <!-- svelte-ignore a11y_autofocus -->
        <label class="entry"><Icon name="search" /><input type="search" placeholder="Search schools…" bind:value={pick} autofocus aria-label="Search schools"></label>
      </div>
    {/if}
    {#key page + step}
      <div class="body" class:turned {@attach edges}>
        {#if page === 'school' && schools.length > 1}
          <div class="list" role="group" aria-label="School">
            {#each pickable as i (i.id)}
              <button type="button" class="row" aria-pressed={picked === i.id} onclick={() => choose(i)}>
                <span class="rt"><span class="t">{i.name}</span></span>
                {#if picked === i.id}<span class="tick"><Icon name="check" /></span>{/if}
              </button>
            {:else}
              <p class="row dim">No schools match</p>
            {/each}
          </div>
        {:else if page === 'classes' && step === 'ask'}
          {#if !canvasReady}
            {#if !lms?.url}
              <label class="field"><span>Canvas address</span><input type="url" bind:value={address} placeholder="https://canvas.yourschool.edu" autocomplete="off" spellcheck="false"></label>
            {/if}
            <div class="field">
              <span><label for="{uid}-token">Canvas access token</label>{#if base}<a class="open" href="{base}/profile/settings" target="_blank" rel="noopener">Get a Token <Icon name="open" size={13} /></a>{/if}</span>
              <input id="{uid}-token" type="password" bind:value={token} autocomplete="off">
              {#if error}<p class="error" role="alert">{error}</p>{/if}
            </div>
            <section class="token-help" aria-labelledby="{uid}-token-help">
              <h3 id="{uid}-token-help">How to get your token</h3>
              <ol class="guide token-steps">
                <li><span class="marker" aria-hidden="true">1</span><p>In Canvas, open <strong>Account</strong>, then <strong>Settings</strong>.</p></li>
                <li><span class="marker" aria-hidden="true">2</span><p>Under <strong>Approved Integrations</strong>, choose <strong>New Access Token</strong>.</p></li>
                <li><span class="marker" aria-hidden="true">3</span><p>Enter <strong>Honorum</strong> as the purpose and choose an expiry date.</p></li>
                <li><span class="marker" aria-hidden="true">4</span><p>Select <strong>Generate Token</strong>, then copy and paste it above.</p></li>
              </ol>
            </section>
          {:else if error}<p class="error" role="alert">{error}</p>{/if}
        {:else if page === 'classes' && step === 'finding'}
          <!-- Rows of the list's shape hold its place while Canvas answers. -->
          <p class="label">&nbsp;</p>
          <div class="list" role="status" aria-label="Finding your classes">
            {#each [0, 1, 2, 3] as n (n)}<div class="row bones"><span class="rt"><i class="bone"></i><i class="bone short"></i></span><i class="bone switch-bone"></i></div>{/each}
          </div>
        {:else if page === 'classes' && step === 'review'}
          <p class="label">{foundTerm}</p>
          <CanvasClasses {found} onadd={add} actions={false} bind:count bind:this={classes} />
        {:else if page === 'account'}
          <p class="sync-intro">Use the same phone number to sign in on every device.</p>
          <ul class="guide sync-points">
            <li><span class="marker" aria-hidden="true"><Icon name="sync" /></span><div><h3>Schedules and tasks together</h3><p>New plans, edits, and completed tasks sync automatically.</p></div></li>
            <li><span class="marker" aria-hidden="true"><Icon name="edit" /></span><div><h3>Keep planning offline</h3><p>Your changes are saved here and sync when you're back online.</p></div></li>
            <li><span class="marker" aria-hidden="true"><Icon name="person" /></span><div><h3>An account is optional</h3><p>Skip sign-in to keep planning on this device.</p></div></li>
          </ul>
        {/if}
      </div>
    {/key}
    <div class="foot" class:edge>
      <button type="submit" class="btn pill suggested" disabled={view.primary[2]}>{#if view.busy}<span class="spin"><Icon name="sync" /></span>{/if}{view.primary[0]}</button>
      {#if view.secondary}
        <button type="button" class="btn pill quiet" disabled={view.secondary[2]} onclick={view.secondary[1]}>{view.secondary[0]}</button>
      {:else}
        <span class="btn pill quiet" aria-hidden="true"></span>
      {/if}
    </div>
  </form>
</section>

<style>
  /* One side margin (--m) for the head, the search, the content and the
     footer, with the back arrow's stroke on it. Vertically: 8 under the top
     bar, 12 from glyph to title, 20 from title to content, 12 from search to
     list, 6 from a label to its control, 16 between fields. */
  .welcome { --m: 20px; --arrow: 12px; position: fixed; inset: 0; z-index: 20; display: flex; padding: env(safe-area-inset-top) env(safe-area-inset-right) 0 env(safe-area-inset-left); background: var(--bg); }
  /* Where the back button grows to 44px for touch, its arrow sits further in. */
  @media (pointer: coarse) { .welcome { --arrow: 17px; } }
  .card { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .bar { flex: none; display: grid; grid-template-columns: 44px 1fr 44px; align-items: center; height: 48px; padding: 0 calc(var(--m) - var(--arrow)); }
  .steps { display: flex; justify-content: center; gap: 6px; }
  .steps span { width: 28px; height: 4px; border-radius: 2px; background: var(--track); transition: background-color .2s; }
  .steps .on { background: var(--accent); }
  .head { flex: none; display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 8px var(--m) 0; text-align: center; }
  .mark, .glyph { width: 56px; height: 56px; }
  .glyph { display: flex; align-items: center; justify-content: center; border-radius: 50%; background: var(--tint); color: var(--accent-fg); }
  .glyph :global(.i) { width: 28px; height: 28px; stroke-width: 1.5; }
  .head h2 { font-family: var(--display); font-weight: 500; font-size: 1.3rem; line-height: 1.25; }
  .tools { flex: none; padding: 20px var(--m) 0; }
  .entry { width: 100%; height: 40px; background: var(--hover); font-size: 1rem; }
  .body { flex: 1; min-height: 0; overflow-y: auto; padding: 20px var(--m) 16px; }
  .tools + .body { padding-top: 12px; }
  .body > :global(:last-child) { margin-bottom: 0; }
  .body :global(.field) { margin-bottom: 16px; }
  .body :global(.field > span) { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin-bottom: 6px; }
  .body :global(.found .row), .bones { min-height: 56px; }
  .label { margin: 0 0 6px; font-size: .8rem; font-weight: 600; color: var(--muted); }
  .tick { display: flex; color: var(--accent-fg); }
  .open { display: inline-flex; align-items: center; gap: 4px; font-size: .8rem; font-weight: 500; text-decoration: none; }
  .error { padding: 8px 10px; border-radius: 8px; background: var(--err-bg); color: var(--err-fg); font-size: .88rem; }
  .field .error { display: block; margin-top: 8px; }
  .token-help h3, .sync-points h3 { font-size: .9rem; font-weight: 600; color: var(--text); }
  .guide { display: grid; gap: 14px; margin: 12px 0 0; padding: 0; list-style: none; }
  .guide li { display: grid; grid-template-columns: 26px 1fr; align-items: start; gap: 10px; }
  .marker { display: flex; align-items: center; justify-content: center; width: 26px; height: 26px; border-radius: 50%; background: var(--tint); color: var(--accent-fg); font-size: .8rem; font-weight: 600; }
  .guide p { margin: 2px 0 0; color: var(--text2); font-size: .9rem; line-height: 1.5; }
  .guide strong { color: var(--text); font-weight: 600; }
  .sync-intro { color: var(--text2); font-size: .95rem; line-height: 1.5; }
  .sync-points { gap: 20px; margin-top: 24px; }
  .bone { display: block; height: 10px; border-radius: 5px; background: var(--hover); }
  .bone.short { width: 55%; height: 8px; margin-top: 6px; }
  .switch-bone { flex: none; width: 40px; height: 24px; border-radius: 999px; }
  .foot { flex: none; display: flex; flex-direction: column; gap: 4px; padding: 12px var(--m) calc(16px + env(safe-area-inset-bottom)); transition: box-shadow .15s; }
  .foot.edge { box-shadow: 0 -1px 0 var(--line); }
  /* A height, not a minimum, so the touch rule's 44px minimum leaves it be. */
  .foot .btn { width: 100%; height: 48px; padding: 0 16px; font-size: 1rem; }
  .foot .quiet { height: auto; min-height: 40px; font-size: .95rem; }
  .quiet { background: none; color: var(--accent-fg); }
  span.quiet { visibility: hidden; }
  .spin { display: flex; }
  /* In a window the frame is a card of one size. The verb ends the footer and
     the way out starts it, a plain button whose edge meets the margin. */
  @media (min-width: 601px) {
    .welcome { --m: 24px; align-items: center; justify-content: center; padding: 24px; }
    .card { flex: none; width: 30rem; height: min(36.25rem, 100dvh - 48px); border-radius: 14px; background: var(--view); box-shadow: 0 0 0 1px var(--line), var(--shadow); overflow: hidden; }
    .foot { flex-direction: row-reverse; gap: 8px; padding: 16px var(--m) var(--m); }
    .foot .btn { width: auto; height: auto; min-height: 36px; font-size: inherit; }
    .foot .suggested { min-width: 8.5rem; }
    .foot .quiet { margin-right: auto; background: var(--hover); color: var(--text); font-size: inherit; }
  }
  /* A phone on its side scrolls the whole frame rather than squeeze the content. */
  @media (max-height: 480px) {
    .card { overflow-y: auto; }
    .body { flex: none; overflow: visible; }
  }
  @media (prefers-reduced-motion: no-preference) {
    .turned { animation: enter .18s ease-out; }
    @keyframes enter { from { opacity: 0; transform: translateY(6px); } }
    .spin :global(.i) { animation: spin 1s linear infinite; }
    @keyframes spin { to { transform: rotate(360deg); } }
    .bone { animation: pulse 1.2s ease-in-out infinite; }
    @keyframes pulse { 50% { opacity: .45; } }
  }
</style>
