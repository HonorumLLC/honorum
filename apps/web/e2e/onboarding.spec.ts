import { test, expect, type Page } from '@playwright/test';
import { localState, openSettings } from './helpers';

// The first run ends with the student's classes on the week. At a Canvas
// school, Canvas supplies them. Rust tests cover the server match logic.
// These tests mock its response.
const meeting = (days: string, start: number, end: number) => [{ days, start, end, location: 'Hall 1' }];
const section = (id: string, course: string, num: string, type: string, link: string, meetings: unknown) =>
  ({ id, course, subject: course.split(' ')[0], title: course === 'CS 101' ? 'Intro to Computing' : 'Calculus I', num, type, link, instructors: [], meetings, available: 3, capacity: 30, waitlist: 0 });
const found = {
  term: '2026FA',
  classes: [
    { course: { id: '1', name: 'CS-101-001: Intro', code: 'CS-101-001' }, code: 'CS 101', title: 'Intro to Computing', sections: ['10001'], options: [['10002', '10003']], sure: true },
    { course: { id: '2', name: 'Calculus', code: 'MATH-151' }, code: 'MATH 151', title: 'Calculus I', sections: [], options: [['10020', '10021']], sure: false },
  ],
  unmatched: [{ id: '5', name: 'Campus Engagement', code: 'ENGAGE' }],
  sections: {
    10001: section('10001', 'CS 101', '001', 'LEC', 'A', meeting('MWF', 540, 590)),
    10002: section('10002', 'CS 101', 'L01', 'LAB', 'A', meeting('T', 780, 890)),
    10003: section('10003', 'CS 101', 'L02', 'LAB', 'A', meeting('R', 780, 890)),
    10020: section('10020', 'MATH 151', '001', 'LEC', '', meeting('MWF', 600, 650)),
    10021: section('10021', 'MATH 151', '002', 'LEC', '', meeting('MWF', 660, 710)),
  },
};

async function canvasSchool(page: Page) {
  await page.route('**/api/config', async route => {
    const config = await (await route.fetch()).json();
    config.institutions = config.institutions.map((i: Record<string, unknown>) => ({ ...i, lms: { kind: 'canvas', url: 'https://canvas.example.edu' } }));
    await route.fulfill({ json: config });
  });
}
const importTasks = (tasks: unknown[]) => [JSON.stringify({ type: 'tasks', tasks }), JSON.stringify({ type: 'complete', tasks }), ''].join('\n');

test('a Canvas school brings the classes and their assignments from Canvas', async ({ page }) => {
  await canvasSchool(page);
  const due = new Date(Date.now() + 3 * 86400e3).toISOString();
  await page.route('**/api/lms/canvas', route => route.fulfill({ contentType: 'application/x-ndjson', body: importTasks([
    { uid: 'canvas:assignment:1', title: 'Homework 1', course: 'CS-101-001: Intro', courseId: '1', courseCode: 'CS-101-001', due, done: false },
  ]) }));
  const asked: unknown[] = [];
  await page.route('**/api/lms/canvas/classes', route => { asked.push(route.request().postDataJSON()); route.fulfill({ json: found }); });
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'Add Your Classes', exact: true })).toBeVisible();
  await expect(welcome.getByRole('link', { name: 'Get a Token' })).toHaveAttribute('href', 'https://canvas.example.edu/profile/settings');
  const token = welcome.getByLabel('Canvas access token', { exact: true });
  await page.screenshot({ path: 'test-results/onboarding-canvas.png', animations: 'disabled' });
  await token.fill('short');
  await welcome.getByRole('button', { name: 'Find Classes', exact: true }).click();
  await expect(welcome.getByRole('alert')).toContainText('does not look like a Canvas token');
  await token.fill('test-canvas-token-with-enough-characters');
  await welcome.getByRole('button', { name: 'Find Classes', exact: true }).click();
  await expect(welcome.getByText('Fall 2026', { exact: true })).toBeVisible();
  expect(asked).toEqual([{ inst: 'demo', term: expect.any(String), base: 'https://canvas.example.edu', token: 'test-canvas-token-with-enough-characters' }]);
  await expect(welcome.getByText('Campus Engagement')).toHaveCount(0);
  // A class with a section is ticked; one that is only a choice is ticked by choosing.
  await expect(welcome.getByRole('checkbox', { name: 'Add CS 101' })).toBeChecked();
  await expect(welcome.getByRole('checkbox', { name: 'Add MATH 151' })).not.toBeChecked();
  await expect(welcome.getByRole('button', { name: 'Add 1 Class', exact: true })).toBeVisible();
  await welcome.getByLabel('CS 101 Lab').selectOption('10003');
  await welcome.getByLabel('MATH 151 Lecture').selectOption('10021');
  await expect(welcome.getByRole('checkbox', { name: 'Add MATH 151' })).toBeChecked();
  await page.screenshot({ path: 'test-results/onboarding-classes.png', animations: 'disabled' });
  await welcome.getByRole('button', { name: 'Add 2 Classes', exact: true }).click();
  await expect(welcome).not.toBeVisible();
  await expect(page.getByRole('button', { name: /^CS 101-001/ }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: /^MATH 151-002/ }).first()).toBeVisible();
  await expect.poll(async () => {
    const state = await localState(page);
    const schedule = state.schedules.find((s: { id: string }) => s.id === state.current);
    return [schedule.sections, state.lms.map, state.onboarded];
  }).toEqual([['10001', '10003', '10021'], { 1: 'CS 101', 2: 'MATH 151' }, true]);
  // The assignment Canvas brought belongs to its class.
  await expect(page.locator('.tasks li.row', { hasText: 'Homework 1' })).toContainText('CS 101');
});

test('a school without Canvas imports assignments first and ends in the class search', async ({ page }) => {
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'Import Assignments', exact: true })).toBeVisible();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'Add Your Classes', exact: true })).toBeVisible();
  await welcome.getByRole('button', { name: 'Back', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'Import Assignments', exact: true })).toBeVisible();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await welcome.getByRole('button', { name: 'Search Classes', exact: true }).click();
  await expect(welcome).not.toBeVisible();
  await expect(page.getByRole('searchbox', { name: 'Search classes' })).toBeVisible();
  expect((await localState(page)).onboarded).toBe(true);
});

// Every page shares one frame: the title and the main button stay put
// however much a page holds, and a school is chosen before Continue. In a
// window the button ends the footer, so its trailing edge is what holds.
test('the tour keeps its title and main button in place from page to page', async ({ page }) => {
  await page.route('**/api/config', async route => {
    const config = await (await route.fetch()).json();
    const school = config.institutions[0];
    config.institutions = [school, { ...school, id: 'other', name: 'Another School', terms: [] }];
    await route.fulfill({ json: config });
  });
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  const title = welcome.getByRole('heading', { level: 2 }), main = welcome.locator('button[type=submit]');
  const places: unknown[] = [];
  const place = async () => {
    await page.waitForTimeout(250);
    const [t, b] = [await title.boundingBox(), await main.boundingBox()];
    places.push([t!.y, b!.x + b!.width, b!.y, b!.height]);
  };
  await expect(main).toHaveText('Continue');
  await expect(main).toBeDisabled();
  await place();
  await welcome.getByRole('button', { name: 'Demo University', exact: true }).click();
  await expect(welcome.getByRole('button', { name: 'Demo University', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await main.click();
  await expect(welcome.getByRole('heading', { name: 'Import Assignments', exact: true })).toBeVisible();
  await place();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'Add Your Classes', exact: true })).toBeVisible();
  await place();
  expect(new Set(places.map((p) => JSON.stringify(p))).size).toBe(1);
});

test('Canvas errors keep the token page, and finding no classes still finishes', async ({ page }) => {
  await canvasSchool(page);
  let rejected = true;
  await page.route('**/api/lms/canvas', route => rejected
    ? route.fulfill({ status: 502, json: { error: 'the service rejected the token or feed url' } })
    : route.fulfill({ contentType: 'application/x-ndjson', body: importTasks([]) }));
  await page.route('**/api/lms/canvas/classes', route => route.fulfill({ json: { term: '2026FA', classes: [], unmatched: [], sections: {} } }));
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await welcome.getByLabel('Canvas access token', { exact: true }).fill('test-canvas-token-with-enough-characters');
  await welcome.getByRole('button', { name: 'Find Classes', exact: true }).click();
  await expect(welcome.getByRole('alert')).toHaveText('Canvas rejected that token');
  await expect(welcome.getByLabel('Canvas access token', { exact: true })).toBeVisible();
  rejected = false;
  await welcome.getByLabel('Canvas access token', { exact: true }).fill('test-canvas-token-with-enough-characters');
  await welcome.getByRole('button', { name: 'Find Classes', exact: true }).click();
  await expect(welcome.getByRole('heading', { name: 'No Classes Found', exact: true })).toBeVisible();
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(welcome).not.toBeVisible();
  await expect(page.getByRole('heading', { name: 'Nothing Scheduled', exact: true })).toBeVisible();
});

test('Settings finds classes in Canvas again for a later term', async ({ page }) => {
  await canvasSchool(page);
  await page.route('**/api/lms/canvas', route => route.fulfill({ contentType: 'application/x-ndjson', body: importTasks([]) }));
  await page.route('**/api/lms/canvas/classes', route => route.fulfill({ json: found }));
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await welcome.getByLabel('Canvas access token', { exact: true }).fill('test-canvas-token-with-enough-characters');
  await welcome.getByRole('button', { name: 'Find Classes', exact: true }).click();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await expect(welcome).not.toBeVisible();
  await openSettings(page, 'Schedule');
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: /^Add Classes from Canvas/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Classes from Canvas', exact: true });
  await expect(dialog.getByRole('checkbox', { name: 'Add CS 101' })).toBeChecked();
  await page.screenshot({ path: 'test-results/settings-canvas-classes.png', animations: 'disabled' });
  await dialog.getByRole('button', { name: 'Add 1 Class', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect.poll(async () => {
    const state = await localState(page);
    return state.schedules.find((s: { id: string }) => s.id === state.current).sections;
  }).toEqual(['10001']);
});
