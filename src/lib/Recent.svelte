<script lang="ts">
  import {
    copyCode,
    errorText,
    listRecent,
    onRecent,
    onRejected,
    revealCode,
    sendTestCode,
    subscribe,
    type Delivery,
    type Reject,
    type RecentView,
    type Status,
  } from "./ipc";

  let { status, ongo }: { status: Status | null; ongo: (tab: "pairing") => void } = $props();

  const REVEAL_MS = 10_000;
  const TEST_DELAY_S = 5;

  let items = $state<RecentView[]>([]);
  let now = $state(Date.now());
  let revealed = $state<{ id: string; code: string } | null>(null);
  let rejection = $state<Reject | null>(null);
  let testing = $state(false);
  let countdown = $state(0);
  let notice = $state("");
  let error = $state("");

  const visible = $derived(items.filter((r) => r.expires_ms > now));

  $effect(() => {
    let gotEvent = false;
    const stop = subscribe(
      [
        onRecent((r) => {
          gotEvent = true;
          items = r;
        }),
        onRejected((r) => (rejection = r)),
      ],
      async () => {
        const r = await listRecent();
        // An event that arrived meanwhile is at least as new.
        if (!gotEvent) items = r;
      },
      (e) => (error = e),
    );
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => {
      stop();
      clearInterval(tick);
    };
  });

  let maskTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(maskTimer));

  // Re-mask as soon as the revealed entry is gone.
  $effect(() => {
    const id = revealed?.id;
    if (id && !visible.some((r) => r.id === id)) revealed = null;
  });

  function hide() {
    clearTimeout(maskTimer);
    revealed = null;
  }

  async function reveal(id: string) {
    if (revealed?.id === id) return hide();
    error = "";
    try {
      revealed = { id, code: await revealCode(id) };
      clearTimeout(maskTimer);
      maskTimer = setTimeout(hide, REVEAL_MS);
    } catch (e) {
      error = errorText(e);
    }
  }

  async function copy(id: string) {
    error = "";
    notice = "";
    try {
      await copyCode(id);
      notice = "Code copied to the clipboard.";
    } catch (e) {
      error = errorText(e);
    }
  }

  async function sendTest() {
    error = "";
    notice = `Sending a test code in ${TEST_DELAY_S} seconds — click into the target field.`;
    testing = true;
    countdown = TEST_DELAY_S;
    const tick = setInterval(() => (countdown = Math.max(1, countdown - 1)), 1000);
    try {
      await sendTestCode();
      notice = "Test code sent.";
    } catch (e) {
      notice = "";
      error = errorText(e);
    } finally {
      clearInterval(tick);
      testing = false;
    }
  }

  function age(ms: number): string {
    const s = Math.max(0, Math.round((now - ms) / 1000));
    return s < 60 ? `${s} s ago` : `${Math.floor(s / 60)} min ago`;
  }

  function expiresIn(ms: number): string {
    const s = Math.max(0, Math.round((ms - now) / 1000));
    return s < 60 ? `expires in ${s} s` : `expires in ${Math.ceil(s / 60)} min`;
  }

  function tone(d: Delivery): string {
    return d.kind === "typed" ? "ok" : d.kind === "failed" ? "error" : "muted";
  }

  function outcome(d: Delivery): string {
    switch (d.kind) {
      case "pending":
        return "Delivering…";
      case "typed":
        return `Typed into ${d.target}`;
      case "copied":
        return `Copied to clipboard (${d.reason})`;
      case "failed":
        return `Not delivered: ${d.reason}`;
    }
  }

  function rejectText(r: Reject): string {
    switch (r.kind) {
      case "stale":
        return `Ignored a code that was ${r.age_s} s old — check the phone/PC clock.`;
      case "from_future":
        return `Ignored a code dated ${r.ahead_s} s in the future — check the phone/PC clock.`;
      case "undecryptable":
        return "Received a message that could not be decrypted (wrong key or not from your phone).";
      case "unsupported_version":
        return `Received a message with unsupported version ${r.v} — update the apps.`;
      case "duplicate":
        return "Ignored a code that was already received.";
      case "too_large":
        return "Ignored a message that was too large.";
      default:
        return "Ignored a malformed message from the phone.";
    }
  }
</script>

{#if rejection}
  <div class="callout rejection" role="status">
    <span>{rejectText(rejection)}</span>
    <button type="button" class="compact outline secondary" aria-label="Dismiss" onclick={() => (rejection = null)}
      >✕</button
    >
  </div>
{/if}

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#snippet testButton(cls: string)}
  <button type="button" class={cls} onclick={sendTest} disabled={testing} aria-busy={testing}>Send test code</button>
{/snippet}

{#if visible.length === 0}
  {#if status && !status.paired}
    <div class="callout">
      <h2>Pair your phone first</h2>
      <p class="muted">Scan a QR code with the OTP Forwarder Android app, then forwarded codes show up here.</p>
      <button type="button" onclick={() => ongo("pairing")}>Go to Pairing</button>
    </div>
  {:else}
    <div class="callout">
      <h2>No codes yet</h2>
      <p class="muted">Codes your phone forwards appear here for 5 minutes.</p>
      {@render testButton("")}
      <p><small class="muted">Tests this PC only: types a sample code into the app you click into.</small></p>
    </div>
  {/if}
{:else}
  <ul class="codes">
    {#each visible as r (r.id)}
      {@const shown = revealed?.id === r.id}
      {@const from = r.sender || "unknown sender"}
      <li>
        <div class="row">
          {#if revealed?.id === r.id}
            <code>{revealed.code}</code>
          {:else}
            <code aria-label="hidden code">{r.masked}</code>
          {/if}
          <span class="actions">
            <button
              type="button"
              class="compact outline"
              aria-pressed={shown}
              aria-label={`${shown ? "Hide" : "Reveal"} code from ${from}`}
              onclick={() => reveal(r.id)}>{shown ? "Hide" : "Reveal"}</button
            >
            <button type="button" class="compact" aria-label={`Copy code from ${from}`} onclick={() => copy(r.id)}
              >Copy</button
            >
          </span>
        </div>
        <small class="muted">{r.sender || "Unknown sender"} · {age(r.received_ms)} · {expiresIn(r.expires_ms)}</small>
        <small class="delivery {tone(r.delivery)}">{outcome(r.delivery)}</small>
      </li>
    {/each}
  </ul>
  {@render testButton("outline")}
{/if}

{#if testing}
  <p class="countdown" aria-hidden="true">Sending in {countdown} s — click into the target field</p>
{/if}
<p class="live" class:visually-hidden={testing} role="status">{#if notice}<small>{notice}</small>{/if}</p>

<style>
  h2 {
    margin-bottom: 0.35rem;
    font-size: 1.05rem;
  }
  .callout p {
    margin-bottom: 0.75rem;
  }
  .callout button {
    margin-bottom: 0.5rem;
  }
  .callout > :last-child {
    margin-bottom: 0;
  }
  .rejection {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }
  .codes {
    padding: 0;
  }
  .codes li {
    list-style: none;
    margin-bottom: 0.75rem;
    padding: 0.6rem 0.85rem;
    border: 1px solid var(--pico-muted-border-color);
    border-radius: var(--pico-border-radius);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.25rem;
  }
  .actions {
    display: flex;
    gap: 0.4rem;
  }
  code {
    padding: 0;
    background: none;
    font-size: 1.5rem;
    font-weight: 600;
    letter-spacing: 0.12em;
  }
  .codes small {
    display: block;
    line-height: 1.35;
  }
  .delivery.ok {
    color: var(--pico-ins-color);
  }
  .countdown {
    margin: 0.75rem 0 0;
    font-weight: 600;
  }
  .live {
    margin: 0.5rem 0 0;
  }
  .live:empty {
    margin: 0;
  }
</style>
