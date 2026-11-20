<script lang="ts">
  /// Settings section for the home backdrop. Self-contained: delete this file,
  /// the two lib imports in Settings.svelte and the `style` binding on
  /// `.empty-main` in VaultShell.svelte to remove the feature entirely.
  import {
    BACKGROUND_IMAGES,
    BACKGROUND_PATTERNS,
    type BackgroundMode,
  } from '#lib/background';
  import {
    background,
    backgroundCustom,
    clearBackgroundCustom,
    selectBackgroundCustom,
    setBackgroundCustom,
    setBackgroundDim,
    setBackgroundImage,
    setBackgroundMode,
    setBackgroundPattern,
    setBackgroundTermDim,
  } from '#lib/background.svelte';

  const MODES: { value: BackgroundMode; label: string }[] = [
    { value: 'none', label: 'Off' },
    { value: 'image', label: 'Photo' },
    { value: 'pattern', label: 'Pattern' },
  ];

  const MAX_EDGE = 2048;
  const JPEG_QUALITY = 0.85;

  let fileInput = $state<HTMLInputElement | null>(null);
  let dragging = $state(false);
  let error = $state('');
  let notice = $state('');
  // "Your image" toggle: reveals the form; turning it on also re-activates a
  // previously uploaded image, turning it off reverts to the plain backdrop.
  let yourImageOn = $state(background.mode === 'image' && background.imageId === 'custom');

  function toggleYourImage(event: Event) {
    const on = (event.currentTarget as HTMLInputElement).checked;
    yourImageOn = on;
    if (on) {
      if (backgroundCustom.src) selectBackgroundCustom();
    } else if (background.mode === 'image' && background.imageId === 'custom') {
      setBackgroundMode('none');
    }
  }

  function acceptFile(file: File) {
    if (!file.type.startsWith('image/')) {
      error = 'That file is not an image.';
      return;
    }
    error = '';
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result ?? '');
      const img = new Image();
      img.onload = () => {
        const scale = Math.min(1, MAX_EDGE / Math.max(img.naturalWidth, img.naturalHeight));
        let src = dataUrl;
        if (scale < 1) {
          const canvas = document.createElement('canvas');
          canvas.width = Math.max(1, Math.round(img.naturalWidth * scale));
          canvas.height = Math.max(1, Math.round(img.naturalHeight * scale));
          const ctx = canvas.getContext('2d');
          if (ctx) {
            ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
            src = canvas.toDataURL(file.type === 'image/png' ? 'image/png' : 'image/jpeg', JPEG_QUALITY);
          }
        }
        notice = setBackgroundCustom(src)
          ? ''
          : 'Image kept for this session — it was too large to save permanently.';
      };
      img.onerror = () => {
        error = 'Could not read that image.';
      };
      img.src = dataUrl;
    };
    reader.onerror = () => {
      error = 'Could not read that file.';
    };
    reader.readAsDataURL(file);
  }

  function onPickFile(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (file) acceptFile(file);
  }

  function onDropFile(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    const file = event.dataTransfer?.files?.[0];
    if (file) acceptFile(file);
  }

</script>

<section class="section">
  <h2 class="section-title">Background</h2>
  <div class="group">
    <div class="row">
      <span class="row-label">Backdrop</span>
      <div class="segmented-lg" role="group" aria-label="Background">
        {#each MODES as mode (mode.value)}
          <button
            class="segment-lg"
            class:on={background.mode === mode.value}
            type="button"
            onclick={() => setBackgroundMode(mode.value)}
          >
            {mode.label}
          </button>
        {/each}
      </div>
    </div>

    {#if background.mode === 'image'}
      <div class="bg-thumbs">
        {#each BACKGROUND_IMAGES as img (img.id)}
          <button
            class="bg-thumb"
            class:on={background.imageId === img.id}
            type="button"
            title={img.label}
            aria-pressed={background.imageId === img.id}
            onclick={() => setBackgroundImage(img.id)}
          >
            <img src={img.src} alt={img.label} draggable="false" loading="lazy" />
            <span class="bg-name">{img.label}</span>
          </button>
        {/each}
        {#if backgroundCustom.src}
          <div class="bg-thumb custom-wrap" class:on={background.imageId === 'custom'}>
            <button
              class="thumb-hit"
              type="button"
              title="Your image"
              aria-pressed={background.imageId === 'custom'}
              onclick={selectBackgroundCustom}
            >
              <img src={backgroundCustom.src} alt="Your uploaded wallpaper" draggable="false" />
              <span class="bg-name">Custom</span>
            </button>
            <button
              class="thumb-remove"
              type="button"
              title="Remove custom image"
              aria-label="Remove custom image"
              onclick={clearBackgroundCustom}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true">
                <path d="M7 7l10 10M17 7L7 17" />
              </svg>
            </button>
          </div>
        {/if}
      </div>
      <div class="upload-form">
        <label class="upload-head-row">
          <div class="cell">
            <div class="strong">Your image</div>
            <small>Use your own photo as the window backdrop. Stored locally — never leaves this Mac.</small>
          </div>
          <input
            type="checkbox"
            class="switch"
            checked={yourImageOn}
            onchange={toggleYourImage}
            aria-label="Use your own image"
          />
        </label>
        {#if yourImageOn}
          <button
            type="button"
            class="dropzone"
            class:dragging
            onclick={() => fileInput?.click()}
            ondragover={event => {
              event.preventDefault();
              dragging = true;
            }}
            ondragleave={() => (dragging = false)}
            ondrop={onDropFile}
          >
            <svg class="dz-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="3" y="4" width="18" height="16" rx="3" />
              <circle cx="9" cy="10" r="1.6" />
              <path d="M4.5 17l4.5-4.5 4 4 3.5-3.5L21 17.5" />
            </svg>
            <span class="dz-title">{dragging ? 'Drop to use this photo' : 'Drag & drop a photo here'}</span>
            <span class="dz-sub">or <span class="dz-browse">browse your Mac…</span></span>
          </button>
          {#if backgroundCustom.src}
            <button class="btn ghost remove-custom" type="button" onclick={clearBackgroundCustom}>
              Remove current custom image
            </button>
          {/if}
          {#if error}<p class="hint error">{error}</p>{/if}
          {#if notice}<p class="hint">{notice}</p>{/if}
        {/if}
      </div>
      <input
        bind:this={fileInput}
        type="file"
        accept="image/*"
        class="visually-hidden"
        aria-hidden="true"
        tabindex="-1"
        onchange={onPickFile}
      />
      <div class="row dim-row">
        <div class="cell">
          <div class="strong">Frost</div>
          <small>How much the card behind the text blurs and tints. The photo stays sharp.</small>
        </div>
        <input
          class="range"
          type="range"
          min="0"
          max="85"
          step="5"
          value={background.dim}
          style:--fill={`${(background.dim / 85) * 100}%`}
          oninput={event => setBackgroundDim(Number(event.currentTarget.value))}
          aria-label="Frost strength"
        />
        <span class="dim-value mono">{background.dim}%</span>
      </div>
    {:else if background.mode === 'pattern'}
      <div class="bg-thumbs patterns">
        {#each BACKGROUND_PATTERNS as pattern (pattern.id)}
          <button
            class="bg-thumb pattern"
            class:on={background.patternId === pattern.id}
            type="button"
            title={pattern.label}
            aria-pressed={background.patternId === pattern.id}
            onclick={() => setBackgroundPattern(pattern.id)}
          >
            <span
              class="swatch"
              style:background-image={pattern.image}
              style:background-size={pattern.size}
            ></span>
            <span class="bg-name">{pattern.label}</span>
          </button>
        {/each}
      </div>
    {/if}

    {#if background.mode !== 'none'}
      <div class="row dim-row">
        <div class="cell">
          <div class="strong">Terminal</div>
          <small>Lets the backdrop bleed through the terminal area. Lower = more background visible.</small>
        </div>
        <input
          class="range"
          type="range"
          min="0"
          max="85"
          step="5"
          value={background.termDim}
          style:--fill={`${(background.termDim / 85) * 100}%`}
          oninput={event => setBackgroundTermDim(Number(event.currentTarget.value))}
          aria-label="Terminal backdrop visibility"
        />
        <span class="dim-value mono">{background.termDim}%</span>
      </div>
    {/if}
  </div>
  <p class="hint">
    Applies to the whole window — the sidebar goes translucent over it. Bundled photos are free-license
    images (<a href="https://unsplash.com">Unsplash</a>); your own uploads stay on this Mac.
    Patterns adapt to the active theme.
  </p>
</section>

<style>
  /* ----------------------- same grouped-list primitives as Settings ------ */
  .section-title {
    margin: 0 0 10px 4px;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.015em;
    color: var(--text);
    user-select: none;
  }
  .group {
    border-radius: 12px;
    background: var(--bg-elevated);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 16px;
    min-height: 50px;
    padding: 12px 16px;
    margin: 0;
  }
  .row + .row {
    border-top: 1px solid var(--line);
  }
  .row-label {
    flex: 0 0 120px;
    margin: 0;
    font-size: 13px;
    font-weight: 400;
    letter-spacing: -0.01em;
    color: var(--text-secondary);
  }
  .cell {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .strong {
    font-size: 13.5px;
    font-weight: 500;
    color: var(--text);
    letter-spacing: -0.01em;
  }
  .cell small {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-tertiary);
  }
  .hint {
    margin: 9px 4px 0;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--text-tertiary);
  }

  /* -------------------------------------- Backdrop segmented (large) ------ */
  .segmented-lg {
    display: flex;
    gap: 3px;
    padding: 3px;
    flex: 1;
    border-radius: 11px;
    background: color-mix(in srgb, var(--text) 9%, transparent);
    user-select: none;
  }
  .segment-lg {
    flex: 1;
    min-width: 64px;
    height: 33px;
    padding: 0 16px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
    cursor: pointer;
    transition:
      background 0.15s ease,
      color 0.15s ease;
  }
  .segment-lg:hover {
    color: var(--text);
  }
  .segment-lg.on {
    background: color-mix(in srgb, var(--text) 22%, transparent);
    color: var(--text);
    font-weight: 600;
  }
  .segment-lg:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* ------------------------------------------- image / pattern picker ------ */
  .bg-thumbs {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 16px;
    padding: 18px;
    border-top: 1px solid var(--line);
  }
  .bg-thumb {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 9px;
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 16px;
    background: var(--panel);
    cursor: pointer;
    outline: none;
    transition:
      border-color 0.14s ease,
      background 0.14s ease;
  }
  .bg-thumb:hover {
    background: var(--panel-hover);
    border-color: var(--line-hover);
  }
  .bg-thumb:focus-visible {
    border-color: var(--accent);
    outline: 2px solid var(--accent-dim);
    outline-offset: 1px;
  }
  .bg-thumb.on {
    border-color: var(--accent);
    background: var(--accent-dim);
  }
  /* flat check badge, like the Photos selection mark */
  .bg-thumb.on::after {
    content: '';
    position: absolute;
    top: 13px;
    right: 13px;
    width: 21px;
    height: 21px;
    border-radius: 50%;
    background: var(--accent)
      url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none'%3E%3Cpath d='M5.5 13.2l4.2 4.2L18.5 8.6' stroke='white' stroke-width='2.6' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E")
      center/12px no-repeat;
  }
  /* ------------------------- custom image thumb with small × to delete --- */
  .custom-wrap {
    position: relative;
    cursor: default;
  }
  .custom-wrap.on::after {
    display: none; /* the × occupies the corner */
  }
  .thumb-hit {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 9px;
    width: 100%;
    height: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .thumb-hit:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
    border-radius: 12px;
  }
  .thumb-remove {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(15, 15, 18, 0.55);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    color: #ffffff;
    cursor: pointer;
    transition:
      background 0.15s ease,
      transform 0.15s ease;
  }
  .thumb-remove:hover {
    background: rgba(15, 15, 18, 0.8);
  }
  .thumb-remove:active {
    transform: scale(0.9);
  }
  .thumb-remove:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .thumb-remove svg {
    width: 11px;
    height: 11px;
  }

  .bg-thumb img {
    display: block;
    width: 100%;
    aspect-ratio: 16 / 10;
    object-fit: cover;
    border-radius: 10px;
    background: var(--bg-elevated);
    user-select: none;
    pointer-events: none;
  }
  .bg-name {
    font-size: 12px;
    font-weight: 500;
    letter-spacing: -0.005em;
    color: var(--text-secondary);
    text-align: center;
    padding: 1px 0 5px;
    user-select: none;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .bg-thumb.on .bg-name {
    color: var(--text);
  }
  .bg-thumbs.patterns {
    grid-template-columns: repeat(6, 1fr);
    gap: 10px;
    padding: 14px;
    max-width: none;
  }
  @media (max-width: 760px) {
    .bg-thumbs.patterns {
      gap: 8px;
      padding: 12px;
    }
  }
  .swatch {
    display: block;
    width: 100%;
    aspect-ratio: 16 / 10;
    border-radius: 10px;
    border: 1px solid var(--line);
    background-color: var(--bg);
    pointer-events: none;
  }
  /* --------------------------------------- your own image (upload only) --- */
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    opacity: 0;
    pointer-events: none;
  }
  .upload-form {
    border-top: 1px solid var(--line);
    padding: 14px 16px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .upload-head-row {
    display: flex;
    align-items: center;
    gap: 16px;
    cursor: pointer;
    user-select: none;
    -webkit-user-select: none;
  }
  .remove-custom {
    align-self: flex-start;
  }
  .dropzone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 7px;
    min-height: 132px;
    padding: 16px 14px;
    border: 1.5px dashed var(--line-hover);
    border-radius: 12px;
    background: color-mix(in srgb, var(--text) 3%, transparent);
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      background 0.15s ease,
      color 0.15s ease;
  }
  .dropzone:hover {
    border-color: var(--accent);
    color: var(--text);
  }
  .dropzone.dragging {
    border-color: var(--accent);
    background: var(--accent-dim);
    color: var(--text);
  }
  .dropzone:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .dz-icon {
    width: 30px;
    height: 30px;
    color: var(--accent);
  }
  .dz-title {
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .dz-sub {
    font-size: 12px;
  }
  .dz-browse {
    color: var(--accent);
    text-decoration: underline;
  }
  .hint.error {
    color: var(--bad);
  }

  /* -------------------------------------------- macOS-style switch toggle */
  .switch {
    appearance: none;
    -webkit-appearance: none;
    flex: 0 0 auto;
    width: 40px;
    height: 22px;
    margin: 0;
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 16%, transparent);
    position: relative;
    outline: none;
    cursor: pointer;
    transition: background 0.2s ease;
  }
  .switch::after {
    content: '';
    position: absolute;
    top: 3px;
    left: 3px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #ffffff;
    transition:
      transform 0.2s cubic-bezier(0.34, 1.56, 0.64, 1),
      width 0.15s ease;
  }
  .switch:active::after {
    width: 19px;
  }
  .switch:checked {
    background: var(--accent);
  }
  .switch:checked:hover {
    background: var(--accent-hover);
  }
  .switch:checked::after {
    left: 3px;
    top: 3px;
    width: 16px;
    height: 16px;
    border: 0;
    transform: translateX(18px);
    background: var(--accent-on);
  }
  .switch:checked:active::after {
    transform: translateX(15px);
    width: 19px;
  }
  .switch:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch::after {
      transition: none;
    }
  }

  /* ----------------------------------------------- macOS-style slider ----- */
  .dim-row {
    border-top: 1px solid var(--line);
    padding-top: 16px;
    padding-bottom: 16px;
    min-height: 58px;
  }
  .dim-row .cell {
    gap: 4px;
  }
  .dim-row .cell small {
    max-width: 46ch;
    line-height: 1.55;
  }
  .range {
    -webkit-appearance: none;
    appearance: none;
    flex: 0 0 auto;
    width: 172px;
    height: 22px;
    margin: 0;
    background: transparent;
    cursor: pointer;
    --fill: 40%;
  }
  .range::-webkit-slider-runnable-track {
    height: 5px;
    border-radius: 999px;
    background:
      linear-gradient(to right, var(--accent) var(--fill), transparent var(--fill)),
      color-mix(in srgb, var(--text) 15%, transparent);
  }
  .range::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 18px;
    height: 18px;
    margin-top: -6.5px;
    border-radius: 50%;
    background: #ffffff;
    border: 0.5px solid rgba(0, 0, 0, 0.18);
  }
  .range::-moz-range-track {
    height: 5px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 15%, transparent);
  }
  .range::-moz-range-progress {
    height: 5px;
    border-radius: 999px;
    background: var(--accent);
  }
  .range::-moz-range-thumb {
    width: 17px;
    height: 17px;
    border-radius: 50%;
    background: #ffffff;
    border: 0.5px solid rgba(0, 0, 0, 0.18);
  }
  .range:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 4px;
    border-radius: 999px;
  }
  .dim-value {
    min-width: 40px;
    text-align: right;
    color: var(--text-secondary);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 760px) {
    .bg-thumbs {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>
