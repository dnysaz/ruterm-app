<script lang="ts">
  import Icon from './Icon.svelte';
  import {
    adoptRemote,
    app,
    changeAccount,
    createVault,
    resetVault,
    setRememberKey,
    signIn,
    unlock,
    unlockWithSavedKey,
  } from '#lib/state.svelte';
  import GoogleIcon from './GoogleIcon.svelte';

  const isSetup = $derived(!app.status?.has_vault);
  const signedIn = $derived(app.status?.signed_in === true);
  const accountEmail = $derived(app.status?.account?.email ?? '');

  let adopting = $state(false);
  let passphrase = $state('');
  let confirm = $state('');
  let localError = $state('');
  let showRecovery = $state(false);
  let resetPassphrase = $state('');
  let resetConfirm = $state('');
  let resetToken = $state('');
  let resetError = $state('');

  const title = $derived(
    adopting ? 'Retrieve from the sync bridge' : isSetup ? 'Create your vault' : 'Unlock your vault',
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    localError = '';

    if (passphrase.length < 8) {
      localError = 'Use at least 8 characters.';
      return;
    }
    if (isSetup && !adopting && passphrase !== confirm) {
      localError = 'The passphrases do not match.';
      return;
    }

    if (adopting) {
      await adoptRemote(passphrase);
    } else if (isSetup) {
      await createVault(passphrase);
    } else {
      await unlock(passphrase);
    }

    passphrase = '';
    confirm = '';
  }

  function switchToAdopt() {
    adopting = true;
    passphrase = '';
    confirm = '';
    localError = '';
  }

  async function confirmReset() {
    resetError = '';
    if (resetPassphrase.length < 8) {
      resetError = 'Use at least 8 characters.';
      return;
    }
    if (resetPassphrase !== resetConfirm) {
      resetError = 'The passphrases do not match.';
      return;
    }
    if (resetToken !== 'RESET') {
      resetError = 'Type RESET exactly to confirm.';
      return;
    }
    await resetVault(resetPassphrase, resetToken);
    resetPassphrase = '';
    resetConfirm = '';
    resetToken = '';
    showRecovery = false;
  }
</script>

<div class="wrap">
  <form class="panel glass" onsubmit={submit}>
    <h1>{title}</h1>
    <p class="hint">
      {#if adopting}
        Enter the passphrase of the vault stored in this Google account. It will be
        downloaded and opened on this device.
      {:else if isSetup}
        This passphrase derives the encryption key with Argon2id. It never leaves
        this device — nobody can recover it for you.
      {:else}
        Enter your vault passphrase to decrypt the secrets stored on this device.
      {/if}
    </p>

    {#if !isSetup && !adopting && app.canAutoUnlock}
      <button
        class="btn lg full keychain"
        type="button"
        disabled={app.busy}
        onclick={() => void unlockWithSavedKey()}
      >
        <Icon name="lock" size={15} />
        Unlock with saved key
      </button>
      <div class="or"><span>or enter passphrase</span></div>
    {/if}

    <div class="field">
      <label for="pass">Passphrase</label>
      <input autocapitalize="off" autocorrect="off" spellcheck="false"
        id="pass"
        type="password"
        autocomplete={isSetup ? 'new-password' : 'current-password'}
        bind:value={passphrase}
        placeholder="At least 8 characters"
      />
    </div>

    {#if isSetup && !adopting}
      <div class="field">
        <label for="confirm">Repeat passphrase</label>
        <input autocapitalize="off" autocorrect="off" spellcheck="false" id="confirm" type="password" autocomplete="new-password" bind:value={confirm} />
      </div>
    {/if}

    {#if localError}
      <p class="error">{localError}</p>
    {/if}

    {#if !isSetup && !adopting}
      <div class="recovery">
        <button class="link" type="button" onclick={() => (showRecovery = !showRecovery)}>
          {showRecovery ? 'Hide recovery options' : "Can't unlock your vault?"}
        </button>
        {#if showRecovery}
          <div class="recovery-box">
            <p class="muted tiny">
              Your key may still be correct — older vaults are now read and
              re-sealed automatically. If a saved key is stale, forget it first.
              Resetting is the last resort: it deletes every host, identity and
              note on this device.
            </p>
            {#if app.canAutoUnlock}
              <button
                class="btn sm full"
                type="button"
                disabled={app.busy}
                onclick={() => void setRememberKey(false)}
              >
                Forget saved key
              </button>
            {/if}
            <div class="field">
              <label for="reset-pass">New passphrase</label>
              <input id="reset-pass" type="password" autocomplete="new-password" bind:value={resetPassphrase} />
            </div>
            <div class="field">
              <label for="reset-confirm">Repeat new passphrase</label>
              <input id="reset-confirm" type="password" autocomplete="new-password" bind:value={resetConfirm} />
            </div>
            <div class="field">
              <label for="reset-token">Type RESET to erase the old vault</label>
              <input id="reset-token" type="text" autocomplete="off" bind:value={resetToken} placeholder="RESET" />
            </div>
            {#if resetError}
              <p class="error">{resetError}</p>
            {/if}
            <button
              class="btn danger sm full"
              type="button"
              disabled={app.busy}
              onclick={() => void confirmReset()}
            >
              Erase data and create a new vault
            </button>
            <p class="muted tiny">
              If Google sync is on, sign out first — otherwise the old synced
              vault can come back on the next sync.
            </p>
          </div>
        {/if}
      </div>
    {/if}

    <button class="btn primary lg full" type="submit" disabled={app.busy}>
      {#if app.busy}
        Working…
      {:else if adopting}
        Retrieve vault
      {:else if isSetup}
        Create vault
      {:else}
        Unlock
      {/if}
    </button>

    <div class="foot">
      {#if adopting}
        <button
          class="link"
          type="button"
          onclick={() => {
            adopting = false;
            passphrase = '';
          }}
        >
          Create a new vault on this device
        </button>
      {:else if isSetup && signedIn}
        <button class="link" type="button" onclick={switchToAdopt}>
          Already have a vault on the bridge? Retrieve it
        </button>
      {:else if signedIn}
        <div class="account-line">
          Not {accountEmail}?
          <button class="link" type="button" disabled={app.busy} onclick={() => void changeAccount()}>
            Change account
          </button>
        </div>
      {:else}
        <button class="link google-link" type="button" disabled={app.busy} onclick={() => void signIn()}>
          <GoogleIcon size={16} />
          Continue with Google to sync
        </button>
      {/if}
    </div>
  </form>
</div>

<style>
  .wrap {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 62px 32px 32px;
  }
  .panel {
    width: min(430px, 100%);
    padding: 32px;
    border-radius: var(--radius-lg);
    border: 1px solid var(--glass-border);
    background: var(--panel);
  }
  h1 {
    margin: 0 0 8px;
    font-size: 18px;
    font-weight: 600;
  }
  .hint {
    margin: 0 0 22px;
    font-size: 13.5px;
    line-height: 1.65;
    color: var(--text-secondary);
  }
  .full {
    width: 100%;
    margin-top: 4px;
  }
  .error {
    margin: 0 0 12px;
    font-size: 13px;
    color: var(--bad);
  }
  .foot {
    margin-top: 18px;
    display: flex;
    justify-content: center;
    text-align: center;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    font-size: 13px;
    line-height: 1.5;
    cursor: pointer;
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 2px;
  }
  .link:hover {
    color: var(--accent-hover);
  }
  .tiny {
    font-size: 12px;
  }
  .keychain {
    width: 100%;
    margin-bottom: 4px;
  }
  .or {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 14px 0 16px;
    font-size: 12px;
    color: var(--text-tertiary);
  }
  .or::before,
  .or::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--line);
  }
  .recovery {
    margin: 16px 0 4px;
    text-align: center;
  }
  .recovery-box {
    margin-top: 12px;
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    text-align: left;
  }
  .recovery-box p {
    margin: 0 0 12px;
    line-height: 1.6;
  }
  .recovery-box .field {
    margin-bottom: 10px;
  }
  .recovery-box .field label {
    font-size: 12px;
  }
  .recovery-box .field input {
    width: 100%;
    box-sizing: border-box;
  }
  .account-line {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-tertiary);
  }
  .google-link {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
</style>
