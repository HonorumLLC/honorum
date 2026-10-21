// One tab writes shared IndexedDB state at a time. A handover flushes saves
// before the next tab restores state. Browser focus chooses the editing tab.
export function coordinateTabs({ acquired, releasing, changed }) {
  if (!navigator.locks) {
    const ready = Promise.resolve().then(acquired).then(() => changed(true));
    return { ready, close() {} };
  }
  let release, pending = false, stopped = false, relinquishing;
  let wanted = document.visibilityState === 'visible';
  let firstResolve, firstReject;
  const ready = new Promise((resolve, reject) => { firstResolve = resolve; firstReject = reject; });
  async function acquire() {
    if (pending || release || stopped || !wanted) return;
    pending = true;
    try {
      await navigator.locks.request('honorum-editor', async () => {
        if (stopped || !wanted) return;
        // Do not expose writable UI until restoration is complete.
        await acquired();
        const held = new Promise((resolve) => { release = resolve; });
        changed(true); firstResolve();
        if (!wanted || stopped) void relinquish();
        await held;
        release = undefined;
      });
    } catch (error) { wanted = false; firstReject(error); changed(false, error); }
    finally { pending = false; if (wanted && !stopped) void acquire(); }
  }
  async function relinquish() {
    if (!release || relinquishing) return relinquishing;
    changed(false);
    relinquishing = (async () => {
      try { await releasing(); release?.(); }
      catch (error) { wanted = false; changed(true, error); }
      finally { relinquishing = undefined; }
    })();
    return relinquishing;
  }
  const focus = () => { wanted = document.visibilityState === 'visible'; if (wanted) void acquire(); };
  const blur = () => { wanted = false; void relinquish(); };
  const visibility = () => document.visibilityState === 'hidden' ? blur() : focus();
  window.addEventListener('focus', focus);
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', visibility);
  changed(false); void acquire();
  return { ready, close() { stopped = true; window.removeEventListener('focus', focus); window.removeEventListener('blur', blur); document.removeEventListener('visibilitychange', visibility); void relinquish(); } };
}
