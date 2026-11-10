<script lang="ts">
  import {
    errorText,
    getStatus,
    onHidden,
    onRelayStatus,
    subscribe,
    type RelayStatus,
    type Status,
  } from "./lib/ipc";
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
  // Relay events win over a fetch that was in flight when they arrived.
  let pendingRelay: RelayStatus | null = null;

  async function refresh() {
    try {
      status = await getStatus();
      if (pendingRelay) status.relay = pendingRelay;
      pendingRelay = null;
      error = "";
    } catch (e) {
      error = errorText(e);
    }
  }

  $effect(() =>
    subscribe(
      [
        onRelayStatus((relay) => {
          if (status) status.relay = relay;
          else pendingRelay = relay;
        }),
        // Leaving the Pairing tab drops the QR code (it contains the key).
        onHidden(() => {
          if (tab === "pairing") tab = "recent";
        }),
      ],
      async () => {
        await refresh();
        if (tab === "recent" && status && !status.paired) tab = "pairing";
      },
      (e) => (error = e),
    ),
  );

  type Tone = "ok" | "busy" | "off";
  const badge = $derived.by((): { tone: Tone; label: string; detail?: string } => {
    if (!status) return { tone: "off", label: "…" };
    if (!status.paired) return { tone: "off", label: "Not paired" };
    const relay = status.relay;
    if (!relay || relay.state === "connecting") return { tone: "busy", label: "Connecting" };
    if (relay.state === "connected") return { tone: "ok", label: "Connected" };
    return { tone: "busy", label: "Reconnecting", detail: `Retrying in ${relay.in_secs} s: ${relay.reason}` };
  });
</script>

<div class="app">
  <header>
    <div class="wrap">
      <div class="bar">
        <h1>OTP Forwarder</h1>
        <span class="pill {badge.tone}" role="status"><span class="dot" aria-hidden="true"></span>{badge.label}</span>
      </div>
      <p class="detail" role="status">{#if badge.detail}<small class="muted">{badge.detail}</small>{/if}</p>
      <nav class="tabs" aria-label="Sections">
        {#each tabs as [id, label] (id)}
          <button
            type="button"
            aria-controls="panel-{id}"
            aria-current={tab === id ? "page" : undefined}
            onclick={() => (tab = id)}>{label}</button
          >
        {/each}
      </nav>
    </div>
  </header>

  <div class="scroll">
    <main class="wrap">
      {#if error}<p class="error" role="alert">{error}</p>{/if}

      <section id="panel-recent" aria-label="Codes" hidden={tab !== "recent"}>
        <Recent {status} ongo={(t) => (tab = t)} />
      </section>
      <section id="panel-pairing" aria-label="Pairing" hidden={tab !== "pairing"}>
        <Pairing active={tab === "pairing"} {status} onrepaired={refresh} ongo={(t) => (tab = t)} />
      </section>
      <section id="panel-settings" aria-label="Settings" hidden={tab !== "settings"}><Settings /></section>
    </main>
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .wrap {
    max-width: 36rem;
    margin: 0 auto;
    padding: 0 var(--pico-spacing);
  }
  header {
    flex: none;
    padding-top: 0.75rem;
    border-bottom: 1px solid var(--pico-muted-border-color);
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
  }
  h1 {
    margin: 0;
    font-size: 1rem;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.1rem 0.6rem;
    border: 1px solid var(--pico-muted-border-color);
    border-radius: 999px;
    font-size: 0.8125rem;
    white-space: nowrap;
  }
  .dot {
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    background: var(--pico-muted-color);
  }
  .ok .dot {
    background: var(--pico-ins-color);
  }
  /* Amber reads on light and dark backgrounds; Pico has no amber variable. */
  .busy .dot {
    background: #e0a100;
  }
  .detail {
    margin: 0.2rem 0 0;
    line-height: 1.3;
    overflow-wrap: anywhere;
  }
  .detail:empty {
    margin: 0;
  }
  .tabs {
    display: flex;
    margin-top: 0.4rem;
  }
  .tabs button {
    flex: 1;
    margin: 0;
    padding: 0.5rem 0.25rem;
    border: 0;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    color: var(--pico-muted-color);
    font-weight: 500;
  }
  .tabs button:hover {
    color: var(--pico-color);
  }
  .tabs button:focus:not(:focus-visible) {
    box-shadow: none;
  }
  .tabs button[aria-current="page"] {
    border-bottom-color: var(--pico-primary);
    color: var(--pico-primary);
    font-weight: 600;
  }
  main.wrap {
    padding-top: 1rem;
    padding-bottom: 1.5rem;
  }
</style>
