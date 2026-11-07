<script lang="ts">
  import { errorText, getAutostart, getSettings, setAutostart, setSettings, type Settings } from "./ipc";

  let settings = $state<Settings | null>(null);
  let autostart = $state(false);
  let error = $state("");
  let saveError = $state("");
  let autostartError = $state("");
  let saved = $state(false);
  let saving = $state(false);

  $effect(() => {
    getSettings().then(
      (s) => (settings = s),
      (e) => (error = errorText(e)),
    );
    getAutostart().then(
      (a) => (autostart = a),
      (e) => (error = errorText(e)),
    );
  });

  async function toggleAutostart(box: HTMLInputElement) {
    autostartError = "";
    try {
      await setAutostart(box.checked);
      autostart = box.checked;
    } catch (e) {
      box.checked = autostart;
      autostartError = errorText(e);
    }
  }

  function editRules(edit: (rules: Settings["rules"]) => void) {
    if (!settings) return;
    edit(settings.rules);
    saved = false;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!settings) return;
    saveError = "";
    saved = false;
    saving = true;
    // Cleared number inputs bind as null.
    const s = $state.snapshot(settings);
    s.clipboard_clear_secs = Number(s.clipboard_clear_secs) || 0;
    for (const rule of s.rules) rule.delay_ms = Number(rule.delay_ms) || 0;
    try {
      await setSettings(s);
      saved = true;
    } catch (e) {
      saveError = errorText(e);
    } finally {
      saving = false;
    }
  }
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}

<label>
  <input type="checkbox" role="switch" checked={autostart} onchange={(e) => toggleAutostart(e.currentTarget)} />
  Start with Windows
</label>
{#if autostartError}<p class="error" role="alert">{autostartError}</p>{/if}

{#if settings}
  <form onsubmit={save} oninput={() => (saved = false)}>
    <label>
      <input type="checkbox" role="switch" bind:checked={settings.auto_type} />
      Auto-type codes into these apps
    </label>

    <table>
      <thead>
        <tr>
          <th scope="col">App (.exe)</th>
          <th scope="col">On</th>
          <th scope="col">Enter</th>
          <th scope="col">Delay ms</th>
          <th scope="col"><span class="visually-hidden">Remove</span></th>
        </tr>
      </thead>
      <tbody>
        {#each settings.rules as rule, i (rule)}
          <tr>
            <td><input aria-label="Executable name" bind:value={rule.exe} required maxlength="64" /></td>
            <td><input type="checkbox" aria-label="Enabled" bind:checked={rule.enabled} /></td>
            <td><input type="checkbox" aria-label="Press Enter after typing" bind:checked={rule.press_enter} /></td>
            <td>
              <input
                type="number"
                aria-label="Delay between keystrokes in ms"
                bind:value={rule.delay_ms}
                required
                min="0"
                max="500"
                step="1"
              />
            </td>
            <td>
              <button
                type="button"
                class="outline secondary"
                aria-label="Remove rule"
                onclick={() => editRules((rules) => rules.splice(i, 1))}>×</button
              >
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <button
      type="button"
      class="outline"
      onclick={() => editRules((rules) => rules.push({ exe: "", enabled: true, press_enter: false, delay_ms: 15 }))}
    >
      Add app
    </button>
    <p><small>Enable "Enter" only for apps you trust: anyone who can text your phone can trigger it.</small></p>

    <label>
      Clear clipboard after (seconds, 0 = never)
      <input type="number" bind:value={settings.clipboard_clear_secs} required min="0" max="600" step="1" />
    </label>
    <label>
      <input type="checkbox" role="switch" bind:checked={settings.notifications} />
      Show notifications
    </label>
    <label>
      Hotkey to type the last code (empty = off)
      <input bind:value={settings.hotkey} placeholder="Ctrl+Alt+Shift+O" />
    </label>

    <button type="submit" disabled={saving} aria-busy={saving}>Save</button>
    {#if saveError}<p class="error" role="alert">{saveError}</p>{/if}
    {#if saved}<p role="status"><small>Saved</small></p>{/if}
  </form>
{/if}

<style>
  table {
    font-size: 0.85rem;
  }
  th,
  td {
    padding: 0.25rem;
  }
  td input:not([type="checkbox"]) {
    margin: 0;
    padding: 0.25rem 0.4rem;
    height: auto;
  }
  input[type="number"] {
    min-width: 4.5rem;
  }
  td button {
    margin: 0;
    padding: 0.2rem 0.6rem;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .error {
    color: var(--pico-del-color);
  }
</style>
