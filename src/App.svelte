<script lang="ts">
  import { errorText, getStatus, onRelayStatus, type Status } from "./lib/ipc";
  import Recent from "./lib/Recent.svelte";
  import Pairing from "./lib/Pairing.svelte";
  import Settings from "./lib/Settings.svelte";

  type Tab = "recent" | "pairing" | "settings";
  const tabs: [Tab, string][] = [
    ["recent", "Codes"],
    ["pairing", "Pairing"],
    ["settings", "Settings"],
  ];

  let status = $state<Status | null>(null);
  let tab = $state<Tab>("recent");
  let error = $state("");

  async function refresh() {
    try {
      status = await getStatus();
    } catch (e) {
      error = errorText(e);
    }
  }

  $effect(() => {
    refresh().then(() => {
      if (status && !status.paired) tab = "pairing";
    });
    return onRelayStatus((relay) => {
      if (status) status.relay = relay;
    });
  });

  const badge = $derived.by(() => {
    if (!status) return "…";
    if (!status.paired) return "Not paired";
    const relay = status.relay;
    if (!relay || relay.state === "connecting") return "Connecting";
    if (relay.state === "connected") return "Connected";
    return `Reconnecting in ${relay.in_secs} s: ${relay.reason}`;
  });
</script>

<main class="container">
  <header>
    <strong>OTP Forwarder</strong>
    <small class="badge" class:ok={badge === "Connected"} role="status">{badge}</small>
  </header>
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <nav>
    <ul>
      {#each tabs as [id, label] (id)}
        <li>
          <button
            type="button"
            class:outline={tab !== id}
            aria-current={tab === id ? "page" : undefined}
            onclick={() => (tab = id)}>{label}</button
          >
        </li>
      {/each}
    </ul>
  </nav>

  <section hidden={tab !== "recent"}><Recent /></section>
  <section hidden={tab !== "pairing"}><Pairing onrepaired={refresh} /></section>
  <section hidden={tab !== "settings"}><Settings /></section>
</main>

<style>
  main {
    padding-top: 0.75rem;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
  }
  .badge {
    color: var(--pico-muted-color);
    text-align: right;
  }
  .badge.ok {
    color: var(--pico-ins-color);
  }
  nav button {
    padding: 0.3rem 0.8rem;
  }
  .error {
    color: var(--pico-del-color);
  }
</style>
