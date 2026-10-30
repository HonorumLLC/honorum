// The account streak, read from the server when the page is in the
// foreground or the account's tasks sync. The server counts days that ended
// with nothing overdue, so visits themselves count for nothing; the first
// check-in only fixes the account's time zone from its school. The last
// streak is kept on the device and shown until the server answers, and
// streakMissed records a check that failed with nothing to show.
export function createActivity({ app, request, storage, visible = () => document.visibilityState === 'visible' }) {
  let job;
  const store = () => storage ?? globalThis.localStorage;
  async function visit() {
    const account = app.account;
    if (!account) return;
    if (!app.streak) try { app.streak = JSON.parse(store().getItem('streak:' + account.id)) || app.streak; } catch {}
    if (!visible()) return;
    if (job?.account === account) return job.promise;
    const active = { account, promise: null };
    active.promise = request('/me/activity', { account: account.id, method: 'POST', body: JSON.stringify({ inst: app.inst }) })
      .then(streak => {
        if (app.account !== account) return;
        app.streak = streak; app.streakMissed = false;
        try {
          store().setItem('streak:' + account.id, JSON.stringify(streak));
          // Visits queued by earlier versions no longer count.
          store().removeItem('activity:' + account.id);
        } catch {}
      }).catch(() => { if (app.account === account && !app.streak) app.streakMissed = true; }).finally(() => { if (job === active) job = null; });
    job = active;
    await active.promise;
  }
  return { visit };
}
