<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { installExternalLinkHandler } from '$lib/externalLink.svelte';
  import ExternalLinkConfirm from '$lib/components/ExternalLinkConfirm.svelte';

  let { children } = $props();

  onMount(installExternalLinkHandler);
</script>

{@render children()}
<ExternalLinkConfirm />
