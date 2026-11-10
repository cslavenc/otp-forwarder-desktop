<script lang="ts">
  import { errorText, getPairingInfo, pairingQr, repair, type PairingInfo, type Status } from "./ipc";

  let {
    active,
    status,
    onrepaired,
    ongo,
  }: { active: boolean; status: Status | null; onrepaired: () => void; ongo: (tab: "recent") => void } = $props();

  const relayText = $derived.by(() => {
    const relay = status?.relay;
    if (!status?.paired) return "This PC is not paired yet.";
    if (relay?.state === "connected") return "This PC is connected to the relay.";
    if (relay?.state === "reconnecting") return "This PC is reconnecting to the relay…";
    return "This PC is connecting to the relay…";
  });

  let info = $state<PairingInfo | null>(null);
  // The QR contains the key: fetched on demand, dropped when the tab or the window is hidden.
  let qr = $state<string | null>(null);
  let busy = $state(false);
  let error = $state("");

  async function load() {
    try {
      info = await getPairingInfo();
    } catch (e) {
      error = errorText(e);
    }
  }

  $effect(() => {
    load();
  });

  $effect(() => {
    if (!active) qr = null;
  });

  $effect(() => {
    const onVisibility = () => {
      if (document.hidden) qr = null;
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  });

  async function toggleQr() {
    error = "";
    if (qr) {
      qr = null;
      return;
    }
    try {
      showQr(await pairingQr());
    } catch (e) {
      error = errorText(e);
    }
  }

  // The fetch may finish after the tab or window was hidden.
  function showQr(svg: string | null) {
    qr = active && !document.hidden ? svg : null;
  }

  async function rePair() {
    const ok = confirm(
      "Create a new pairing? The current pairing stops working and the phone must scan the new QR code.",
    );
    if (!ok) return;
    error = "";
    busy = true;
    try {
      await repair();
      await load();
      showQr(info?.paired ? await pairingQr() : null);
      onrepaired();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if info?.error}
  <p class="error" role="alert">Pairing could not be loaded: {info.error}</p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

<ol class="steps">
  <li>
    <strong>Install OTP Forwarder on your Android phone.</strong>
  </li>
  <li>
    <strong>Show the QR code and scan it in the app.</strong>
    {#if info?.paired}
      <p><small class="muted">The QR code contains your encryption key. Only show it to your own phone.</small></p>
      <button type="button" class="compact outline" onclick={toggleQr} aria-expanded={qr !== null}>
        {qr ? "Hide QR code" : "Show QR code"}
      </button>
      {#if qr}
        <img src={`data:image/svg+xml;utf8,${encodeURIComponent(qr)}`} alt="Pairing QR code" />
      {/if}
    {:else if info}
      <p><small class="muted">No pairing yet. Create one with Re-pair under Advanced.</small></p>
    {/if}
  </li>
  <li>
    <strong>In the Android app, tap Send test code.</strong>
    <p><small class="muted">It should appear under Codes. {relayText}</small></p>
    <button type="button" class="compact outline" onclick={() => ongo("recent")}>Open Codes</button>
  </li>
</ol>

<details open={!!info?.error || (!!info && !info.paired)}>
  <summary>Advanced</summary>
  {#if info?.relay}<p><small>Relay: <code>{info.relay}</code></small></p>{/if}
  <p><small class="muted">Re-pairing creates a new key. The phone must scan the new QR code.</small></p>
  <button type="button" class="compact secondary outline" onclick={rePair} disabled={busy} aria-busy={busy}
    >Re-pair</button
  >
</details>

<style>
  .steps {
    padding-left: 1.25rem;
  }
  .steps li {
    margin-bottom: 1.1rem;
    padding-left: 0.25rem;
  }
  .steps p {
    margin: 0.2rem 0 0.5rem;
    line-height: 1.35;
  }
  img {
    display: block;
    width: 240px;
    height: 240px;
    margin: 0.75rem 0 0;
    background: #fff;
  }
  details {
    margin-top: 0.5rem;
    border-top: 1px solid var(--pico-muted-border-color);
    padding-top: 0.75rem;
  }
  details p {
    margin-bottom: 0.5rem;
  }
</style>
