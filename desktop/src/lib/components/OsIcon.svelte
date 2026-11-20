<script lang="ts">
  /* Official brand mark (or monogram) on a light "app icon" disc. */
  import { osLook } from '#lib/os';

  interface Props {
    os?: string | null;
    size?: number;
  }

  let { os = null, size = 30 }: Props = $props();

  const look = $derived(osLook(os));
  const radius = $derived(Math.round(size * 0.28));
</script>

{#if look.slug}
  <span
    class="disc photo"
    style="width:{size}px;height:{size}px;border-radius:{radius}px"
    title={look.name}
    aria-hidden="true"
  >
    <img
      src={`/os/${look.slug}.svg`}
      alt=""
      width={Math.round(size * 0.62)}
      height={Math.round(size * 0.62)}
      draggable="false"
    />
  </span>
{:else}
  <span
    class="disc mono"
    style="--from:{look.from};--to:{look.to};--fg:{look.fg};width:{size}px;height:{size}px;border-radius:{radius}px;font-size:{Math.round(size * 0.38)}px"
    title={look.name}
    aria-hidden="true"
  >{look.text}</span>
{/if}

<style>
  .disc {
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    user-select: none;
  }
  /* Light tray for official marks — brand colors stay true on any theme. */
  .photo {
    background: linear-gradient(165deg, #fdfdfb, #e8e6e1);
    border: 1px solid rgba(0, 0, 0, 0.28);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      0 1px 2px rgba(0, 0, 0, 0.35);
  }
  .mono {
    color: var(--fg);
    background: linear-gradient(155deg, var(--from), var(--to));
    border: 1px solid rgba(0, 0, 0, 0.3);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.35),
      inset 0 -8px 14px rgba(0, 0, 0, 0.18),
      0 1px 2px rgba(0, 0, 0, 0.3);
    font-weight: 700;
    letter-spacing: 0.01em;
  }
</style>
