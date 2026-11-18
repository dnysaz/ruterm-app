// Static pages served by the Worker: landing page + privacy policy.
// Both carry the RuTerm identity (burnished orange on matte black).

const BASE = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{{TITLE}} — RuTerm</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    background: #131110; color: #f4f1ec;
    font: 16px/1.65 -apple-system, BlinkMacSystemFont, "Helvetica Neue", system-ui, sans-serif;
    padding: 48px 20px;
  }
  main { max-width: 640px; }
  .wordmark { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-weight: 700;
              font-size: 28px; letter-spacing: -0.02em; }
  .cursor { color: #f74c00; animation: blink 1.15s step-end infinite; }
  @keyframes blink { 0%, 55% { opacity: 1; } 56%, 100% { opacity: 0; } }
  h1 { font-size: 28px; letter-spacing: -0.02em; margin: 28px 0 8px; }
  p { color: #a39d95; }
  a { color: #ff6b1f; text-decoration: none; }
  a:hover { text-decoration: underline; }
  code { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.9em;
         background: rgba(255,236,222,.06); border: 1px solid rgba(255,236,222,.12);
         padding: 1px 6px; border-radius: 6px; }
  hr { border: 0; border-top: 1px solid rgba(255,236,222,.1); margin: 32px 0; }
  .muted { color: #6f6a63; font-size: 13px; }
  ul { color: #a39d95; padding-left: 20px; }
  li { margin: 6px 0; }
</style>
</head>
<body><main>{{BODY}}</main></body></html>`;

function page(title: string, body: string): Response {
  const html = BASE.replace('{{TITLE}}', title).replace('{{BODY}}', body);
  return new Response(html, {
    headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'public, max-age=3600' },
  });
}

export function landing(): Response {
  return page('Open-source SSH client', `
<p class="wordmark">RuTerm<span class="cursor">_</span></p>
<h1>Your SSH hosts, sealed.</h1>
<p>RuTerm is an open-source, privacy-first SSH client for macOS built with Tauri
and Rust. Vaults are encrypted end-to-end on your device — this domain exists only as
a zero-knowledge sync bridge, so your terminal sessions and credentials follow you
without ever being readable by anyone else.</p>
<ul>
  <li>Field-level encryption: only passwords and private keys are sealed (XChaCha20-Poly1305, Argon2id).</li>
  <li>One JSON vault, kept in sync across devices — or offline, entirely local.</li>
  <li>Nothing to configure. Sign in with Google and you are synced.</li>
</ul>
<hr />
<p class="muted">Source: <a href="https://github.com/dnysaz/ruterm-app">github.com/dnysaz/ruterm-app</a> &middot;
<a href="/privacy">Privacy policy</a> &middot; <a href="/health">Status</a></p>
`);
}

export function privacy(): Response {
  return page('Privacy', `
<p class="wordmark">RuTerm<span class="cursor">_</span></p>
<h1>Privacy policy</h1>
<p class="muted">Last updated: November 2026</p>
<p>The RuTerm sync bridge ("the service") is operated as a minimal,
zero-knowledge store. This page explains what the service does and does not see.</p>
<ul>
  <li><strong>Account.</strong> When you sign in with Google, the service receives your
      Google account identifier (<code>sub</code>), email address and display name. It is
      used solely to authenticate you and identify your devices.</li>
  <li><strong>Vault data.</strong> Your vault is encrypted on your device before upload.
      The service stores ciphertext, a revision counter and timestamps. It cannot decrypt,
      inspect or scan your hosts, credentials, keys, notes or terminal output.</li>
  <li><strong>Devices.</strong> The service records the device name, platform and app
      version you register, so you can see and revoke sessions.</li>
  <li><strong>Refresh tokens.</strong> Stored only as SHA-256 hashes, rotated on every use,
      and never readable by anyone — including the operator.</li>
  <li><strong>Retention &amp; deletion.</strong> You can delete your account and all
      associated data at any time from the app (Settings → Google and sync), or request it
      via the repository. Deletion removes the account, devices and vault copies, and is
      irreversible.</li>
  <li><strong>Sharing.</strong> Data is never sold, shared or used for analytics, ads or
      model training. Google is involved only in the sign-in flow; Google's own privacy
      policy applies to that portion of the process.</li>
  <li><strong>Changes.</strong> This policy may be updated; material changes will be noted
      in the app release notes.</li>
</ul>
<hr />
<p class="muted"><a href="/">Back</a> · Questions: open an issue on the project repository.</p>
`);
}
