<script lang="ts">
  import { errorText, getPairingInfo, pairingQr, repair, type PairingInfo } from "./ipc";

  let { active, onrepaired }: { active: boolean; onrepaired: () => void } = $props();

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

<p>Scan this with the OTP Forwarder Android app.</p>

{#if info?.error}
  <p class="error" role="alert">Pairing could not be loaded: {info.error}</p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if info?.paired}
  <p><small>Relay: {info.relay}</small></p>
  <button type="button" class="outline" onclick={toggleQr} aria-expanded={qr !== null}>
    {qr ? "Hide QR code" : "Show QR code"}
  </button>
  {#if qr}
    <img src={`data:image/svg+xml;utf8,${encodeURIComponent(qr)}`} alt="Pairing QR code" />
  {/if}
{/if}

<p>
  <button type="button" class="secondary" onclick={rePair} disabled={busy} aria-busy={busy}>Re-pair</button>
</p>

<style>
  img {
    display: block;
    width: 260px;
    height: 260px;
    margin: 1rem auto;
    background: #fff;
  }
  .error {
    color: var(--pico-del-color);
  }
</style>
