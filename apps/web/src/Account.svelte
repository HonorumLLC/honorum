<script>
  import { onDestroy } from 'svelte';
  import { requestCode, verifyCode } from './lib/state.svelte.js';
  import { countries, formatPhoneInput, normalizePhone } from './lib/phone.js';
  import { dialogs } from './lib/ui.svelte.js';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';

  let country = $state('US'), number = $state(''), recipient = $state(''), code = $state('');
  let busy = $state(false), verifying = $state(false), error = $state('');
  let retryAt = $state(0), now = $state(Date.now()), lastPhone = $state('');
  let generation = 0, sending;
  const phoneId = $props.id();
  const callingCode = $derived(countries.find(item => item.code === country)?.callingCode);
  const phone = $derived(normalizePhone(number, country));
  const validCode = $derived(/^\d{6}$/.test(code));
  const wait = $derived(lastPhone === (recipient || phone) ? Math.max(0, Math.ceil((retryAt - now) / 1000)) : 0);

  $effect(() => {
    if (!dialogs.account) return;
    now = Date.now();
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(timer);
  });

  onDestroy(() => sending?.abort());

  function editNumber(event) {
    const input = event.currentTarget;
    const formatted = formatPhoneInput(input.value, country, input.selectionStart ?? input.value.length, number, event.inputType || '');
    country = formatted.country;
    number = input.value = formatted.value;
    input.setSelectionRange(formatted.caret, formatted.caret);
  }
  function changeCountry(event) {
    country = event.currentTarget.value;
    number = formatPhoneInput(number, country).value;
  }

  function cooldown(phone, seconds) {
    lastPhone = phone;
    now = Date.now();
    retryAt = now + seconds * 1000;
  }
  async function send() {
    const target = recipient || phone;
    if (busy || !target || wait) return;
    const attempt = ++generation;
    sending = new AbortController();
    busy = true;
    error = '';
    code = '';
    // A canceled or timed-out request may still send a text.
    cooldown(target, 30);
    try {
      const result = await requestCode(target, sending.signal);
      if (attempt !== generation) return;
      cooldown(target, result.retry_after);
      recipient = target;
    } catch (cause) {
      if (attempt !== generation) return;
      error = cause.message;
      if (cause.retryAfter) cooldown(target, cause.retryAfter);
    } finally {
      if (attempt === generation) busy = false;
    }
  }
  async function verify() {
    if (busy || !recipient || !validCode) return;
    busy = verifying = true;
    error = '';
    try {
      await verifyCode(recipient, code);
      dialogs.account = false;
    } catch (cause) {
      error = cause.message;
      code = '';
      if (cause.retryAfter) cooldown(recipient, cause.retryAfter);
    } finally {
      busy = verifying = false;
    }
  }
  function reset() {
    generation++;
    sending?.abort();
    recipient = '';
    code = '';
    error = '';
    busy = false;
  }
</script>

<Dialog bind:open={dialogs.account} kind="action" width="24rem"
  title={recipient ? 'Enter the Code' : 'Sign In'}
  ok={busy ? (verifying ? 'Signing In…' : 'Sending…') : recipient ? 'Sign In' : wait ? `Try Again in ${wait}s` : 'Send Code'}
  okDisabled={busy || (recipient ? !validCode : !phone || wait > 0)}
  dismissible={!verifying} onok={recipient ? verify : send} onclose={reset}>
  {#if !recipient}
    <p class="text">We’ll text you a sign-in code.</p>
    <div class="phone-field">
      <label for={phoneId}>Mobile number</label>
      <div class="phone-entry">
        <div class="dial-code">
          <span aria-hidden="true">{country} +{callingCode}<Icon name="down" size={14} /></span>
          <select aria-label="Country or region" value={country} onchange={changeCountry} disabled={busy} autocomplete="country">
            {#each countries as item (item.code)}<option value={item.code}>{item.name} (+{item.callingCode})</option>{/each}
          </select>
        </div>
        <!-- svelte-ignore a11y_autofocus -->
        <input id={phoneId} type="tel" value={number} oninput={editNumber} autocomplete="tel-national" placeholder={country === 'US' || country === 'CA' ? '(201) 555-0123' : 'Mobile number'} maxlength="40" disabled={busy} autofocus>
      </div>
    </div>
  {:else}
    <p class="text" role="status">Check {recipient} for your code. It may take a moment to arrive.</p>
    <!-- svelte-ignore a11y_autofocus -->
    <label class="field"><span>Code</span><input type="text" inputmode="numeric" autocomplete="one-time-code" maxlength="6" bind:value={code} oninput={(e) => { code = e.currentTarget.value.replace(/\D/g, ''); void verify(); }} placeholder="123456" disabled={busy} autofocus></label>
    <button type="button" class="btn link" onclick={send} disabled={busy || wait > 0}>{wait ? `Resend in ${wait}s` : 'Resend Code'}</button>
  {/if}
  <p class="text small">Standard message and data rates may apply.
    <a href="/legal.html#terms" target="_blank" rel="noopener">Terms</a> and <a href="/legal.html#privacy" target="_blank" rel="noopener">Privacy Policy</a>.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#snippet extra()}
    {#if recipient}<button type="button" class="btn flat" onclick={reset} disabled={busy}>Change Number</button>{/if}
  {/snippet}
</Dialog>

<style>
  .phone-field { margin-bottom: 12px; }
  .phone-field > label { display: block; margin-bottom: 4px; font-size: .8rem; font-weight: 500; color: var(--text2); }
  .phone-entry { display: flex; align-items: center; border-radius: 8px; background: var(--hover); }
  .phone-entry:focus-within { outline: 2px solid var(--accent-fg); outline-offset: -1px; }
  .phone-entry input { flex: 1; min-width: 0; width: 100%; min-height: 44px; padding: 8px 10px; border: 0; outline: 0; background: none; }
  .dial-code { position: relative; flex: none; border-right: 1px solid var(--line); }
  .dial-code > span { display: flex; align-items: center; gap: 5px; padding: 0 10px; min-height: 28px; font-size: .88rem; }
  .dial-code select { position: absolute; inset: -8px 0; width: 100%; height: 44px; opacity: 0; cursor: pointer; }
  .dial-code:focus-within { border-radius: 6px; background: var(--hover); }
  .text { margin-bottom: 12px; color: var(--text2); }
  .small { font-size: .8rem; }
  .link { margin: -4px 0 8px -4px; }
  .error { margin: 4px 0 10px; padding: 8px 10px; border-radius: 8px; background: var(--err-bg); color: var(--err-fg); font-size: .88rem; }
</style>
