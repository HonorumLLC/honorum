// The account streak, read from the server when the page is in the
// foreground or the account's tasks sync. The server counts days that ended
// with nothing overdue, so visits themselves count for nothing; the first
// check-in only fixes the account's time zone from its school.
export function createActivity({ app, request, storage, visible = () => document.visibilityState === 'visible' }) {
  let job;
  async function visit() {
    const account = app.account;
    if (!account || !visible()) return;
    if (job?.account === account) return job.promise;
    const active = { account, promise: null };
    active.promise = request('/me/activity', { account: account.id, method: 'POST', body: JSON.stringify({ inst: app.inst }) })
      .then(streak => {
        if (app.account !== account) return;
        app.streak = streak;
        // Visits queued by earlier versions no longer count.
        try { (storage ?? globalThis.localStorage).removeItem('activity:' + account.id); } catch {}
      }).catch(() => {}).finally(() => { if (job === active) job = null; });
    job = active;
    await active.promise;
  }
  return { visit };
}
