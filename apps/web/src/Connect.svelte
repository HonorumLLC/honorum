<script>
  // Connecting an assignment source: pick a provider, follow its steps and
  // paste one thing, then match Canvas courses to classes.
  import { app, lmsOf, institution, connectCanvas, addFeed, canvasCourses, lmsSync } from './lib/state.svelte.js';
  import { PROVIDER_NAMES } from './lib/providers.js';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import CourseMatch from './CourseMatch.svelte';

  const PROVIDERS = {
    canvas: { how: 'Paste a token from your Canvas settings', steps: ['In Canvas, open Account, then Settings', 'Under Approved Integrations, choose New Access Token, then Generate Token', 'Copy the token and paste it here'] },
    classroom: { how: 'Paste the class calendar link from Google Calendar', steps: ['Open Google Calendar on a computer', 'Under My calendars, open the class calendar’s Settings and sharing', 'Under Integrate calendar, copy the Secret address in iCal format'] },
    blackboard: { how: 'Paste your calendar link from Blackboard', steps: ['In Blackboard, open Calendar', 'Open the calendar settings and choose Share calendar', 'Copy the link'] },
    brightspace: { how: 'Paste your calendar link from Brightspace', steps: ['In Brightspace, open Calendar and choose Settings', 'Turn on Enable Calendar Feeds, then choose Subscribe', 'Copy the URL'] },
    moodle: { how: 'Paste your calendar link from Moodle', steps: ['In Moodle, open Calendar', 'Choose Import or export calendars, then Export calendar', 'Pick All events and Get calendar URL, then copy it'] },
    ical: { title: 'Add a Calendar Link', cta: 'Add Calendar', how: 'Paste any iCal (.ics) link', steps: ['Find the share or export option in your calendar app', 'Copy the iCal link, the one ending in .ics or marked as a subscription address'] },
  };
  for (const k in PROVIDERS) PROVIDERS[k].name = PROVIDER_NAMES[k];

  let step = $state('form'); // form | done
  let busy = $state(false), error = $state('');
  let base = $state(''), token = $state(''), url = $state(''), label = $state('');

  const lms = $derived(lmsOf());
  const school = $derived(institution()?.name || 'your school');
  const provider = $derived(PROVIDERS[app.connect] ? app.connect : '');
  const p = $derived(PROVIDERS[provider]);
  const isCanvas = $derived(provider === 'canvas');
  // The school's own LMS goes first.
  const order = $derived([...(PROVIDERS[lms?.kind] ? [lms.kind] : []), ...Object.keys(PROVIDERS).filter((k) => k !== lms?.kind)]);
  const tokenPage = $derived(isCanvas && /^https:\/\//.test(base.trim()) ? base.trim().replace(/\/+$/, '') + '/profile/settings' : '');
  const title = $derived(!provider ? 'Connect Assignments' : step === 'done' ? (isCanvas ? 'Canvas Connected' : 'Calendar Added') : p.title || `Connect ${p.name}`);

  $effect(() => {
    provider; // entering a provider resets the form; Canvas is prefilled with the school's address
    step = 'form'; error = ''; token = ''; url = ''; label = '';
    base = isCanvas ? app.lms.canvas.base || (lms?.kind === 'canvas' ? lms.url : '') : '';
  });
  const close = () => (app.connect = '');

  async function submit(e) {
    e.preventDefault();
    error = '';
    if (isCanvas) {
      if (!/^https:\/\/\S+\.\S+/.test(base.trim())) return (error = 'Enter your Canvas address, starting with https://');
      if (token.trim().length < 20) return (error = 'That does not look like a Canvas token. Tokens are long strings of letters and numbers');
    } else if (!/^https:\/\/\S+/.test(url.trim())) {
      return (error = 'Enter a link starting with https://');
    }
    busy = true;
    try {
      isCanvas ? await connectCanvas(base.trim(), token.trim()) : await addFeed(url.trim(), label);
      token = '';
      step = 'done';
    } catch (err) {
      error = friendly(err.message);
    }
    busy = false;
  }
  function friendly(m) {
    if (/rejected the token/i.test(m)) return isCanvas ? 'Canvas rejected that token. Generate a new one and try again' : 'That link was refused. Copy it again from the source, it may have changed';
    if (/host not found|bad url|only https/i.test(m)) return 'That address could not be reached. Check it for typos';
    return m;
  }
</script>

<Dialog open={!!app.connect} kind="plain" {title} width="27rem" onclose={close}>
  {#if !provider}
    <p class="text">Pick the site your classes post to. You will need to copy one thing from it</p>
    <div class="list">
      {#each order as k (k)}
        <button class="row" onclick={() => (app.connect = k)}>
          <span class="mark {k}" aria-hidden="true"></span>
          <span class="rt"><span class="t">{PROVIDERS[k].name}{#if lms?.kind === k}<em>Used at {school}</em>{/if}</span></span>
          <span class="chev"><Icon name="next" /></span>
        </button>
      {/each}
    </div>
  {:else if step === 'done'}
    <div class="status compact">
      <div class="glyph"><Icon name="check" /></div>
      {#if isCanvas && lmsSync.owner === app.lms && lmsSync.phase === 'completion'}<p role="status">Your assignments are ready. Completion is still updating.</p>{/if}
      {#if isCanvas && lmsSync.owner === app.lms && lmsSync.error}<p role="status">Assignments imported. {lmsSync.error}</p>{/if}
      {#if isCanvas && canvasCourses().length}<p>Match each Canvas course to a class so its assignments take the class’s color</p>{/if}
    </div>
    {#if isCanvas && canvasCourses().length}<CourseMatch />{/if}
    <div class="foot">
      <button type="button" class="btn pill" onclick={() => (app.connect = 'pick')}>Add Another</button>
      <button type="button" class="btn pill suggested" onclick={close}>Done</button>
    </div>
  {:else}
    <form onsubmit={submit}>
      <ol class="steps">{#each p.steps as s, i (i)}<li><span>{i + 1}</span>{s}</li>{/each}</ol>
      {#if isCanvas}
        <label class="field"><span>Canvas address</span><input type="url" bind:value={base} placeholder="https://canvas.yourschool.edu" autocomplete="off" spellcheck="false"></label>
        {#if tokenPage}<a class="open" href={tokenPage} target="_blank" rel="noopener">Open the Canvas settings page <Icon name="open" size={13} /></a>{/if}
        <p class="text">{app.account ? 'Connecting saves an encrypted Canvas token with your account so all your devices can refresh assignments. Disconnecting stops access on every device.' : 'Your Canvas token stays on this device. Sign in to connect Canvas across devices.'}</p>
        <label class="field"><span>Access token</span><input type="password" bind:value={token} placeholder="Paste the token" autocomplete="off"></label>
      {:else}
        <label class="field"><span>Calendar link</span><input type="url" bind:value={url} placeholder="https://… .ics" autocomplete="off" spellcheck="false"></label>
        <p class="text">{app.account ? 'The link is saved encrypted with your account, so every device gets these assignments and they stay current while the app is closed.' : 'The link stays on this device. Sign in to share it across devices.'}</p>
        <label class="field"><span>Class <span class="hint">optional, so items get the right color</span></span><input bind:value={label} placeholder="For example CS 1213"></label>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="foot">
        <button type="button" class="btn pill" onclick={close}>Cancel</button>
        <button class="btn pill suggested" disabled={busy}>{busy ? (isCanvas ? 'Connecting…' : 'Adding…') : p.cta || (isCanvas ? 'Connect Canvas' : 'Add Calendar')}</button>
      </div>
    </form>
  {/if}
</Dialog>

<style>
  .text { margin-bottom: 12px; color: var(--text2); }
  .mark { flex: none; width: 12px; height: 12px; border-radius: 50%; background: var(--muted); }
  .mark.canvas { background: #e4573d; } .mark.classroom { background: #34a853; } .mark.blackboard { background: #3b3b3b; } .mark.brightspace { background: #f2a51a; } .mark.moodle { background: #f98012; } .mark.ical { background: var(--accent); }
  .rt em { margin-left: 8px; font-style: normal; color: var(--accent-fg); font-size: .8rem; }
  .steps { margin: 0 0 14px; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 6px; color: var(--text2); font-size: .9rem; }
  .steps li { display: flex; gap: 8px; align-items: flex-start; }
  .steps span { flex: none; display: inline-flex; align-items: center; justify-content: center; width: 20px; height: 20px; border-radius: 50%; background: var(--tint); color: var(--accent-fg); font-size: .75rem; font-weight: 600; }
  .open { display: inline-flex; align-items: center; gap: 4px; margin: -4px 0 12px; font-size: .85rem; text-decoration: none; }
  .error { margin-bottom: 10px; padding: 8px 10px; border-radius: 8px; background: var(--err-bg); color: var(--err-fg); font-size: .88rem; }
  .foot { display: flex; justify-content: flex-end; gap: 8px; margin: 6px 0 12px; }
  .status.compact { padding: 8px 0 16px; }
  .status.compact + :global(.list) { margin-bottom: 8px; }
</style>
