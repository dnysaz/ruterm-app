<script lang="ts">
  import { app, signIn } from '#lib/state.svelte';

  function continueLocal() {
    app.screen = app.status?.has_vault ? 'unlock' : 'setup';
  }

  const account = $derived(app.status?.account ?? null);
</script>

<div class="wrap">
  <div class="grid">
    <section class="pitch">
      <div class="kicker">RuTerm<span class="caret" aria-hidden="true">_</span></div>
      <h1>A vault for your servers.</h1>
      <p class="lede">
        Hosts, keys and terminal preferences live in a single JSON file. Passwords
        and private keys stay sealed with your passphrase; the readable document
        syncs through the RuTerm bridge — end to end encrypted, stored as ciphertext.
      </p>
      <dl class="facts">
        <div>
          <dt>Storage</dt>
          <dd>One JSON document per account</dd>
        </div>
        <div>
          <dt>Encryption</dt>
          <dd>XChaCha20-Poly1305 · Argon2id</dd>
        </div>
        <div>
          <dt>Sync</dt>
          <dd>RuTerm bridge — zero config</dd>
        </div>
      </dl>
    </section>

    <section class="panel glass">
      {#if account}
        <div class="account">
          <span class="avatar">{account.email.slice(0, 1).toUpperCase()}</span>
          <div>
            <div class="email">{account.email}</div>
            <div class="muted small">Google account connected</div>
          </div>
        </div>
        <button class="btn primary lg full" onclick={continueLocal}>Continue</button>
      {:else}
        <h2>Get started</h2>
        <p class="muted body">
          Sign in to sync your vault across devices, or work locally and add an
          account later.
        </p>
        <button class="btn primary lg full" disabled={app.busy} onclick={() => void signIn()}>
          {app.busy ? 'Waiting for browser…' : 'Sign in with Google'}
        </button>
        <button class="btn lg full" onclick={continueLocal}>Continue without an account</button>
      {/if}

      <hr class="hairline" />
      <p class="fine">
        Sign-in uses OAuth PKCE through your system browser. No client secret,
        and only the <span class="mono">openid email profile</span> scope.
      </p>
    </section>
  </div>
</div>

<style>
  .wrap {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 70px 40px 40px;
  }
  .grid {
    width: min(1080px, 100%);
    display: grid;
    grid-template-columns: minmax(0, 1.15fr) minmax(340px, 0.85fr);
    gap: 68px;
    align-items: center;
  }
  .kicker {
    font-family: var(--term);
    font-size: 13.5px;
    font-weight: 700;
    letter-spacing: 0.02em;
    color: var(--accent);
    margin-bottom: 18px;
  }
  .caret {
    animation: caret 1.15s step-end infinite;
  }
  @keyframes caret {
    0%,
    55% {
      opacity: 1;
    }
    56%,
    100% {
      opacity: 0;
    }
  }
  h1 {
    margin: 0 0 16px;
    font-size: clamp(26px, 3vw, 32px);
    line-height: 1.12;
    font-weight: 600;
    letter-spacing: -0.035em;
  }
  .lede {
    margin: 0;
    max-width: 50ch;
    font-size: 14px;
    line-height: 1.7;
    color: var(--text-secondary);
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 22px;
    margin: 40px 0 0;
    padding-top: 26px;
    border-top: 1px solid var(--line);
  }
  .facts dt {
    font-size: 11px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--text-tertiary);
    margin-bottom: 6px;
  }
  .facts dd {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text);
  }
  .panel {
    border-radius: var(--radius-lg);
    padding: 28px;
    border: 1px solid var(--glass-border);
    background: var(--panel);
  }
  h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .body {
    margin: 0 0 20px;
    font-size: 13.5px;
    line-height: 1.6;
    color: var(--text-secondary);
  }
  .full {
    width: 100%;
    margin-bottom: 10px;
  }
  .hairline {
    margin: 20px 0 16px;
  }
  .fine {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-tertiary);
  }
  .account {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 22px;
  }
  .avatar {
    width: 38px;
    height: 38px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--panel-hover);
    border: 1px solid var(--line-hover);
    font-size: 14px;
    font-weight: 600;
  }
  .email {
    font-size: 13.5px;
    font-weight: 550;
  }
  .small {
    font-size: 12px;
  }

  @media (max-width: 880px) {
    .wrap {
      padding: 28px;
      place-items: start center;
    }
    .grid {
      grid-template-columns: 1fr;
      gap: 34px;
    }
    .facts {
      display: none;
    }
  }
</style>
