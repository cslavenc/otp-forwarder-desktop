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
  } from "./ipc";

  const REVEAL_MS = 10_000;

  let items = $state<RecentView[]>([]);
  let now = $state(Date.now());
  let revealed = $state<{ id: string; code: string } | null>(null);
  let rejection = $state<Reject | null>(null);
  let testing = $state(false);
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
    notice = "Focus the target field now — sending in 5 s…";
    testing = true;
    try {
      await sendTestCode();
      notice = "Test code sent.";
    } catch (e) {
      notice = "";
      error = errorText(e);
    } finally {
      testing = false;
    }
  }

  function age(ms: number): string {
    const s = Math.max(0, Math.round((now - ms) / 1000));
    return s < 60 ? `${s} s ago` : `${Math.floor(s / 60)} min ago`;
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
  <p class="notice" role="status">
    <span>{rejectText(rejection)}</span>
    <button type="button" class="outline secondary" aria-label="Dismiss" onclick={() => (rejection = null)}>
      ×
    </button>
  </p>
{/if}

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if visible.length === 0}
  <p><small>No recent codes. Codes disappear after 5 minutes.</small></p>
{:else}
  <ul>
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
              class="outline"
              aria-pressed={shown}
              aria-label={`${shown ? "Hide" : "Reveal"} code from ${from}`}
              onclick={() => reveal(r.id)}>{shown ? "Hide" : "Reveal"}</button
            >
            <button type="button" class="outline" aria-label={`Copy code from ${from}`} onclick={() => copy(r.id)}
              >Copy</button
            >
          </span>
        </div>
        <small>{r.sender || "Unknown sender"} · {age(r.received_ms)} · {outcome(r.delivery)}</small>
      </li>
    {/each}
  </ul>
{/if}

<button type="button" onclick={sendTest} disabled={testing} aria-busy={testing}>Send test code</button>
{#if notice}<p role="status"><small>{notice}</small></p>{/if}

<style>
  ul {
    padding: 0;
  }
  li {
    list-style: none;
    margin-bottom: 0.75rem;
  }
  .row,
  .notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .actions button,
  .notice button {
    padding: 0.2rem 0.6rem;
    margin: 0;
  }
  code {
    font-size: 1.2rem;
    letter-spacing: 0.1em;
  }
  .notice {
    padding: 0.5rem;
    border: 1px solid var(--pico-muted-border-color);
    border-radius: var(--pico-border-radius);
  }
</style>
