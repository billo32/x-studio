<script>
  import Connect from "./lib/Connect.svelte";
  import Shell from "./lib/Shell.svelte";

  let status = $state(null);
</script>

{#if status}
  <Shell {status} onDisconnect={() => (status = null)} />
{:else}
  <Connect onConnected={(s) => (status = s)} />
{/if}
