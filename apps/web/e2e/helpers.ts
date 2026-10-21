import { expect, type Page } from '@playwright/test';

// Waits for the planner, skipping optional account and assignment setup.
export async function settle(page: Page) {
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await expect(page.getByTitle('Add Event', { exact: true }).or(welcome)).toBeVisible();
  for (let step = 0; step < 4 && await welcome.isVisible(); step++) {
    const next = welcome.getByRole('button', { name: /^Continue$/ });
    if (await next.isVisible()) await next.click();
    else await welcome.getByRole('button', { name: /^(Skip|Not Now)$/ }).click();
  }
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled();
}

export async function open(page: Page, url = '/') {
  await page.goto(url);
  await settle(page);
}

// Read the canonical record selected by the durable owner pointer.
export async function localState(page: Page, key = 'state') {
  return page.evaluate(async key => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const r = indexedDB.open('honorum'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error);
    });
    try {
      return await new Promise<any>((resolve, reject) => {
        const tx = db.transaction('kv'), store = tx.objectStore('kv');
        tx.onabort = () => reject(tx.error);
        const read = (name: string) => { const r = store.get(name); r.onsuccess = () => resolve(r.result); };
        if (key !== 'state' && key !== 'sync') { read(key); return; }
        const owner = store.get('activeOwner'); owner.onsuccess = () => read(key + ':' + (owner.result || 'guest'));
      });
    } finally { db.close(); }
  }, key);
}

// The header bar's controls. Editing classes starts from the header bar's
// Edit Classes button on a wide screen and from the Classes tab of the
// phone's bottom bar; Done in the header bar ends it. "Add Classes" on the
// empty week is not matched.
export const header = (page: Page) => page.getByRole('banner');
export const findClasses = (page: Page) => header(page).getByRole('button', { name: 'Edit Classes', exact: true }).or(page.getByRole('tab', { name: 'Classes', exact: true }));
export const doneEditing = (page: Page) => header(page).getByRole('button', { name: 'Done', exact: true }).click();
// Settings is a preferences dialog whose pages (General, Account, Schedule,
// Assignments) are listed in a sidebar. On phones the list is the first thing
// shown and a page covers it until Back returns.
export type SettingsPage = 'General' | 'Account' | 'Schedule' | 'Assignments';
export async function openSettings(page: Page, section: SettingsPage = 'General') {
  await header(page).getByRole('button', { name: 'Main Menu', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
  await settingsPage(page, section);
}
export async function settingsPage(page: Page, section: SettingsPage) {
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  const item = dialog.getByRole('navigation').getByRole('button', { name: section, exact: true });
  if (!await item.isVisible()) await dialog.getByRole('button', { name: 'Back', exact: true }).click();
  if (await item.getAttribute('aria-current') !== 'page') await item.click();
  await expect(dialog.getByRole('heading', { name: section, exact: true })).toBeVisible();
}
