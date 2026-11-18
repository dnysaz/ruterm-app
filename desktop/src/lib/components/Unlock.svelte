<script lang="ts">
  import Icon from './Icon.svelte';
  import {
    adoptRemote,
    app,
    createVault,
    signIn,
    unlock,
    unlockWithSavedKey,
  } from '$lib/state.svelte';

  const isSetup = $derived(!app.status?.has_vault);
  const signedIn = $derived(app.status?.signed_in === true);
  const accountEmail = $derived(app.status?.account?.email ?? '');

  let adopting = $state(false);
  let passphrase = $state('');
  let confirm = $state('');
  let localError = $state('');

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
        <span class="muted tiny">{accountEmail}</span>
      {:else}
        <button class="link" type="button" onclick={() => void signIn()}>
          Sign in with Google to sync
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
    border-radius: 18px;
    box-shadow: var(--shadow-strong);
  }
  h1 {
    margin: 0 0 10px;
    font-size: 27.5px;
  }
  .hint {
    margin: 0 0 24px;
    font-size: 15.5px;
    line-height: 1.65;
    color: var(--muted);
  }
  .full {
    width: 100%;
    margin-top: 4px;
  }
  .error {
    margin: 0 0 12px;
    font-size: 14.5px;
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
    font-size: 14.5px;
    line-height: 1.5;
    cursor: pointer;
  }
  .link:hover {
    color: var(--accent-strong);
    text-decoration: underline;
  }
  .tiny {
    font-size: 14px;
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
    font-size: 13.5px;
    color: var(--dim);
  }
  .or::before,
  .or::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--line);
  }
</style>
