import { test, expect, type Page } from '@playwright/test';
import { open, settle, openSettings } from './helpers';

const canvas = { base: 'https://canvas.example.edu', token: 'test-canvas-token-with-enough-characters', synced: 0 };
const assignment = (id: string, title: string, done = false) => ({
  uid: `canvas:assignment:${id}`, title, done, due: '', source: 'canvas',
  course: 'Programming', courseCode: 'CS 101', courseId: '101', url: '', reason: done ? 'marked' : '',
});

// A real streaming Response lets the browser consume the planner snapshot
// while completion checks are still pending. Playwright's route.fulfill
// buffers the entire body, so it cannot exercise this boundary.
async function mockCanvas(page: Page) {
  await page.addInitScript(() => {
    const nativeFetch = window.fetch.bind(window);
    const streams: { controller: ReadableStreamDefaultController<Uint8Array>; canceled: boolean }[] = [];
    const mock = {
      imports: [] as { accept: string; body: unknown }[],
      completions: [] as unknown[],
      feeds: 0,
      send(index: number, value: unknown, close = false) {
        const stream = streams[index];
        if (stream.canceled) return;
        if (value !== null) stream.controller.enqueue(new TextEncoder().encode(JSON.stringify(value) + '\n'));
        if (close) stream.controller.close();
      },
    };
    Object.assign(window, { canvasTest: mock });
    window.fetch = async (input, init) => {
      const path = new URL(input instanceof Request ? input.url : String(input), location.href).pathname;
      if (path === '/api/lms/canvas/complete') {
        mock.completions.push(JSON.parse(String(init?.body)));
        return Response.json({ overrideId: 'override-test', module: true });
      }
      if (path === '/api/lms/feed') {
        mock.feeds++;
        return Response.json({ error: 'This unrelated calendar is unavailable' }, { status: 503 });
      }
      if (path !== '/api/lms/canvas') return nativeFetch(input, init);
      mock.imports.push({ accept: new Headers(init?.headers).get('Accept') || '', body: JSON.parse(String(init?.body)) });
      const index = streams.length;
      const body = new ReadableStream<Uint8Array>({
        start(controller) { streams.push({ controller, canceled: false }); },
        cancel() { streams[index].canceled = true; },
      });
      return new Response(body, { headers: { 'Content-Type': 'application/x-ndjson' } });
    };
  });
}

async function savedState(page: Page) {
  return page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open('honorum');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const state: any = await new Promise((resolve, reject) => {
      const request = db.transaction('kv').objectStore('kv').get('state:guest');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    db.close();
    return state;
  });
}

async function seed(page: Page, tasks: ReturnType<typeof assignment>[], connected = true, feeds: { url: string; label: string }[] = [], synced = Date.now()) {
  await expect.poll(() => savedState(page)).toBeTruthy();
  await page.evaluate(async ({ tasks, connected, canvas, feeds, synced }) => {
    const db = await new Promise<IDBDatabase>(resolve => {
      const request = indexedDB.open('honorum');
      request.onsuccess = () => resolve(request.result);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
      const request = store.get('state:guest');
      request.onsuccess = () => {
        const state = request.result;
        state.tasks = tasks.map((task, index) => ({ id: `seed-${index}`, ...task }));
        state.lms = {
          canvas: connected ? { ...canvas, synced } : { base: '', token: '', synced: 0 },
          feeds, feedSynced: synced, map: {},
        };
        store.put(state, 'state:guest');
      };
    });
    db.close();
  }, { tasks, connected, canvas, feeds, synced });
  await page.reload();
  await settle(page);
}

const imported = (page: Page) => page.evaluate(() => (window as any).canvasTest.imports);
const send = (page: Page, value: unknown, close = false, index = 0) => page.evaluate(
  ({ value, close, index }) => (window as any).canvasTest.send(index, value, close), { value, close, index },
);
const taskRow = (page: Page, title: string) => page.locator('.tasks li.row').filter({ has: page.getByText(title, { exact: true }) });

async function syncNow(page: Page) {
  await page.getByRole('button', { name: 'Sync assignments', exact: true }).click();
  await expect.poll(async () => (await imported(page)).length).toBe(1);
}

test('Canvas connection opens assignments before completion checks and skips unrelated calendars', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  await seed(page, [], false, [{ url: 'https://calendar.example.edu/broken.ics', label: 'Existing calendar' }]);
  await openSettings(page, 'Assignments');
  await page.getByRole('button', { name: 'Add Another Source', exact: true }).click();
  await page.getByRole('dialog', { name: 'Connect Assignments', exact: true }).getByRole('button', { name: 'Canvas', exact: true }).click();
  const connect = page.getByRole('dialog', { name: 'Connect Canvas', exact: true });
  await connect.getByLabel('Canvas address', { exact: true }).fill(canvas.base);
  await connect.getByLabel('Access token', { exact: true }).fill(canvas.token);
  await connect.getByRole('button', { name: 'Connect Canvas', exact: true }).click();
  await expect.poll(async () => (await imported(page)).length).toBe(1);
  expect((await imported(page))[0]).toEqual({ accept: 'application/x-ndjson', body: { base: canvas.base, token: canvas.token } });

  const task = assignment('1', 'Available while Canvas checks modules');
  await send(page, { type: 'tasks', tasks: [task] });
  const connected = page.getByRole('dialog', { name: 'Canvas Connected', exact: true });
  await expect(connected).toBeVisible();
  await connected.getByRole('button', { name: 'Done', exact: true }).click();
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
  await expect(taskRow(page, task.title)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sync assignments', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).canvasTest.feeds)).toBe(0);
  await expect.poll(async () => (await savedState(page)).tasks.map((task: any) => task.title)).toContain(task.title);

  await send(page, { type: 'complete', tasks: [{ ...task, moduleItem: 'module-1' }] }, true);
  await expect(page.getByRole('button', { name: 'Sync assignments', exact: true })).toBeEnabled();
  await expect.poll(async () => (await savedState(page)).tasks.find((item: any) => item.uid === task.uid)?.moduleItem).toBe('module-1');
  expect(await page.evaluate(() => (window as any).canvasTest.feeds)).toBe(0);
});

for (const ending of ['error', 'truncated'] as const) {
  test(`${ending === 'error' ? 'an error' : 'a truncated'} completion stream preserves earlier Canvas assignments`, async ({ page }) => {
    await mockCanvas(page);
    await open(page);
    const old = assignment('old', 'Previously imported assignment');
    await seed(page, [old]);
    const before = (await savedState(page)).lms.canvas.synced;
    await syncNow(page);
    const fresh = assignment('new', 'Fresh assignment before interruption');
    await send(page, { type: 'tasks', tasks: [fresh] });
    await expect(taskRow(page, fresh.title)).toBeVisible();
    await expect(taskRow(page, old.title)).toBeVisible();
    await send(page, ending === 'error' ? { type: 'error', error: 'Canvas completion checks were interrupted' } : null, true);
    await expect(page.getByRole('button', { name: 'Sync assignments', exact: true })).toBeEnabled();
    await expect.poll(async () => (await savedState(page)).tasks.map((task: any) => task.title).sort()).toEqual([fresh.title, old.title].sort());
    expect((await savedState(page)).lms.canvas.synced).toBe(before);
  });
}

test('a late Canvas completion snapshot preserves a task the user unchecked during sync', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const task = assignment('1', 'Review this assignment again', true);
  await seed(page, [task]);
  await page.getByRole('button', { name: 'Filter and Sort', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Show Completed', exact: true }).click();
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [task] });
  const checkbox = taskRow(page, task.title).getByRole('checkbox', { name: 'Done', exact: true });
  await expect(checkbox).toBeChecked();
  await checkbox.uncheck();
  await expect(checkbox).not.toBeChecked();
  await send(page, { type: 'complete', tasks: [{ ...task, moduleItem: 'module-1' }] }, true);
  await expect(page.getByRole('button', { name: 'Sync assignments', exact: true })).toBeEnabled();
  await expect(checkbox).not.toBeChecked();
  await expect.poll(() => page.evaluate(() => (window as any).canvasTest.completions)).toEqual([
    { base: canvas.base, token: canvas.token, type: 'assignment', id: '1', done: false, overrideId: '', courseId: '101', moduleItem: 'module-1' },
  ]);
  await expect.poll(async () => (await savedState(page)).tasks.find((item: any) => item.uid === task.uid)?.done).toBe(false);
});

test('an unchecked Canvas task survives reload and sends its saved change after enrichment', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const task = assignment('1', 'Reopen this Canvas assignment', true);
  await seed(page, [task]);
  await page.getByRole('button', { name: 'Filter and Sort', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Show Completed', exact: true }).click();
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [task] });
  await taskRow(page, task.title).getByRole('checkbox', { name: 'Done', exact: true }).uncheck();
  await expect.poll(async () => {
    const saved = (await savedState(page)).tasks.find((item: any) => item.uid === task.uid);
    return { done: saved?.done, pending: !!saved?.pendingCanvasDone };
  }).toEqual({ done: false, pending: true });
  expect(await page.evaluate(() => (window as any).canvasTest.completions)).toEqual([]);

  await page.reload();
  await settle(page);
  await expect.poll(async () => (await imported(page)).length).toBe(1);
  const checkbox = taskRow(page, task.title).getByRole('checkbox', { name: 'Done', exact: true });
  await expect(checkbox).not.toBeChecked();
  await send(page, { type: 'tasks', tasks: [task] });
  await expect(page.getByRole('button', { name: 'Sync assignments', exact: true })).toBeDisabled();
  await expect(checkbox).not.toBeChecked();
  expect(await page.evaluate(() => (window as any).canvasTest.completions)).toEqual([]);
  await send(page, { type: 'complete', tasks: [{ ...task, moduleItem: 'module-after-reload' }] }, true);
  await expect.poll(() => page.evaluate(() => (window as any).canvasTest.completions)).toEqual([
    { base: canvas.base, token: canvas.token, type: 'assignment', id: '1', done: false, overrideId: '', courseId: '101', moduleItem: 'module-after-reload' },
  ]);
  await expect(checkbox).not.toBeChecked();
  await expect.poll(async () => {
    const saved = (await savedState(page)).tasks.find((item: any) => item.uid === task.uid);
    return { done: saved?.done, pending: !!saved?.pendingCanvasDone };
  }).toEqual({ done: false, pending: false });
});

test('Canvas enrichment preserves an open task draft and edits saved during a later refresh', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const task = assignment('1', 'Reading assignment from Canvas');
  await seed(page, [task]);
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [task] });
  await taskRow(page, task.title).getByRole('button').click();
  const dialog = page.getByRole('dialog', { name: 'Edit Task', exact: true });
  await dialog.getByLabel('Title', { exact: true }).fill('My unfinished reading plan');
  await dialog.getByLabel('Due date', { exact: true }).fill('2026-09-21');
  await dialog.getByLabel('Due time', { exact: true }).fill('09:30');
  await dialog.getByLabel('Note', { exact: true }).fill('Keep these draft notes');
  await send(page, { type: 'complete', tasks: [{ ...task, title: 'Updated title from Canvas', due: '2026-09-22T20:00:00Z', moduleItem: 'module-1' }] }, true);
  await expect.poll(async () => (await savedState(page)).tasks.find((item: any) => item.uid === task.uid)?.moduleItem).toBe('module-1');
  await expect(dialog.getByLabel('Title', { exact: true })).toHaveValue('My unfinished reading plan');
  await expect(dialog.getByLabel('Due date', { exact: true })).toHaveValue('2026-09-21');
  await expect(dialog.getByLabel('Due time', { exact: true })).toHaveValue('09:30');
  await expect(dialog.getByLabel('Note', { exact: true })).toHaveValue('Keep these draft notes');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).not.toBeVisible();

  await page.getByRole('button', { name: 'Sync assignments', exact: true }).click();
  await expect.poll(async () => (await imported(page)).length).toBe(2);
  await send(page, { type: 'tasks', tasks: [task] }, false, 1);
  await taskRow(page, task.title).getByRole('button').click();
  await dialog.getByLabel('Title', { exact: true }).fill('My saved reading plan');
  await dialog.getByLabel('Due date', { exact: true }).fill('2026-09-23');
  await dialog.getByLabel('Due time', { exact: true }).fill('10:45');
  await dialog.getByLabel('Note', { exact: true }).fill('Read the examples first');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await send(page, { type: 'complete', tasks: [{ ...task, due: '2026-09-22T20:00:00Z', moduleItem: 'module-2' }] }, true, 1);
  const due = await page.evaluate(() => new Date('2026-09-23T10:45:00').toISOString());
  await expect.poll(async () => {
    const saved = (await savedState(page)).tasks.find((item: any) => item.uid === task.uid);
    return { title: saved?.title, due: saved?.due, note: saved?.note, moduleItem: saved?.moduleItem };
  }).toEqual({ title: 'My saved reading plan', due, note: 'Read the examples first', moduleItem: 'module-2' });
  await expect(taskRow(page, 'My saved reading plan')).toBeVisible();
});

test('a due date changed by hand survives later syncs until Canvas moves it', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const canvasDue = '2026-10-02T04:59:59Z';
  const essay = { ...assignment('1', 'Essay from Canvas'), due: canvasDue };
  const reading = { ...assignment('2', 'Reading from Canvas'), due: canvasDue };
  const saved = async () => {
    const tasks = (await savedState(page)).tasks;
    const find = (uid: string) => tasks.find((item: any) => item.uid === uid);
    return { essay: find(essay.uid)?.due, reading: find(reading.uid)?.due, imported: find(essay.uid)?.importedDue, pass: find(essay.uid)?.moduleItem };
  };
  await seed(page, [essay, reading]);
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [essay, reading] });
  await send(page, { type: 'complete', tasks: [essay, reading] }, true);
  await expect.poll(async () => (await saved()).imported).toBe(canvasDue);

  const dialog = page.getByRole('dialog', { name: 'Edit Task', exact: true });
  await taskRow(page, essay.title).getByRole('button').click();
  await dialog.getByLabel('Due date', { exact: true }).fill('2026-10-05');
  await dialog.getByLabel('Due time', { exact: true }).fill('18:00');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  // Saving only a note keeps Canvas's own due time, seconds included.
  await taskRow(page, reading.title).getByRole('button').click();
  await dialog.getByLabel('Note', { exact: true }).fill('Chapter 4');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).not.toBeVisible();

  const refresh = async (index: number, tasks: unknown[]) => {
    await page.getByRole('button', { name: 'Sync assignments', exact: true }).click();
    await expect.poll(async () => (await imported(page)).length).toBe(index + 1);
    await send(page, { type: 'tasks', tasks }, false, index);
    await send(page, { type: 'complete', tasks: tasks.map(task => ({ ...(task as object), moduleItem: `pass-${index}` })) }, true, index);
  };
  const mine = await page.evaluate(() => new Date('2026-10-05T18:00:00').toISOString());
  await refresh(1, [essay, reading]);
  await expect.poll(saved).toEqual({ essay: mine, reading: canvasDue, imported: canvasDue, pass: 'pass-1' });
  const moved = '2026-10-09T04:59:59Z';
  await refresh(2, [{ ...essay, due: moved }, reading]);
  await expect.poll(saved).toEqual({ essay: moved, reading: canvasDue, imported: moved, pass: 'pass-2' });
});

test('startup and repeated lifecycle refreshes share one import and keep fresh assignments', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const task = assignment('1', 'An assignment refreshed in the background');
  await seed(page, [task], true, [], 0);
  await expect.poll(async () => (await imported(page)).length).toBe(1);
  const refresh = page.getByRole('button', { name: 'Sync assignments', exact: true });
  await expect(refresh).toBeDisabled();
  await send(page, { type: 'tasks', tasks: [task] });
  const revisit = () => page.evaluate(() => {
    for (let i = 0; i < 3; i++) {
      document.dispatchEvent(new Event('visibilitychange'));
      window.dispatchEvent(new Event('online'));
    }
  });
  await revisit();
  await page.getByRole('button', { name: 'Filter and Sort', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Sync Now', exact: true })).toHaveCount(0);
  await expect(page.getByRole('menu').locator('[role^="menuitem"]').first()).toHaveAccessibleName('Show Completed');
  await expect(refresh).toBeDisabled();
  await page.keyboard.press('Escape');
  expect((await imported(page)).length).toBe(1);
  await send(page, { type: 'complete', tasks: [task] }, true);
  await expect(refresh).toBeEnabled();
  await revisit();
  expect((await imported(page)).length).toBe(1);
  await expect(taskRow(page, task.title)).toBeVisible();
});

test('disconnecting Canvas prevents a late snapshot from restoring assignments', async ({ page }) => {
  await mockCanvas(page);
  await open(page);
  const task = assignment('1', 'Disconnected Canvas assignment');
  await seed(page, [task]);
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [task] });
  await openSettings(page, 'Assignments');
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Disconnect', exact: true }).click();
  await page.getByRole('dialog', { name: 'Disconnect Canvas?', exact: true }).getByRole('button', { name: 'Disconnect', exact: true }).click();
  await send(page, { type: 'complete', tasks: [task] }, true);
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
  await expect(taskRow(page, task.title)).not.toBeVisible();
  await expect.poll(async () => (await savedState(page)).lms.canvas.token).toBe('');
  expect((await savedState(page)).tasks.filter((task: any) => task.source === 'canvas')).toEqual([]);
});

test('Canvas sync removes open assignments that left Canvas only after a complete import', async ({ page }) => {
  const task = assignment('new', 'Still in Canvas');
  // Canvas lists only the past week, so a finished task it stops listing stays.
  const finished = { ...assignment('done', 'Finished last month', true), due: '2026-08-20T04:59:59Z' };
  await mockCanvas(page);
  await open(page);
  await seed(page, [assignment('old', 'Removed from Canvas'), finished]);
  await syncNow(page);
  await send(page, { type: 'tasks', tasks: [task] });
  await expect(taskRow(page, task.title)).toBeVisible();
  await expect(taskRow(page, 'Removed from Canvas')).toBeVisible();
  await send(page, { type: 'complete', tasks: [task] }, true);
  await expect(taskRow(page, 'Removed from Canvas')).not.toBeVisible();
  await expect.poll(async () => (await savedState(page)).tasks.map((task: any) => task.title)).toEqual([finished.title, task.title]);
});

for (const width of [1280, 390]) {
  test(`failed Canvas completion offers token renewal at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.route('**/api/lms/canvas/complete', route => route.fulfill({ status: 502, json: { error: 'Canvas rejected the token' } }));
    await open(page);
    const task = assignment('1', 'Assignment with an expired Canvas token');
    await seed(page, [task]);
    if (width < 600) await page.getByRole('tab', { name: 'Tasks', exact: true }).click();
    await taskRow(page, task.title).getByRole('checkbox', { name: 'Done', exact: true }).check();
    const toast = page.getByRole('status');
    await expect(toast).toContainText('Canvas was not updated. Your token may have expired');
    const link = toast.getByRole('link', { name: 'Reconnect Canvas', exact: true });
    await expect(link).toHaveAttribute('href', '#settings/assignments');
    await expect.poll(async () => {
      const saved = (await savedState(page)).tasks.find((item: any) => item.uid === task.uid);
      return { done: saved?.done, pending: !!saved?.pendingCanvasDone };
    }).toEqual({ done: true, pending: true });
    await link.click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(settings).toBeVisible();
    await settings.getByRole('button', { name: 'Reconnect Canvas', exact: false }).click();
    const connect = page.getByRole('dialog', { name: 'Connect Canvas', exact: true });
    await expect(connect.getByLabel('Access token', { exact: true })).toBeVisible();
    await expect(connect.getByLabel('Access token', { exact: true })).toHaveValue('');
    await expect(connect.getByLabel('Canvas address', { exact: true })).toHaveValue(canvas.base);
  });
}

test('a Canvas completion that cannot reach the server does not blame the token', async ({ page }) => {
  await page.route('**/api/lms/canvas/complete', route => route.abort());
  await open(page);
  const task = assignment('1', 'Assignment written while offline');
  await seed(page, [task]);
  await taskRow(page, task.title).getByRole('checkbox', { name: 'Done', exact: true }).check();
  const toast = page.getByRole('status');
  await expect(toast).toContainText('Canvas was not updated. The server could not be reached');
  await expect(toast.getByRole('link', { name: 'Reconnect Canvas', exact: true })).toHaveCount(0);
});
