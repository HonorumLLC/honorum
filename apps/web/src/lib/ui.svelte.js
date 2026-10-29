import { emptyFilters } from './filters.js';

// Transient UI state any view may touch: the toast queue, the alert dialog
// and the app-level dialogs. App.svelte renders them.
export const toasts = $state([]);
let timer;

// action: { label, run } adds a button, usually Undo; { label, href } adds a link.
export function toast(text, action = null) {
  toasts.push({ id: Math.random().toString(36).slice(2), text, action });
  if (toasts.length === 1) arm();
}
function arm() {
  clearTimeout(timer);
  if (toasts.length) timer = setTimeout(dismissToast, toasts[0].action ? 8000 : 4500);
}
export function dismissToast() {
  toasts.shift();
  arm();
}

// One alert dialog at a time: heading, one line of body, up to three buttons
// ({ label, style: 'suggested' | 'destructive', action }).
export const alert = $state({ open: false, title: '', body: '', buttons: [] });
export const askAlert = (title, body, buttons) => Object.assign(alert, { open: true, title, body, buttons });

// Dialogs opened from menus and shortcuts. Settings remembers its page for the
// session; none chosen is the first page in a window and the list of pages on a phone.
export const dialogs = $state({ settings: false, settingsPage: '', shortcuts: false, account: false });
export const SETTINGS_PAGES = ['general', 'account', 'schedule', 'assignments'];

// The class finder's controls: whether the filter list is shown, and the
// filters themselves. They last the session, so the finder is as it was left.
export const finder = $state({ filters: false, f: emptyFilters() });

// How the task list is shown: one class or all, the order within each due
// group, and whether completed tasks are listed. Kept for the session so
// opening details or hiding the pane does not reset it.
export const taskView = $state({ filter: '', sort: 'due', showDone: false });
