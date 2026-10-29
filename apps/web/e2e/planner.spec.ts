import { test, expect, type Page } from '@playwright/test';
import { open, settle, localState, findClasses, doneEditing, header, openSettings, settingsPage } from './helpers';

const ready = (page: Page) => open(page);
async function event(page: Page, name: string) {
  await page.getByTitle('Add Event', { exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Event' });
  await dialog.getByLabel('Name', { exact: true }).fill(name);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).not.toBeVisible();
}
async function savedBlocks(page: Page) {
  const state = await localState(page);
  return state?.schedules.flatMap((s: any) => s.blocks.map((b: any) => b.name)) || [];
}

test('edits survive immediate reload and cold offline lazy views', async ({ page, context }) => {
  const errors: string[] = []; page.on('pageerror', e => errors.push(e.message));
  await ready(page);
  await event(page, 'Study group');
  await expect.poll(() => savedBlocks(page)).toContain('Study group');
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  await page.reload();
  await expect.poll(() => savedBlocks(page)).toContain('Study group');
  await context.setOffline(true);
  await page.reload();
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled();
  await openSettings(page);
  await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test('tab handover retains edits from both tabs', async ({ page, context }) => {
  await ready(page); await event(page, 'First tab');
  await expect.poll(() => savedBlocks(page)).toContain('First tab');
  const second = await context.newPage();
  // Headless windows do not reliably emit OS focus changes; dispatch the
  // same browser events explicitly while exercising real Web Locks and IDB.
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await ready(second); await event(second, 'Second tab');
  await expect.poll(() => savedBlocks(second)).toEqual(['First tab', 'Second tab']);
  await second.evaluate(() => window.dispatchEvent(new Event('blur')));
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled();
  await event(page, 'Back in first');
  await expect.poll(() => savedBlocks(page)).toEqual(['First tab', 'Second tab', 'Back in first']);
});

test('an event being written up survives leaving the tab and coming back', async ({ page }) => {
  await ready(page); await event(page, 'Earlier');
  await expect.poll(() => savedBlocks(page)).toContain('Earlier');
  await page.getByTitle('Add Event', { exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Event' });
  await dialog.getByLabel('Name', { exact: true }).fill('Club meeting');
  await dialog.getByLabel('Location', { exact: true }).fill('Room 12');
  const content = page.locator('.app-content');
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect(content).toHaveAttribute('inert');
  // Coming back restores the schedule from storage.
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(content).not.toHaveAttribute('inert');
  await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('Club meeting');
  await expect(dialog.getByLabel('Location', { exact: true })).toHaveValue('Room 12');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect.poll(() => savedBlocks(page)).toEqual(['Earlier', 'Club meeting']);
});

test('events keep notes and take a class or a color of their own', async ({ page }) => {
  await ready(page);
  // Without classes the dialog offers only a color label.
  await page.getByTitle('Add Event', { exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Event' });
  await dialog.getByLabel('Name', { exact: true }).fill('Gym');
  await dialog.getByLabel('Notes', { exact: true }).fill('Bring a towel');
  await dialog.getByRole('radiogroup', { name: 'Color' }).getByRole('radio').nth(4).click();
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  const blocks = async () => { const state = await localState(page); return state.schedules.flatMap((s: any) => s.blocks); };
  await expect.poll(async () => (await blocks()).map((b: any) => [b.name, b.note, b.color, b.course])).toEqual([['Gym', 'Bring a towel', '#d3f0da', '']]);
  // The event's box on the week carries the chosen color; its detail shows the note.
  const gym = page.getByRole('button', { name: /^Gym\b/ }).first();
  await expect(gym).toHaveCSS('background-color', 'rgb(211, 240, 218)');
  await gym.click();
  await expect(page.getByRole('complementary', { name: 'Side pane' }).getByText('Bring a towel')).toBeVisible();
  // Add a class, then give the event to it: the box takes the class's color.
  await findClasses(page).click();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS 101');
  await page.locator('.results').getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  await doneEditing(page);
  await gym.click();
  await page.getByRole('complementary', { name: 'Side pane' }).getByRole('button', { name: 'Edit', exact: true }).click();
  const edit = page.getByRole('dialog', { name: 'Edit Event' });
  await edit.getByRole('combobox', { name: 'Class' }).selectOption('CS 101');
  await expect(edit.getByText('Same as CS 101')).toBeVisible();
  await edit.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(async () => (await blocks()).map((b: any) => [b.course, b.color])).toEqual([['CS 101', '']]);
  await expect(gym).toHaveCSS('background-color', 'rgb(207, 230, 251)');
});

test('a setting puts tasks on the hours at their due time', async ({ page }) => {
  await ready(page);
  await event(page, 'Study'); // the week grid draws once the schedule has something on it
  await page.getByRole('button', { name: /^Add task$/i }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Task' });
  await dialog.getByLabel('Title', { exact: true }).fill('Problem set 1');
  const today = new Date();
  const ymd = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`;
  await dialog.getByLabel('Due date', { exact: true }).fill(ymd);
  await dialog.getByLabel('Due time', { exact: true }).fill('14:00');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  // The due row holds the task; nothing sits on the hours.
  const row = page.locator('.due .task', { hasText: 'Problem set 1' });
  await expect(row).toBeVisible();
  await expect(page.locator('.col .due-at')).toHaveCount(0);
  await openSettings(page, 'Schedule');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  await settings.getByLabel('Tasks at Their Due Time').check();
  await page.keyboard.press('Escape');
  const block = page.locator('.col .due-at', { hasText: 'Problem set 1' });
  await expect(block).toBeVisible();
  await expect(block).toHaveAttribute('title', /due 2:00pm/);
  await expect(row).toHaveCount(0);
  // A half-hour block ending at 2pm sits 13.5 hours down a 56px-per-hour grid.
  expect(await block.evaluate((el) => parseFloat((el as HTMLElement).style.top))).toBe(13.5 * 56);
  await block.click();
  await expect(page.getByRole('dialog', { name: 'Edit Task' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.reload();
  await settle(page);
  await expect(page.locator('.col .due-at', { hasText: 'Problem set 1' })).toBeVisible();
});

test('tasks due throughout this week appear in the list without opening a fold', async ({ page }) => {
  await page.clock.setFixedTime(new Date('2026-10-05T15:00:00Z')); // Monday on the campus clock
  await ready(page);
  const tasks = page.locator('.tasks');
  for (const [title, date] of [['Friday reading', '2026-10-09'], ['Saturday quiz', '2026-10-10'], ['Sunday response', '2026-10-11'], ['Next Tuesday paper', '2026-10-13']]) {
    await page.getByRole('button', { name: /^Add task$/i }).click();
    const dialog = page.getByRole('dialog', { name: 'Add Task' });
    await dialog.getByLabel('Title', { exact: true }).fill(title);
    await dialog.getByLabel('Due date', { exact: true }).fill(date);
    await dialog.getByLabel('Due time', { exact: true }).fill('23:59');
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog).not.toBeVisible();
  }
  await expect(tasks.getByRole('button', { name: /^(Rest of )?This Week,/ })).toHaveCount(0);
  for (const day of ['Friday', 'Saturday', 'Sunday']) {
    await expect(tasks.getByRole('heading', { name: new RegExp(`^${day}`) })).toBeVisible();
    await expect(tasks.locator('li.row').filter({ hasText: day })).toBeVisible();
  }
  await expect(tasks.locator('li.row').filter({ hasText: 'Next Tuesday paper' })).toHaveCount(0);
  await tasks.getByRole('button', { name: 'Next Week, 1 task', exact: true }).click();
  await expect(tasks.locator('li.row').filter({ hasText: 'Next Tuesday paper' })).toBeVisible();
  await tasks.locator('li.row').filter({ hasText: 'Sunday response' }).getByRole('checkbox', { name: 'Done', exact: true }).check();
  await expect(tasks.locator('li.row').filter({ hasText: 'Sunday response' })).toHaveCount(0);
  await expect.poll(async () => (await localState(page)).tasks.find((t: any) => t.title === 'Sunday response')?.completedAt).toBeTruthy();
});

test('the progress card counts today and this week, and a guest streak offers sign-in', async ({ page }) => {
  await page.route('**/api/config', async route => {
    const response = await route.fetch();
    await route.fulfill({ json: { ...await response.json(), phone_auth: true } });
  });
  await ready(page);
  const progress = page.getByRole('region', { name: 'Progress', exact: true });
  await expect(progress).toHaveCount(0); // the card waits for a first task
  await page.getByRole('button', { name: /^Add task$/i }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Task' });
  await dialog.getByLabel('Title', { exact: true }).fill('Reading response');
  const today = new Date();
  const ymd = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`;
  await dialog.getByLabel('Due date', { exact: true }).fill(ymd);
  await dialog.getByLabel('Due time', { exact: true }).fill('23:59');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  // The card is open from the first task, on the Rings page.
  const todayRow = progress.getByRole('button', { name: /^Today:/ });
  await expect(todayRow).toHaveAccessibleName('Today: 0 of 1 done');
  await expect(progress.getByRole('button', { name: /^This Week:/ })).toHaveAccessibleName('This Week: 0 of 1 done');
  await expect(progress.getByRole('button', { name: /^This Term:/ })).toBeVisible();
  // Finishing the last task closes the rings and says so once.
  await page.locator('.tasks li.row').filter({ hasText: 'Reading response' }).getByRole('checkbox', { name: 'Done', exact: true }).check();
  await expect(todayRow).toHaveAccessibleName('Today: 1 of 1 done');
  await expect(page.getByText('All done', { exact: true })).toBeVisible();
  await expect.poll(async () => (await localState(page)).tasks[0].completedAt).toBeTruthy();
  // A ring shows its count in the middle, and Escape puts all three back.
  await todayRow.click();
  await expect(todayRow).toHaveAttribute('aria-pressed', 'true');
  await expect(progress.locator('.center')).toContainText(/^Today[\s\S]*\/ 1$/);
  await todayRow.press('Escape');
  await expect(todayRow).toHaveAttribute('aria-pressed', 'false');
  // Streaks need an account: the card's heading offers one above the rings.
  await expect(progress.getByRole('heading', { name: 'Keep a Streak', exact: true })).toBeVisible();
  await progress.getByRole('button', { name: 'Sign In', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Sign In', exact: true })).toBeVisible();
  await page.getByRole('dialog', { name: 'Sign In', exact: true }).press('Escape');
});

test('progress keeps labels and multi-digit counts inside narrow cards', async ({ page }) => {
  await page.clock.setFixedTime(new Date('2026-10-05T15:00:00Z'));
  await ready(page);
  await expect.poll(() => localState(page)).toBeTruthy();
  await page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>(resolve => {
      const request = indexedDB.open('honorum'); request.onsuccess = () => resolve(request.result);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
      const request = store.get('state:guest');
      request.onsuccess = () => {
        const state = request.result;
        state.tasks = Array.from({ length: 215 }, (_, i) => ({
          id: `progress-${i}`, title: `Assignment ${i}`, inst: state.inst, term: state.term,
          due: '2026-10-05T23:59', done: i < 56, completedAt: i < 56 ? '2026-10-05T14:00:00Z' : '',
        }));
        store.put(state, 'state:guest');
      };
    });
    db.close();
  });
  await page.reload(); await settle(page);
  const progress = page.getByRole('region', { name: 'Progress', exact: true });
  for (const width of [1440, 1024, 320]) {
    await page.setViewportSize({ width, height: 800 });
    if (width < 760) await page.getByRole('tab', { name: 'Tasks', exact: true }).click();
    await expect(progress.getByRole('button', { name: 'This Week: 56 of 215 done', exact: true })).toBeVisible();
    await expect.poll(() => progress.locator('.rings').evaluate(rings => {
      const bounds = rings.getBoundingClientRect();
      return [...rings.querySelectorAll<HTMLElement>('.lb, .ct')].every(el => {
        const rect = el.getBoundingClientRect();
        return el.scrollWidth <= el.clientWidth && rect.left >= bounds.left && rect.right <= bounds.right;
      });
    })).toBe(true);
  }
});

test('exam sittings published with a class appear on their date', async ({ page }) => {
  await page.clock.setFixedTime(new Date('2026-10-05T12:00:00Z'));
  await ready(page);
  await findClasses(page).click();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS 101 001');
  await page.locator('.results').getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  await doneEditing(page);
  const saved = async () => { const state = await localState(page); return state.schedules.find((s: any) => s.id === state.current).pinned; };
  await expect.poll(async () => Object.values(await saved()).map((p: any) => p.exams?.length)).toEqual([2]);
  // The demo lecture sits its midterm on 2026-10-08, a Thursday evening: page to that week.
  const exam = page.getByRole('button', { name: /^CS 101 Exam\b/ });
  for (let i = 0; i < 24 && !(await exam.isVisible()); i++) await header(page).getByRole('button', { name: 'Next Week', exact: true }).click();
  await expect(exam).toBeVisible();
  await expect(exam).toHaveClass(/exam/);
  await exam.click();
  const pane = page.getByRole('complementary', { name: 'Side pane' });
  await expect(pane.getByRole('heading', { name: 'CS 101 Exam' })).toBeVisible();
  await expect(pane.getByText('Engineering 110')).toBeVisible();
  await pane.getByRole('button', { name: 'Hide This Exam' }).click();
  await expect(exam).not.toBeVisible();
  await expect.poll(async () => { const state = await localState(page); return state.schedules.find((s: any) => s.id === state.current).removed; }).toEqual({ '10001:exam:0': ['2026-10-08'] });
  await page.getByRole('button', { name: 'Undo' }).click();
  await expect(exam).toBeVisible();
});

test('a returning planner takes exam sittings from seat counts and leaves the catalog alone', async ({ page }) => {
  await ready(page);
  await findClasses(page).click();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS 101 001');
  await page.locator('.results').getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  await doneEditing(page);
  const pinned = async () => { const state = await localState(page); return Object.values(state.schedules.find((s: any) => s.id === state.current).pinned) as any[]; };
  await expect.poll(async () => (await pinned()).map(p => p.exams.length)).toEqual([2]);
  // The school moves the final: the seat counts carry the new sitting.
  const moved = { date: '2026-12-14', start: 480, end: 600, location: 'Hall 9' };
  await page.route('**/api/seats?*', async route => {
    const seats = await (await route.fetch()).json();
    await route.fulfill({ json: seats.map((x: any) => ({ ...x, exams: [moved] })) });
  });
  const catalogs: string[] = [];
  page.on('request', request => { if (new URL(request.url()).pathname.startsWith('/catalog/')) catalogs.push(request.url()); });
  await page.reload(); await settle(page);
  await expect.poll(async () => (await pinned()).map(p => p.exams)).toEqual([[moved]]);
  await page.evaluate(() => new Promise<void>(resolve => requestIdleCallback(() => setTimeout(resolve, 300))));
  expect(catalogs).toEqual([]);
  // Editing classes loads it.
  await findClasses(page).click();
  await expect(page.locator('.results .course').first()).toBeVisible();
});

test('class search, native shortcuts, and browser back', async ({ page }) => {
  await ready(page);
  const prevented = await page.evaluate(() => {
    const event = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    document.body.dispatchEvent(event); return event.defaultPrevented;
  });
  expect(prevented).toBe(false);
  await page.keyboard.press('/');
  await expect(page.getByRole('searchbox', { name: 'Search classes' })).toBeFocused();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS');
  await expect(page.locator('.results .course').first()).toBeVisible();
  await expect(page).toHaveURL(/#classes$/);
  await page.goBack();
  await expect(page.getByRole('searchbox', { name: 'Search classes' })).not.toBeVisible();
});

test('settings lists its pages beside the page in a window and ahead of it on phones', async ({ page }) => {
  await ready(page);
  await openSettings(page, 'Account');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  const item = (name: string) => settings.getByRole('navigation').getByRole('button', { name, exact: true });
  await expect(page).toHaveURL(/#settings\/account$/);
  // Reopening focuses the page in view, and its rows keep their dividers.
  await page.keyboard.press('Escape');
  await openSettings(page, 'Account');
  await expect(item('Account')).toBeFocused();
  await settingsPage(page, 'General');
  expect(await settings.getByRole('link', { name: 'Privacy Policy and Terms' }).evaluate(el => getComputedStyle(el).borderTopWidth)).toBe('1px');
  // In a window the pages share one history entry, so Back closes Settings.
  await page.goBack();
  await expect(settings).not.toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  await openSettings(page, 'Schedule');
  await expect(settings.getByRole('button', { name: 'Back', exact: true })).toBeFocused();
  await expect(item('Schedule')).not.toBeVisible();
  await settings.getByRole('button', { name: 'Back', exact: true }).click();
  await expect(page).toHaveURL(/#settings$/);
  await expect(item('Schedule')).toBeFocused();
  await expect(item('Account')).toHaveAccessibleDescription('Not signed in');
  await expect(item('Assignments')).toHaveAccessibleDescription('Nothing connected');
  // On a phone a page covers the list, so Back returns to the list first.
  await item('Assignments').click();
  await expect(settings.getByRole('button', { name: 'Connect Canvas or a Calendar', exact: true })).toBeVisible();
  await page.goBack();
  await expect(item('Assignments')).toBeVisible();
  await expect(settings).toBeVisible();
});

test('settings class menu opens search and removes saved classes offline on phones', async ({ page, context }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page);
  await openSettings(page, 'Schedule');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  const classes = page.getByRole('dialog', { name: 'Manage Classes', exact: true });
  await settings.getByRole('button', { name: 'Manage Classes' }).click();
  await expect(classes.getByText(/No classes in this schedule/)).toBeVisible();
  await classes.getByRole('button', { name: 'Add Classes', exact: true }).click();
  await expect(settings).not.toBeVisible();
  await expect(classes).not.toBeVisible();
  // Phones leave the keyboard closed until the search entry is tapped.
  await expect(page.getByRole('searchbox', { name: 'Search classes' })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS');
  await page.locator('.results').getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  const sections = async () => { const state = await localState(page); return state.schedules.find((s: any) => s.id === state.current).sections; };
  await expect.poll(async () => (await sections()).length).toBe(1);
  await page.getByRole('tab', { name: 'Week', exact: true }).click();
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  await context.setOffline(true);
  await page.reload();
  await settle(page);
  await openSettings(page, 'Schedule');
  await settings.getByRole('button', { name: 'Manage Classes' }).click();
  await expect(classes.getByRole('button', { name: /^Remove / })).toHaveCount(1);
  expect(await classes.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/settings-classes-mobile.png', animations: 'disabled' });
  await classes.getByRole('button', { name: /^Remove / }).click();
  await expect(classes.getByText(/No classes in this schedule/)).toBeVisible();
  await expect.poll(sections).toEqual([]);
  await page.keyboard.press('Escape');
  await expect(classes).not.toBeVisible();
  await expect(settings.getByRole('button', { name: 'Manage Classes' })).toBeFocused();
  await page.reload();
  await expect.poll(sections).toEqual([]);
});

test('mobile planner has no horizontal page overflow', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await open(page);
  await findClasses(page).click();
  await expect(page.getByRole('searchbox', { name: 'Search classes' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('.results .course').first()).toBeVisible();
  const add = await page.locator('.results').getByRole('button', { name: /^Add .+ to Schedule$/ }).first().boundingBox();
  expect(add?.height).toBeGreaterThanOrEqual(44);
  await page.screenshot({ path: 'test-results/mobile.png', fullPage: true });
  await context.close();
});

test('saved planner becomes usable while configuration request is stalled', async ({ page }) => {
  await ready(page); await event(page, 'Local first');
  await expect.poll(() => savedBlocks(page)).toContain('Local first');
  await page.route('**/api/config', () => new Promise(() => {}));
  await page.reload();
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled({ timeout: 1500 });
  await event(page, 'Without the server');
  await expect.poll(() => savedBlocks(page)).toContain('Without the server');
});

test('rearrangement previews do not change saved sections', async ({ page }) => {
  await ready(page); await page.keyboard.press('/');
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('CS');
  await page.locator('.results .course').first().getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  const sections = async () => { const state = await localState(page); return state.schedules.find((s: any) => s.id === state.current).sections; };
  await expect.poll(async () => (await sections()).length).toBe(1);
  const before = await sections();
  await header(page).getByTitle('Schedule Menu', { exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rearrange Sections…' }).click();
  await page.getByRole('button', { name: 'Find Combinations', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Keep This One', exact: true })).toBeVisible();
  expect(await sections()).toEqual(before);
  await page.reload();
  expect(await sections()).toEqual(before);
  await expect(page.getByRole('button', { name: 'Keep This One', exact: true })).not.toBeVisible();
});

test('task course matching updates immediately when scheduled classes change', async ({ page }) => {
  await ready(page);
  const config = await (await page.request.get('/api/config')).json();
  const inst = config.institutions[0], term = inst.terms[0];
  const catalog = await (await page.request.get(`/catalog/${inst.id}/${term.id}.structure.${term.structure_snapshot}.json`)).json();
  const course = catalog.courses.find((c: any) => c.code.startsWith('CS') && c.sections.length);
  await page.evaluate(async (course) => {
    const db = await new Promise<IDBDatabase>(resolve => { const q = indexedDB.open('honorum'); q.onsuccess = () => resolve(q.result); });
    const owner = await new Promise<string>(resolve => { const q = db.transaction('kv').objectStore('kv').get('activeOwner'); q.onsuccess = () => resolve(q.result || 'guest'); });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
      const q = store.get('state:' + owner);
      q.onsuccess = () => {
        const state = q.result;
        state.tasks = [{ id: 'matching-test', title: 'Course matching task', course: course.title, source: 'manual', done: false, due: '' }];
        state.term = course.term;
        store.put(state, 'state:' + (state.dataOwner || 'guest'));
          store.put(state.dataOwner || 'guest', 'activeOwner');
      };
    }); db.close();
  }, { title: course.title, term: term.id });
  await page.reload();
  // The row names its class for assistive technology; the tick circle shows its color.
  const chip = page.getByRole('button').filter({ has: page.getByText('Course matching task', { exact: true }) }).locator('.course');
  await expect(chip).toHaveText(course.title);
  await page.keyboard.press('/');
  await page.getByRole('searchbox', { name: 'Search classes' }).fill(course.code);
  const result = page.locator('.results .course').first();
  await result.getByRole('button', { name: /^Add .+ to Schedule$/ }).first().click();
  await doneEditing(page); // The tasks pane is hidden while editing classes.
  await expect(chip).toHaveText(course.code);
  await page.keyboard.press('/');
  await result.getByRole('button', { name: /^Remove .+ From Schedule$/ }).click();
  await doneEditing(page);
  await expect(chip).toHaveText(course.title);
});

test('conflict recovery appears only for an archive and restores a separate copy', async ({ page }) => {
  await page.route('**/api/me**', async route => {
    if (/\/(docs|sync)$/.test(new URL(route.request().url()).pathname)) {
      const posted = route.request().method() === 'POST';
      await route.fulfill({ json: posted
        ? { applied: route.request().postDataJSON().docs.map((d: any, i: number) => ({ id: d.id, rev: i + 1 })), conflicts: [], docs: [], seq: 3, more: false }
        : { docs: [], seq: 3, more: false } });
    } else await route.fulfill({ json: { id: 'account-test', phone: '+12015550123', devices: [] } });
  });
  await ready(page);
  await expect.poll(() => savedBlocks(page)).toEqual([]);
  for (const archived of [false, true]) {
    await page.evaluate(async archived => {
      const db = await new Promise<IDBDatabase>(resolve => { const q = indexedDB.open('honorum'); q.onsuccess = () => resolve(q.result); });
    const owner = await new Promise<string>(resolve => { const q = db.transaction('kv').objectStore('kv').get('activeOwner'); q.onsuccess = () => resolve(q.result || 'guest'); });
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
        tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
        const q = store.get('state:' + owner);
        q.onsuccess = () => {
          const state = q.result;
          state.account = { id: 'account-test', phone: '+12015550123' }; state.dataOwner = state.account.id;
          store.put(state, 'state:' + (state.dataOwner || 'guest'));
          store.put(state.dataOwner || 'guest', 'activeOwner');
          const conflicts = archived ? [{ id: 's_remote', type: 'schedule', rev: 1,
            body: { ...state.schedules[0], id: 'remote', name: 'From another device' } }] : [];
          store.put({ owner: state.dataOwner, seq: 0, known: {}, outbox: {}, conflicts }, 'sync:' + state.dataOwner);
        };
      });
      db.close();
    }, archived);
    await page.reload();
    await openSettings(page, 'Account');
    const recover = page.getByRole('button', { name: 'Recover Conflicting Edits', exact: true });
    if (!archived) {
      await expect(recover).not.toBeVisible();
      await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
      continue;
    }
    await recover.click();
    await page.getByRole('button', { name: 'Restore Copy', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Conflicting Edits', exact: true })).not.toBeVisible();
    await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
    await page.keyboard.press('/'); // Schedules are listed in the schedule menu while editing classes.
    await header(page).getByTitle('Schedule Menu', { exact: true }).click();
    await expect(page.getByRole('menuitemradio', { name: 'From another device (recovered)', exact: true })).toBeVisible();
    await expect(page.getByRole('menuitemradio', { name: 'Schedule 1', exact: true })).toBeVisible();
  }
});
