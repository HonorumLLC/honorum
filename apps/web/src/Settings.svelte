<script>
  // Settings: a preferences dialog with four pages in a sidebar (General,
  // Account, Schedule, Assignments), each of boxed lists whose rows carry a
  // title and at most one control. On phones the pages are a list that says
  // where each stands. The dialog keeps one size on every page, so the pages
  // carry similar amounts: the calendar switch sits with the schedule's
  // classes rather than under General, and a page with nothing to list yet
  // (no account, no assignment source) is a placeholder with its one action.
  // Rows that open a view end in an arrow, links that leave the app in the
  // open mark, and actions that ask first in an ellipsis. No page's controls
  // affect another page. School and term live in the schedule menu while
  // editing classes, and appearance follows the system.
  // The header bar's primary menu opens this; the keyboard shortcuts are
  // reached from here too.
  import { tick } from 'svelte';
  import { app, schedule, section, toggleSection, openClasses, setTaskTimes, syncReminders, stopReminders, disconnectCanvas, removeFeed, canvasCourses, lmsConnected, signOut, deleteAccount, loadMe, removeDevice, docHistory, restoreVersion, findCanvasClasses, addFoundClasses, institution } from './lib/state.svelte.js';
  import { syncConflicts } from './lib/sync.js';
  import { google, loadGoogle, connectGoogle, setGoogle, disconnectGoogle } from './lib/google.svelte.js';
  import { toast, askAlert, dialogs } from './lib/ui.svelte.js';
  const canvas = $derived(!!(app.lms.canvas.token || app.lms.canvas.shared));
  const sources = $derived([canvas && 'Canvas', app.lms.feeds.length && `${app.lms.feeds.length} ${app.lms.feeds.length === 1 ? 'calendar' : 'calendars'}`].filter(Boolean).join(' and '));
  const pages = $derived([
    { id: 'general', label: 'General', icon: 'sliders', detail: 'Reminders, shortcuts and terms' },
    { id: 'account', label: 'Account', icon: 'person', detail: app.account?.phone || 'Not signed in' },
    { id: 'schedule', label: 'Schedule', icon: 'week', detail: 'Calendar and classes' },
    { id: 'assignments', label: 'Assignments', icon: 'tasks', detail: sources || 'Nothing connected' },
  ]);
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import CourseMatch from './CourseMatch.svelte';
  import CanvasClasses from './CanvasClasses.svelte';

  let busy = $state(false), courses = $state(false);
  let devices = $state(false), classes = $state(false);
  // Earlier versions of the open schedule, kept by the server for thirty days.
  let history = $state(null), historyError = $state('');
  async function openHistory() {
    const s = schedule();
    history = []; historyError = '';
    try { history = (await docHistory('s_' + s.id)).versions.filter((v) => v.body); }
    catch (e) { historyError = e.message; }
  }
  function restoreOld(v) {
    restoreVersion('schedule', $state.snapshot(v.body));
    history = null;
  }
  const versionLabel = (v) => new Date(v.updated || v.archived * 1000).toLocaleString();
  const currentSchedule = $derived(schedule());
  async function addClasses() {
    dialogs.settings = false;
    app.selected = '';
    // Remove the modal before the finder tries to focus its search entry.
    await tick();
    openClasses();
  }
  function removeClass(id) {
    toggleSection(id);
    if (app.selected === id) app.selected = '';
  }
  // Classes from Canvas, for a new term: found by the server, confirmed here.
  let fromCanvas = $state(null); // null closed | { busy, error, found }
  async function openFromCanvas() {
    fromCanvas = { busy: true, error: '', found: null };
    try { fromCanvas = { busy: false, error: '', found: await findCanvasClasses() }; }
    catch (e) { fromCanvas = { busy: false, error: e.message, found: null }; }
  }
  const foundTerm = $derived(institution()?.terms.find((t) => t.id === fromCanvas?.found?.term)?.name || 'this term');
  function addFromCanvas(picks) {
    addFoundClasses(fromCanvas.found.term, picks, fromCanvas.found.sections);
    fromCanvas = null;
  }
  let recovery = $state(null);
  const conflicts = $derived.by(() => {
    // Refresh the archive when sync starts or settles, including conflicts
    // received while Settings is already open.
    app.sync.status;
    return syncConflicts();
  });
  function restoreCopy(d) {
    if (!d.body) return;
    const body = $state.snapshot(d.body);
    body.id = crypto.randomUUID();
    if (d.type === 'schedule') { body.name += ' (recovered)'; body.primary = false; app.schedules.push(body); }
    else if (d.type === 'task') app.tasks.push(body);
    recovery = null;
  }
  const pushOK = $derived(!!app.config?.vapid && 'PushManager' in window);

  // Google Calendar, read when the Schedule page opens. A switch moves only
  // once the server agrees; turning a direction on the first time goes to
  // Google to ask for its permission and comes back here.
  const offersGoogle = $derived(!!app.config?.google_calendar);
  const g = $derived(google.status);
  let googleBusy = $state(false), googleError = $state('');
  $effect(() => {
    if (!dialogs.settings || dialogs.settingsPage !== 'schedule' || !offersGoogle || !app.account) return;
    googleError = '';
    loadGoogle().catch((e) => (googleError = e.message));
  });
  async function googleChange(patch) {
    googleBusy = true;
    try { await setGoogle(patch); }
    catch (e) { toast(e.message); loadGoogle().catch(() => {}); }
    googleBusy = false;
  }
  function googleSwitch(e, want) {
    const on = e.currentTarget.checked;
    e.currentTarget.checked = !on;
    if (!on && want === 'publish') return askAlert('Remove the Honorum Calendar?', 'It is deleted from Google Calendar', [{ label: 'Cancel' }, { label: 'Remove', style: 'destructive', action: () => googleChange({ publish: false }) }]);
    if (on && !g?.[want].granted) {
      googleBusy = true;
      return connectGoogle(want).catch((err) => { googleBusy = false; toast(err.message); });
    }
    googleChange({ [want]: on });
  }
  const disconnectGoogleAsk = () => askAlert('Disconnect Google Calendar?', 'The Honorum calendar is deleted from Google Calendar', [{ label: 'Cancel' }, { label: 'Disconnect', style: 'destructive', action: async () => {
    googleBusy = true;
    try { await disconnectGoogle(); } catch (e) { toast(e.message); }
    googleBusy = false;
  } }]);
  // Access removed at Google, or a permission cleared since: one consent
  // brings back every direction that is on.
  const googleLapsed = $derived(!!g?.connected && (!!g.error || (g.publish.on && !g.publish.granted) || (g.show.on && !g.show.granted)));
  function reconnectGoogle() {
    googleBusy = true;
    connectGoogle('reconnect').catch((err) => { googleBusy = false; toast(err.message); });
  }

  // Fetch device details only inside their menu, so Settings stays still.
  let me = $state(null), deviceError = $state(''), deviceRetry = $state(0);
  let removing = $state('');
  $effect(() => {
    const account = app.account;
    deviceRetry;
    me = null;
    deviceError = '';
    if (!account) { devices = false; return; }
    if (!devices) return;
    let active = true;
    loadMe().then((m) => { if (active && app.account === account) me = m; }).catch((e) => {
      if (active && app.account === account) deviceError = e.message;
    });
    return () => { active = false; };
  });
  const syncText = $derived({ pending: 'Waiting to sync', idle: 'Saved on this device', syncing: 'Syncing…', offline: 'Offline', error: 'Not synced' }[app.sync.status] || 'Synced');

  // Removing a device signs it out and cannot be undone, so it asks first.
  const confirmRemove = (d) => askAlert('Remove Device?', `${d.name} will be signed out`, [{ label: 'Cancel' }, { label: 'Remove', style: 'destructive', action: async () => {
    const account = app.account;
    removing = d.id;
    try {
      await removeDevice(d.id);
      if (app.account === account && me) me.devices = me.devices.filter((device) => device.id !== d.id);
    } catch (e) { toast(e.message); }
    finally { removing = ''; }
  } }]);
  const confirmDelete = () => askAlert('Delete Account?', 'Schedules, tasks and alerts are removed from the account and its devices. This device keeps its copy', [{ label: 'Cancel' }, { label: 'Delete Account', style: 'destructive', action: () => deleteAccount().catch((e) => toast(e.message)) }]);
  const disconnect = () => askAlert('Disconnect Canvas?', app.lms.canvas.shared ? 'Canvas will disconnect on every device. Existing assignments stay saved.' : 'Its assignments are removed from Tasks', [{ label: 'Cancel' }, { label: 'Disconnect', style: 'destructive', action: () => disconnectCanvas().catch(e => toast(e.message)) }]);

  async function toggleReminders(e) {
    busy = true;
    try {
      if (!e.target.checked) await stopReminders();
      else {
        if (!('PushManager' in window) || (await Notification.requestPermission()) !== 'granted') throw new Error('Reminders need notification permission');
        app.reminders.on = true;
        await syncReminders();
      }
    } catch (err) {
      app.reminders.on = false;
      toast(err.message);
    }
    busy = false;
  }
</script>

<Dialog bind:open={dialogs.settings} bind:page={dialogs.settingsPage} {pages} kind="plain" title="Settings" width="44rem">
  {#if dialogs.settingsPage === 'account'}
    {#if app.account}
      <div class="list">
        <div class="row"><span class="rt"><span class="t">{app.account.phone}</span></span><span class="value" title={app.sync.error || ''}>{syncText}</span></div>
        <div class="row"><span class="rt"><span class="t">Streak</span><span class="s">{app.streak ? `Days with no tasks overdue · Best: ${app.streak.longest} ${app.streak.longest === 1 ? 'day' : 'days'}` : 'Connect to update your streak'}</span></span><span class="value">{app.streak ? `${app.streak.current} ${app.streak.current === 1 ? 'day' : 'days'}` : 'Pending'}</span></div>
        {#if conflicts.length}
          <button type="button" class="row" onclick={() => (recovery = conflicts)}><span class="rt"><span class="t">Recover Conflicting Edits</span></span><span class="chev"><Icon name="next" /></span></button>
        {/if}
        <button type="button" class="row" onclick={() => (devices = true)}><span class="rt"><span class="t">Devices</span></span><span class="chev"><Icon name="next" /></span></button>
      </div>
      <div class="list next">
        <button type="button" class="row" onclick={() => signOut().catch((e) => toast(e.message))}><span class="rt"><span class="t">Sign Out</span></span></button>
        <button type="button" class="row destructive" onclick={confirmDelete}><span class="rt"><span class="t">Delete Account…</span></span></button>
      </div>
    {:else}
      <div class="status">
        <span class="glyph"><Icon name="person" /></span>
        <h4>Sync Across Devices</h4>
        <p>Sign in to keep schedules and tasks on every device</p>
        <div class="actions"><button type="button" class="btn pill suggested" onclick={() => (dialogs.account = true)} disabled={!app.config?.phone_auth} title={app.config?.phone_auth ? '' : 'This server cannot send sign-in codes'}>Sign In</button></div>
      </div>
    {/if}
  {:else if dialogs.settingsPage === 'schedule'}
    <h4 class="group-title">Calendar</h4>
    <div class="list">
      <label class="row"><span class="rt"><span class="t">Tasks at Their Due Time</span><span class="s wrap">On the hours when they are due, instead of a row above the week</span></span>
        <input type="checkbox" class="switch" checked={app.taskTimes} onchange={(e) => setTaskTimes(e.currentTarget.checked)}>
      </label>
    </div>

    {#if offersGoogle}
      <h4 class="group-title">Google Calendar</h4>
      {#if !app.account}
        <div class="list">
          <div class="row"><span class="rt"><span class="t">Google Calendar</span></span><button type="button" class="btn flat" onclick={() => (dialogs.account = true)} disabled={!app.config?.phone_auth}>Sign In</button></div>
        </div>
      {:else}
        <div class="list">
          {#if g?.connected}
            <div class="row"><span class="rt"><span class="t">{g.email || 'Google Account'}</span>{#if g.error}<span class="s wrap">{g.error}</span>{/if}</span><button type="button" class="btn flat" onclick={disconnectGoogleAsk} disabled={googleBusy}>Disconnect</button></div>
            {#if googleLapsed}
              <button type="button" class="row" onclick={reconnectGoogle} disabled={googleBusy}><span class="rt"><span class="t">Connect Again</span></span><span class="chev"><Icon name="open" /></span></button>
            {/if}
          {/if}
          <label class="row"><span class="rt"><span class="t">Add Classes to Google Calendar</span>{#if g?.publish.error}<span class="s wrap">{g.publish.error}</span>{/if}</span>
            <input type="checkbox" class="switch" checked={!!g?.publish.on} disabled={googleBusy || !g} onchange={(e) => googleSwitch(e, 'publish')}>
          </label>
          {#if g?.publish.on}
            <label class="row"><span class="rt"><span class="t">Include Due Dates</span></span>
              <input type="checkbox" class="switch" checked={g.publish.tasks} disabled={googleBusy} onchange={(e) => googleChange({ tasks: e.currentTarget.checked })}>
            </label>
          {/if}
          <label class="row"><span class="rt"><span class="t">Show Google Events</span></span>
            <input type="checkbox" class="switch" checked={!!g?.show.on} disabled={googleBusy || !g} onchange={(e) => googleSwitch(e, 'show')}>
          </label>
        </div>
        {#if googleError}<p class="text note" role="alert">{googleError}</p>{/if}
        {#if g?.show.on && g.show.calendars.length}
          <div class="list next" aria-label="Google calendars">
            {#each g.show.calendars as cal (cal.id)}
              <label class="row"><span class="dot" style:background={cal.color || 'var(--track)'}></span><span class="rt"><span class="t">{cal.name}</span>{#if cal.error}<span class="s wrap">{cal.error}</span>{/if}</span>
                <input type="checkbox" class="switch" checked={cal.on} disabled={googleBusy} onchange={(e) => googleChange({ calendars: { [cal.id]: e.currentTarget.checked } })} aria-label="Show {cal.name}">
              </label>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}

    <h4 class="group-title">Classes</h4>
    <div class="list">
      <button type="button" class="row" onclick={() => (classes = true)} disabled={!currentSchedule}><span class="rt"><span class="t">Manage Classes</span><span class="s">Add or remove classes in your schedule</span></span><span class="chev"><Icon name="next" /></span></button>
      {#if canvas}
        <button type="button" class="row" onclick={openFromCanvas}><span class="rt"><span class="t">Add Classes from Canvas</span></span><span class="chev"><Icon name="next" /></span></button>
      {/if}
      {#if app.account}
        <button type="button" class="row" onclick={openHistory} disabled={!currentSchedule}><span class="rt"><span class="t">Earlier Versions</span><span class="s">Restore this schedule as it was</span></span><span class="chev"><Icon name="next" /></span></button>
      {/if}
    </div>
  {:else if dialogs.settingsPage === 'assignments'}
    {#if lmsConnected()}
      <div class="list">
        {#if canvas}
          <button type="button" class="row" onclick={() => (app.connect = 'canvas')}><span class="rt"><span class="t">Reconnect Canvas</span><span class="s">Replace your access token</span></span><span class="chev"><Icon name="next" /></span></button>
          <div class="row"><span class="rt"><span class="t">Canvas</span>{#if app.lms.canvas.shared}<span class="s">On all your devices</span>{/if}</span><button type="button" class="btn flat" onclick={disconnect}>Disconnect</button></div>
        {/if}
        {#if app.account && app.lms.canvas.token && !app.lms.canvas.shared}
          <button type="button" class="row" onclick={() => { app.connect = 'canvas'; }}><span class="rt"><span class="t">Use Canvas on All Devices</span><span class="s">Reconnect to save an encrypted token with your account</span></span><span class="chev"><Icon name="next" /></span></button>
        {/if}
        {#each app.lms.feeds as f, i (f.id || f.url)}
          <div class="row"><span class="rt"><span class="t">{f.label || 'Calendar'}</span>{#if f.shared}<span class="s">{f.error || 'On all your devices'}</span>{/if}</span><button type="button" class="btn flat" onclick={() => removeFeed(i).catch((e) => toast(e.message))}>Remove</button></div>
        {/each}
        <button type="button" class="row" onclick={() => (app.connect = 'pick')}><span class="rt"><span class="t">Add Another Source</span></span><span class="chev"><Icon name="next" /></span></button>
      </div>
      {#if canvas && canvasCourses().length}
        <div class="list next">
          <button type="button" class="row" onclick={() => (courses = true)}><span class="rt"><span class="t">Canvas Courses</span><span class="s">Match each course to a class for its color</span></span><span class="chev"><Icon name="next" /></span></button>
        </div>
      {/if}
    {:else}
      <div class="status">
        <span class="glyph"><Icon name="tasks" /></span>
        <h4>Import Assignments</h4>
        <p>Bring due dates from Canvas or a calendar feed</p>
        <div class="actions"><button type="button" class="btn pill suggested" onclick={() => (app.connect = 'pick')}>Connect Canvas or a Calendar</button></div>
      </div>
    {/if}
  {:else}
    <h4 class="group-title">Notifications</h4>
    <div class="list">
      <label class="row"><span class="rt"><span class="t">Reminders</span><span class="s wrap">15 minutes before class and an hour before an assignment is due</span></span>
        <input type="checkbox" class="switch" checked={app.reminders.on} disabled={busy || !pushOK} onchange={toggleReminders}>
      </label>
    </div>

    <div class="list next">
      <button type="button" class="row" onclick={() => (dialogs.shortcuts = true)}><span class="rt"><span class="t">Keyboard Shortcuts</span></span><span class="chev"><Icon name="next" /></span></button>
      <a class="row" href="/legal.html" target="_blank" rel="noopener"><span class="rt"><span class="t">Privacy Policy and Terms</span></span><span class="chev"><Icon name="open" /></span></a>
    </div>
  {/if}
  <div class="foot"></div>
</Dialog>

<Dialog bind:open={devices} kind="plain" title="Devices" width="30rem">
  <p class="text">Devices signed in to your account. Removing a device signs it out.</p>
  {#if deviceError}
    <p class="text" role="alert">{deviceError}</p>
    <button type="button" class="btn" onclick={() => deviceRetry++}>Try Again</button>
  {:else if !me}
    <p class="text" role="status">Loading devices…</p>
  {:else}
    <div class="list">
      {#each me.devices as d (d.id)}
        <div class="row">
          <span class="rt"><span class="t wrap">{d.name}</span></span>
          {#if d.current}<span class="value">This device</span>{:else}<button type="button" class="btn flat" aria-label="Remove {d.name}" disabled={!!removing} onclick={() => confirmRemove(d)}>{removing === d.id ? 'Removing…' : 'Remove'}</button>{/if}
        </div>
      {:else}<p class="row">No devices found.</p>{/each}
    </div>
  {/if}
  <div class="foot"></div>
</Dialog>

<Dialog bind:open={classes} kind="plain" title="Manage Classes" width="30rem">
  <p class="text">{currentSchedule?.name || 'Current schedule'}</p>
  <div class="list">
    <button type="button" class="row" onclick={addClasses}><Icon name="plus" /><span class="rt"><span class="t">Add Classes</span></span><Icon name="next" /></button>
  </div>
  <h3 class="group-title">Scheduled Classes</h3>
  <div class="list">
    {#each currentSchedule?.sections || [] as id (id)}
      {@const sec = section(id)}
      {@const label = sec ? `${sec.course}-${sec.num}` : id}
      <div class="row">
        <span class="rt"><span class="t wrap">{label}</span>{#if sec?.title}<span class="s wrap">{sec.title}</span>{/if}</span>
        <button type="button" class="btn flat" aria-label="Remove {label}" onclick={() => removeClass(id)}>Remove</button>
      </div>
    {:else}<p class="row text">No classes in this schedule. Choose Add Classes to find a class.</p>{/each}
  </div>
  <div class="foot"></div>
</Dialog>

<Dialog open={courses} kind="plain" title="Canvas Courses" width="30rem" onclose={() => (courses = false)}>
  <p class="text">Match each Canvas course to a class so its assignments take the class’s color</p>
  <CourseMatch />
  <div class="foot"></div>
</Dialog>

<Dialog open={history !== null} kind="plain" title="Earlier Versions" onclose={() => (history = null)}>
  <p class="text">{currentSchedule?.name || 'This schedule'} as it was before each change. Restoring replaces the current version; the replaced one is kept too.</p>
  {#if historyError}<p class="text" role="alert">{historyError}</p>{/if}
  {#each history || [] as v (v.rev)}
    <div class="row"><span class="rt"><span class="t">{versionLabel(v)}</span><span class="s">{v.body.name} · {(v.body.sections || []).length} classes</span></span><button class="btn" onclick={() => restoreOld(v)}>Restore</button></div>
  {:else}{#if !historyError}<p class="text">No earlier versions yet.</p>{/if}{/each}
</Dialog>

<Dialog open={fromCanvas !== null} kind="plain" title="Classes from Canvas" width="30rem" onclose={() => (fromCanvas = null)}>
  {#if fromCanvas?.busy}
    <p class="text" role="status">Loading…</p>
  {:else if fromCanvas?.error}
    <p class="text" role="alert">{fromCanvas.error}</p>
  {:else if fromCanvas?.found?.classes.length}
    <h4 class="group-title">{foundTerm}</h4>
    <div class="found"><CanvasClasses found={fromCanvas.found} onadd={addFromCanvas} cancel="Cancel" oncancel={() => (fromCanvas = null)} /></div>
  {:else if fromCanvas?.found}
    <p class="text">No classes found</p>
  {/if}
</Dialog>

<Dialog open={recovery !== null} kind="plain" title="Conflicting Edits" onclose={() => (recovery = null)}>
  <p class="text">Your edits were kept. Restore another device’s version as a separate copy.</p>
  {#each recovery || [] as d, i (i)}
    <div class="row"><span class="rt">{d.body?.name || d.body?.title || d.type}</span>
      {#if d.body && ['schedule', 'task'].includes(d.type)}<button class="btn" onclick={() => restoreCopy(d)}>Restore Copy</button>{:else}<span class="dim">{d.deleted ? 'Deleted on another device' : 'Settings changed'}</span>{/if}
    </div>
  {:else}<p class="text">No conflicting edits.</p>{/each}
</Dialog>

<style>
  .foot { height: 12px; }
  /* A form dialog's actions sit at its end, as when connecting a source. */
  .found :global(.actions) { justify-content: flex-end; margin: 14px 0 12px; }
  .next { margin-top: 16px; }
  /* The placeholder page, sized for a dialog; its heading sits under the page's. */
  .status { padding: 8px 20px 20px; }
  .status .glyph { width: 60px; height: 60px; }
  .status .glyph :global(.i) { width: 28px; height: 28px; }
  .status h4 { margin: 14px 0 0; font-family: var(--display); font-weight: 500; font-size: 1.15rem; line-height: 1.25; }
  .text { margin-bottom: 12px; color: var(--text2); }
  .value { flex: none; color: var(--text2); }
  .note { margin: 8px 2px 0; }
  .dot { flex: none; width: 12px; height: 12px; border-radius: 50%; }
</style>
