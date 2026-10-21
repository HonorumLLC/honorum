<script>
  // A menu button and its popover. sections is a list of item groups drawn
  // with separators between them; an item is { label, action, disabled,
  // radio, check, checked, destructive, kbd, key }, or { heading } for a
  // section label; key tells apart items whose labels can repeat. kbd is
  // exposed as the item's key shortcut without a visible hint, so it stays
  // out of the item's name. Menus with radio or check items reserve a
  // leading column for the check mark, so their items line up; plain menus
  // start at the text. tint colors the button (a filter showing what it is
  // set to); active marks it as not at its default. The popover opens below
  // the button, aligned to its end.
  //
  // Motion: the popover grows out of the button's corner, items and all, and
  // shrinks back into it, quicker (see lib/motion.js).
  import { still, easeOpen, easeClose, QUICK_OPEN, QUICK_CLOSE } from './lib/motion.js';
  import Icon from './Icon.svelte';

  let { sections = [], label = '', icon = 'more', tooltip = '', tint = '', active = false } = $props();
  let open = $state(false);
  const marks = $derived(sections.some((sec) => sec.some((it) => it.radio || it.check)));
  let root, panel = $state(null);
  const focusable = () => [...(panel?.querySelectorAll('[role^=menuitem]:not(:disabled)') || [])];

  export function toggle() {
    open = !open;
    if (open) queueMicrotask(() => focusable()[0]?.focus());
  }
  function close(refocus) {
    if (!open) return;
    open = false;
    if (refocus) root.firstElementChild.focus();
  }
  function onkey(e) {
    if (!open || !root.contains(e.target)) return;
    if (e.key === 'Tab') { close(false); return; }
    if (e.key === 'Escape') { e.preventDefault(); close(true); return; }
    const step = { ArrowDown: 1, ArrowUp: -1 }[e.key];
    if (!step && e.key !== 'Home' && e.key !== 'End') return;
    e.preventDefault();
    const list = focusable();
    if (!list.length) return;
    const i = list.indexOf(document.activeElement);
    list[step ? (i + step + list.length) % list.length : e.key === 'Home' ? 0 : list.length - 1].focus();
  }
  const outside = (e) => open && !root.contains(e.target) && close();
  function run(it) {
    close(true);
    it.action?.();
  }
  const role = (it) => (it.radio ? 'menuitemradio' : it.check ? 'menuitemcheckbox' : 'menuitem');
  const pop = () => ({ direction }) => {
    if (still()) return { duration: 0 };
    const opening = direction === 'in';
    return { duration: opening ? QUICK_OPEN : QUICK_CLOSE, easing: opening ? easeOpen : easeClose, css: (t, u) => `opacity: ${t}; transform: translateY(${-4 * u}px) scale(${1 - .04 * u})` };
  };
</script>

<svelte:window onpointerdown={outside} onkeydown={onkey} />

<div class="menu" bind:this={root}>
  <button type="button" class="btn flat" class:on={open || active} class:icon={!label} class:tinted={tint} style:--c={tint || null} aria-haspopup="menu" aria-expanded={open} title={tooltip} aria-label={label ? undefined : tooltip} onclick={toggle}>
    {#if icon}<Icon name={icon} />{/if}
    {#if label}<span class="lbl">{label}</span>{/if}
    <svg class="i caret" aria-hidden="true"><use href="#i-down" /></svg>
  </button>
  {#if open}
    <div class="pop" class:marks role="menu" bind:this={panel} transition:pop>
      {#each sections as sec, i}
        {#if i}<hr>{/if}
        <!-- A section with a heading is a named group; the name is read with its items. -->
        {@const heading = sec.find((it) => it.heading)?.heading}
        <div class="group" role={heading ? 'group' : undefined} aria-label={heading}>
        {#each sec as it (it.key || it.heading || it.label)}
          {#if it.heading}
            <span class="heading" aria-hidden="true">{it.heading}</span>
          {:else}
            <button type="button" role={role(it)} aria-checked={it.radio || it.check ? !!it.checked : undefined} aria-keyshortcuts={it.kbd ? it.kbd.replaceAll('Ctrl', 'Control') : undefined} disabled={it.disabled} class:destructive={it.destructive} onclick={() => run(it)}>
              {#if marks}<span class="mark">{#if it.checked}<Icon name="check" />{/if}</span>{/if}
              <span class="txt">{it.label}</span>
            </button>
          {/if}
        {/each}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .menu { position: relative; display: inline-flex; }
  .lbl { max-width: 11rem; overflow: hidden; text-overflow: ellipsis; }
  .btn.icon .caret { display: none; }
  .btn[aria-expanded='true'] .caret { rotate: 180deg; }
  .btn.tinted, .btn.tinted.on { background: var(--c); color: var(--event-ink); }
  @media (hover: hover) { .btn.tinted:hover { filter: brightness(.96); } }
  .pop { position: absolute; z-index: 30; top: calc(100% + 4px); right: 0; transform-origin: top right; min-width: 12rem; max-width: calc(100vw - 16px); padding: 6px; display: flex; flex-direction: column; background: var(--card); border-radius: 12px; box-shadow: var(--shadow), 0 0 0 1px var(--line); }
  .pop button { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 10px; border: 0; border-radius: 6px; background: none; text-align: left; white-space: nowrap; }
  @media (hover: hover) { .pop button:hover, .pop button:focus-visible { background: var(--hover); outline: 0; } }
  .pop button:disabled { opacity: .45; background: none; cursor: default; }
  .pop button.destructive .txt { color: var(--red); }
  .group { display: flex; flex-direction: column; }
  .heading { padding: 4px 10px 2px; font-size: .78rem; font-weight: 600; color: var(--muted); }
  .pop.marks .heading { padding-left: 34px; }
  .mark { display: inline-flex; width: 16px; flex: none; color: var(--accent-fg); }
  .txt { flex: 1; overflow: hidden; text-overflow: ellipsis; }
  hr { margin: 6px 0; border: 0; border-top: 1px solid var(--line); }
  @media (prefers-reduced-motion: no-preference) {
    /* The caret turns with the menu. */
    .caret { transition: rotate var(--quick-close) var(--ease-close); }
    .btn[aria-expanded='true'] .caret { transition: rotate var(--quick-open) var(--ease-open); }
  }
</style>
