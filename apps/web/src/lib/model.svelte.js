// Reactive data only. Feature actions own mutations; persistence chooses saved keys.
// Pastel fills with dark text, like a paper planner.
export const COLORS = ['#cfe6fb', '#fbd7d7', '#fde6c3', '#d3f0da', '#fdf4bb', '#e5d9fb', '#ffd8c4', '#d8e1fb'];
export const EVENT_COLOR = '#e6e2dc';
// Account sync excludes Canvas credentials and feed URLs.
export const KEYS = ['inst', 'term', 'schedules', 'current', 'watches', 'reminders', 'onboarded', 'tasks', 'lms', 'account', 'config', 'dataOwner', 'prefExtras'];
export const IDLE_CAT = { status: 'idle', progress: 0, hash: '', attrs: [], count: 0 };
export const pref = (k, d) => { try { return localStorage.getItem(k) ?? d; } catch { return d; } };

export const app = $state({
  config: null, loading: true, online: true, editable: false, storageError: '', storageCleared: false,
  // Navigation. view: week (the planner), classes (editing the schedule's
  // classes: a mode on wide screens, a page on phones) or tasks (a page on
  // phones). selected: the section or event shown in the detail pane. pane:
  // the side pane is shown on wide screens (remembered per device).
  view: 'week', selected: '', selectedDate: '', pane: pref('pane', 'open') !== 'closed',
  // Tasks sit in a due row above the hours, or on the grid at their due time (remembered per device).
  taskTimes: pref('taskTimes', 'off') === 'on',
  editing: '', taskEditing: '', connect: '', // dialogs: event id | 'new'; task id | 'new'; '' | 'pick' | provider
  q: '', focusFind: 0, weekOffset: 0, weekStart: 'mon', // weeks run Monday through Sunday
  inst: '', term: '',
  // The term's catalog lives in a worker; this is all the page knows of it.
  cat: { ...IDLE_CAT },
  cache: {}, // sections the page has looked at, by id
  seats: {}, // live seat counts, by section id
  schedules: [], current: '', watches: [],
  reminders: { id: '', on: false }, onboarded: false,
  tasks: [], lms: { canvas: { base: '', token: '', synced: 0 }, feeds: [], map: {} },
  prefExtras: {}, // preference fields written by another client, uploaded back unchanged
  // Signed-in account ({ id, phone, device }) and the sync engine's state.
  dataOwner: '', account: null, streak: null, streakMissed: false, sync: { status: 'idle', at: 0, error: '' },
});
