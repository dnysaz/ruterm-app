<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import Icon from './Icon.svelte';

  type InfoPageKind = 'about' | 'terms' | 'privacy' | 'contact' | 'github';

  let { page, onclose }: { page: InfoPageKind; onclose?: () => void } = $props();

  let version = $state('');
  let copied = $state('');

  const META: Record<
    InfoPageKind,
    { title: string; sub: string; icon: 'info' | 'file-text' | 'shield' | 'mail' | 'code' }
  > = {
    about: { title: 'About RuTerm', sub: 'An open-source SSH workspace for the desktop.', icon: 'info' },
    terms: { title: 'Terms & Conditions', sub: 'The short version of the deal.', icon: 'file-text' },
    privacy: { title: 'Privacy Policy', sub: 'What stays on your machine, what (never) leaves it.', icon: 'shield' },
    contact: { title: 'Contact', sub: 'Bug reports, ideas and security questions.', icon: 'mail' },
    github: { title: 'GitHub', sub: 'Source code, releases and roadmap.', icon: 'code' },
  };
  const meta = $derived(META[page]);

  const CONTACT_EMAIL = 'danayasa2@gmail.com';
  const REPO_URL = 'https://github.com/dnysaz/ruterm-app';
  const BUILD_SNIPPET = 'git clone https://github.com/dnysaz/ruterm-app.git\ncd ruterm-app/desktop\nnpm install\nnpm run tauri dev';

  onMount(() => {
    window.getSelection()?.removeAllRanges();
    void api.appVersion().then(v => (version = v)).catch(() => (version = ''));
  });

  async function copy(label: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = label;
      setTimeout(() => (copied = ''), 1600);
    } catch {
      copied = '';
    }
  }
</script>

<div class="info-page">
  <header class="page-head">
    <span class="page-icon"><Icon name={meta.icon} size={19} /></span>
    <div class="titles">
      <h1>{meta.title}</h1>
      <p class="muted sub">{meta.sub}</p>
    </div>
    <span class="spacer"></span>
    {#if onclose}
      <button class="icon-btn" title="Close" aria-label="Close" onclick={onclose}>
        <Icon name="close" size={16} />
      </button>
    {/if}
  </header>

  <div class="scroll">
    <div class="column">

      {#if page === 'about'}
        <section class="section">
          <h3>What is RuTerm</h3>
          <p>
            RuTerm is an open-source SSH workspace for the desktop — hosts, keys and terminal
            preferences in one JSON document, sealed on your machine and synced through a
            <strong>zero-knowledge bridge</strong> you control. It is the Termius-class workflow
            without a subscription: no account server, no per-seat pricing, no vendor holding
            your credentials.
          </p>
          <p class="muted">
            RuTerm is not affiliated with, endorsed by, or connected to Termius or Termius
            Corporation. Any resemblance is intentional only in spirit: fast, beautiful,
            keyboard-first SSH.
          </p>
        </section>

        <section class="section">
          <h3>How it works</h3>
          <ul class="list">
            <li><strong>Local-first.</strong> The vault is a plain JSON file you can read, diff and back up.</li>
            <li><strong>Field-level encryption.</strong> Hosts and settings stay readable; passwords and private keys are sealed with <span class="mono">XChaCha20-Poly1305</span> + <span class="mono">Argon2id</span>.</li>
            <li><strong>Zero-knowledge sync.</strong> The optional bridge (Cloudflare Worker + D1) stores ciphertext and a revision counter only — the operator cannot read your credentials.</li>
            <li><strong>Native SSH.</strong> russh in Rust, PTY sessions streamed into xterm.js — real terminals, not emulated panes.</li>
            <li><strong>Reload-proof sessions.</strong> Tabs, splits and per-tab scrollback survive a restart; SSH reconnects below the restored text.</li>
          </ul>
        </section>

        <section class="section">
          <h3>Under the hood</h3>
          <div class="group">
            <div class="row">
              <span class="row-label">Frontend</span>
              <span class="value">Tauri v2 · Svelte 5 · xterm.js</span>
            </div>
            <div class="row">
              <span class="row-label">Engine</span>
              <span class="value mono">russh · Rust · Argon2id · XChaCha20-Poly1305</span>
            </div>
            <div class="row">
              <span class="row-label">UI type</span>
              <span class="value">SF Pro · macOS system font</span>
            </div>
            <div class="row">
              <span class="row-label">Terminal type</span>
              <span class="value">SF Mono · JetBrains Mono NF</span>
            </div>
            <div class="row">
              <span class="row-label">License</span>
              <span class="value">MIT — attribution appreciated</span>
            </div>
            <div class="row">
              <span class="row-label">Version</span>
              <span class="value mono">{version || '…'}</span>
            </div>
          </div>
        </section>

      {:else if page === 'terms'}
        <section class="section">
          <h3>1 · License</h3>
          <p>
            RuTerm is released under the MIT License. You may use, modify and redistribute it,
            commercially or otherwise, provided the copyright notice is preserved. The full
            license text ships in <span class="mono">LICENSE</span> in the repository.
          </p>
        </section>

        <section class="section">
          <h3>2 · Acceptable use</h3>
          <p>
            RuTerm is a tool for connecting to computers. You agree to use it only for systems
            you own or are explicitly authorized to administer. RuTerm grants you no rights to
            any server, and facilitating unauthorized access violates these terms and most laws.
          </p>
        </section>

        <section class="section">
          <h3>3 · Your vault, your responsibility</h3>
          <ul class="list">
            <li>Passwords and private keys are sealed in your vault with your vault password. If you lose the vault file or forget the password, the secrets are unrecoverable — by design. Keep your own backups.</li>
            <li>Refresh tokens and keys kept in the OS keychain are as safe as that keychain. Lock your machine.</li>
            <li>Agent forwarding can expose your keys to the remote host. Enable it per host, knowingly.</li>
          </ul>
        </section>

        <section class="section">
          <h3>4 · Sync bridge</h3>
          <p>
            Sync is opt-in and self-hosted. The hosted testing instance at
            <span class="mono">sync.lagitest.xyz</span> is experimental, may be reset, and is
            provided without guarantees. For production use, deploy your own bridge from
            <span class="mono">bridge/</span> — it takes minutes and costs nothing at hobby scale.
            The bridge only ever sees ciphertext; it cannot decrypt, and it cannot help you
            recover a lost vault password.
          </p>
        </section>

        <section class="section">
          <h3>5 · No warranty &amp; liability</h3>
          <p>
            RuTerm is provided “as is”, without warranty of any kind, express or implied —
            including merchantability, fitness for a particular purpose and non-infringement.
            SSH sessions can disconnect, commands can destroy data, and host keys can change
            under you. Review what you run; the authors are not liable for any damages arising
            from use of the software, to the maximum extent permitted by law.
          </p>
        </section>

        <section class="section">
          <h3>6 · Third parties</h3>
          <p>
            Google Sign-In is used only to establish an identity for sync; Google’s own terms
            and privacy policy apply when you sign in. Cloudflare hosts the bridge
            infrastructure. RuTerm is not affiliated with either company.
          </p>
        </section>

        <section class="section">
          <h3>7 · Changes</h3>
          <p>
            These terms evolve with the project. Material changes will be noted in the release
            notes. Continued use after a change means you accept the updated terms. If you
            disagree, stop using RuTerm and delete the vault file — nothing else is stored
            against your identity.
          </p>
        </section>

      {:else if page === 'privacy'}
        <section class="section">
          <h3>The short version</h3>
          <p>
            RuTerm does not collect telemetry, analytics or crash reports. Nothing leaves your
            machine unless you sign in to sync — and then it leaves only as ciphertext you
            encrypted on your device, to a bridge you can self-host.
          </p>
        </section>

        <section class="section">
          <h3>Stored on this device</h3>
          <ul class="list">
            <li><strong>Vault file</strong> (<span class="mono">vault.json</span>) — your hosts and settings in readable JSON; passwords and private keys are sealed, not readable.</li>
            <li><strong>Scrollback snapshots</strong> — the last lines of each terminal, in local storage as plain text. Never synced, erasable in Settings → Sessions, and deleted per tab when a tab closes.</li>
            <li><strong>Keychain entries</strong> — refresh token (sync) and SSH key material, kept by the operating system, never inside the vault.</li>
            <li><strong>Settings</strong> — theme, sidebar state, bridge URL. Local only, apart from what sync carries as vault fields.</li>
          </ul>
        </section>

        <section class="section">
          <h3>Sent to the bridge (only if you sign in)</h3>
          <ul class="list">
            <li>Your Google account email and device name — so the bridge knows which devices may pull revisions.</li>
            <li>The vault as an <em>opaque ciphertext blob</em> plus a revision number — the bridge cannot read hosts, passwords or keys.</li>
            <li>Google receives only the OAuth authorization exchange for sign-in, handled by the bridge; RuTerm requests <span class="mono">openid email profile</span> and nothing more.</li>
          </ul>
        </section>

        <section class="section">
          <h3>What never happens</h3>
          <ul class="list">
            <li>No telemetry, metrics, ads, A/B tests or “phone home” of any kind.</li>
            <li>No reading of your terminal output — scrollback never leaves the device.</li>
            <li>No sale, rental or sharing of data with data brokers. Ever.</li>
          </ul>
        </section>

        <section class="section">
          <h3>Your controls</h3>
          <ul class="list">
            <li>Use RuTerm fully offline in local mode — nothing is sent anywhere.</li>
            <li>Sign out anytime in Settings to stop this device syncing and drop its refresh token.</li>
            <li>Toggle scrollback off to erase stored snapshots; delete <span class="mono">vault.json</span> and the app state directory to erase everything else.</li>
          </ul>
          <p class="muted">Privacy questions: <span class="mono">{CONTACT_EMAIL}</span> (subject “privacy”).</p>
        </section>

      {:else if page === 'contact'}
        <section class="section">
          <h3>Where to reach us</h3>
          <div class="copy-row">
            <div>
              <div class="copy-label">Email</div>
              <div class="mono copy-value">{CONTACT_EMAIL}</div>
            </div>
            <button class="btn sm" onclick={() => copy('email', CONTACT_EMAIL)}>
              <Icon name={copied === 'email' ? 'check' : 'copy'} size={14} />
              {copied === 'email' ? 'Copied' : 'Copy'}
            </button>
          </div>
          <p class="muted fine">
            For bug reports and feature ideas the GitHub tracker is best once the repository
            goes public — include your RuTerm version (see About), OS version, and steps to
            reproduce. Screenshots help.
          </p>
        </section>

        <section class="section">
          <h3>Security reports</h3>
          <p>
            Found a vulnerability — command injection, keychain misuse, bridge auth bypass,
            crypto misuse? Email <span class="mono">{CONTACT_EMAIL}</span> with “security” in the
            subject. Please don’t file a public issue for undisclosed vulnerabilities. You will
            get an acknowledgement within a few days; credit is given to reporters who want it.
          </p>
        </section>

        <section class="section">
          <h3>Response times</h3>
          <p class="muted">
            This is a nights-and-weekends open-source project, not a support desk. Expect a
            human reply within days, not hours. Sync issues self-hosted on your own bridge are
            yours to debug — happy to review configurations though.
          </p>
        </section>

      {:else if page === 'github'}
        <section class="section">
          <h3>Open source on GitHub</h3>
          <p>
            RuTerm is developed in the open at <strong>github.com/dnysaz/ruterm-app</strong>.
            The full source — desktop app, Rust engine, and the sync bridge — is MIT
            licensed and free to use, fork and learn from.
          </p>
          <div class="copy-row">
            <div>
              <div class="copy-label">Repository</div>
              <div class="mono copy-value">{REPO_URL}</div>
            </div>
            <button class="btn sm" onclick={() => copy('repo', REPO_URL)}>
              <Icon name={copied === 'repo' ? 'check' : 'copy'} size={14} />
              {copied === 'repo' ? 'Copied' : 'Copy'}
            </button>
          </div>
        </section>

        <section class="section">
          <h3>Build from source</h3>
          <div class="code-block">
            <pre class="mono">{BUILD_SNIPPET}</pre>
            <button class="icon-btn copy-code" title="Copy commands" aria-label="Copy commands" onclick={() => copy('clone', BUILD_SNIPPET)}>
              <Icon name={copied === 'clone' ? 'check' : 'copy'} size={15} />
            </button>
          </div>
          <p class="muted fine">
            Requires Rust ≥ 1.80, Node ≥ 20 and the Tauri v2 prerequisites for your OS.
          </p>
        </section>

        <section class="section">
          <h3>What will be there</h3>
          <ul class="list">
            <li>MIT-licensed source: desktop app, Rust engine (<span class="mono">rterm-core</span>), and the sync bridge (<span class="mono">bridge/</span>).</li>
            <li>Roadmap: SSH-config import, SFTP browser, port forwarding, snippets, groups.</li>
            <li>Issues and discussions for bugs, ideas and contributions. Security reports go to <span class="mono">{CONTACT_EMAIL}</span>.</li>
          </ul>
          <p class="muted fine">Clone, build, star it if it helps. PRs welcome.</p>
        </section>
      {/if}

    </div>
  </div>
</div>

<style>
  .info-page {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    overflow: hidden;
  }

  .page-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 20px 28px 14px;
    background: var(--bg);
  }
  .page-icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 10px;
    border: 0;
    color: var(--accent);
    background: var(--accent-dim);
    flex: 0 0 auto;
  }
  .titles h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .sub {
    margin: 3px 0 0;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 4px 28px 48px;
  }
  .column {
    width: 100%;
    max-width: 680px;
    margin: 0 auto;
  }

  .section {
    margin-bottom: 26px;
  }
  .section h3 {
    margin: 0 0 8px 2px;
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--text-secondary);
    user-select: none;
  }
  .section p {
    margin: 0 0 10px;
    font-size: 13.5px;
    line-height: 1.7;
    color: var(--text);
    max-width: 66ch;
  }
  .section p:last-child {
    margin-bottom: 0;
  }
  .section p.muted {
    color: var(--text-secondary);
  }
  .fine {
    font-size: 12.5px;
    line-height: 1.65;
  }

  .list {
    margin: 0;
    padding-left: 1.2em;
    display: flex;
    flex-direction: column;
    gap: 7px;
    font-size: 13.5px;
    line-height: 1.65;
    max-width: 66ch;
  }

  .group {
    border-radius: 12px;
    background: var(--bg-elevated);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: baseline;
    gap: 16px;
    padding: 11px 16px;
  }
  .row + .row {
    border-top: 1px solid var(--line);
  }
  .row-label {
    flex: 0 0 132px;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .value {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    word-break: break-word;
  }

  .copy-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--bg-elevated);
    margin: 14px 0;
  }
  .copy-label {
    font-size: 12px;
    color: var(--text-tertiary);
    margin-bottom: 3px;
  }
  .copy-value {
    font-size: 13px;
    word-break: break-all;
  }

  .code-block {
    position: relative;
    border-radius: 12px;
    background: var(--bg-elevated);
    margin: 14px 0;
  }
  .code-block pre {
    margin: 0;
    padding: 16px 52px 16px 18px;
    overflow-x: auto;
    font-size: 12.5px;
    line-height: 1.7;
    color: var(--text);
    white-space: pre;
  }
  .copy-code {
    position: absolute;
    top: 8px;
    right: 8px;
    color: var(--text-tertiary);
  }

  @media (max-width: 720px) {
    .page-head {
      padding: 18px 20px 14px;
    }
    .scroll {
      padding: 4px 20px 44px;
    }
    .row {
      gap: 12px;
    }
    .row-label {
      flex-basis: 104px;
    }
  }
</style>
