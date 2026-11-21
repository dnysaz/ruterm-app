// Vite emits CSS for lazily imported components (xterm, Monaco) as separate
// chunks and injects them at runtime. Tauri's WebView does not reliably apply
// those injected stylesheets (nor, in some builds, externally linked CSS
// chunks), so inline every emitted CSS file directly into index.html. This
// guarantees the styles are part of the initial document and cannot be lost
// to asset-protocol or injection timing issues.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, posix } from 'node:path';

const assetsDir = 'build/_app/immutable/assets';
const indexPath = 'build/index.html';

let html = readFileSync(indexPath, 'utf8');
const cssFiles = readdirSync(assetsDir)
  .filter((name) => name.endsWith('.css'))
  .sort();

// Drop existing stylesheet links so the same CSS is not loaded twice.
html = html.replace(
  /^\s*<link\b[^>]*href=["']\/?\.?\/?_app\/immutable\/assets\/[^"']+\.css["'][^>]*>\s*$/gim,
  '',
);

const inlineBlocks = cssFiles
  .map((name) => {
    let css = readFileSync(join(assetsDir, name), 'utf8');
    // Resolve relative url() references against the CSS file's original
    // location so fonts/icons still work once the CSS is inlined.
    css = css.replace(/url\((["']?)([^)"']+)\1\)/g, (match, quote, url) => {
      if (/^(data:|https?:|#)/i.test(url)) return match;
      const resolved = posix.resolve('/_app/immutable/assets/', url);
      return `url(${quote}${resolved}${quote})`;
    });
    css = css.replace(/<\/style/gi, '<\\/style');
    return `    <style data-inline-css="${name}">\n${css}\n    </style>`;
  })
  .join('\n');

html = html.replace('  </head>', `${inlineBlocks}\n  </head>`);
writeFileSync(indexPath, html);

console.log(`css inlined: ${cssFiles.length} stylesheet(s)`);
