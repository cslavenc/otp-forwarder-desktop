<script lang="ts">
  import { tick } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    errorText,
    getAutostart,
    getSettings,
    onHidden,
    setAutostart,
    setSettings,
    subscribe,
    type Rule,
    type Settings,
  } from "./ipc";
  import { normalizeExe } from "./exe";

  // Mirror src-tauri/src/settings.rs.
  const MAX_RULES = 50;
  const MAX_DELAY_MS = 500;
  const MAX_CLIPBOARD_CLEAR_SECS = 600;
  const SHELLS = [
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "windowsterminal.exe",
    "wt.exe",
    "conhost.exe",
    "bash.exe",
    "wsl.exe",
  ];
  const TERMINAL_WARNING =
    "Terminal: a code plus Enter runs as a command (e.g. logoff). Anyone who can text you could trigger it.";
  const FLASH_MS = 2000;

  type FieldError = { id: string; label: string; message: string };

  // The only truth once loaded; never replaced, so the {#each} keys stay stable.
  let settings = $state<Settings | null>(null);
  let autostart = $state(false);
  let error = $state("");
  let autostartError = $state("");
  let saveError = $state("");
  let hotkeyWarning = $state("");
  let busy = $state(false);
  let flash = $state("");
  let notice = $state("");
  let newExe = $state("");
  let addError = $state("");
  let addInput = $state<HTMLInputElement>();
  let ruleList = $state<HTMLElement>();
  let hotkeyInput = $state<HTMLInputElement>();
  // Rules whose "Typing delay" details are open; opened for a blocked save, only the user closes them.
  const openRules = new SvelteSet<Rule>();

  let lastSaved = "";
  let savedHotkey = "";
  let saving = false;
  let again = false;
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  const isShell = (exe: string) => SHELLS.includes(exe.toLowerCase());
  const inRange = (v: unknown, max: number) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= max;

  const fieldErrors = $derived.by(() => {
    const errors: FieldError[] = [];
    if (!settings) return errors;
    settings.rules.forEach((rule, i) => {
      if (!inRange(rule.delay_ms, MAX_DELAY_MS))
        errors.push({ id: `delay-${i}`, label: `delay of ${rule.exe}`, message: `Use 0–${MAX_DELAY_MS} ms` });
    });
    if (!inRange(settings.clipboard_clear_secs, MAX_CLIPBOARD_CLEAR_SECS))
      errors.push({ id: "clipboard-clear", label: "clipboard timeout", message: `Use 0–${MAX_CLIPBOARD_CLEAR_SECS} s` });
    return errors;
  });
  const fieldError = (id: string) => fieldErrors.find((e) => e.id === id)?.message;
  const clipboardError = $derived(fieldError("clipboard-clear"));

  const status = $derived(
    fieldErrors.length > 0 ? `Not saved: fix ${fieldErrors[0].label}` : busy ? "Saving…" : flash,
  );

  const candidate = $derived(normalizeExe(newExe));

  function load() {
    return Promise.all([
      getSettings().then(
        (s) => {
          lastSaved = JSON.stringify(s);
          savedHotkey = s.hotkey;
          settings = s;
        },
        (e) => (error = errorText(e)),
      ),
      getAutostart().then(
        (a) => (autostart = a),
        (e) => (error = errorText(e)),
      ),
    ]);
  }

  $effect(() => {
    window.addEventListener("blur", save);
    const unsubscribe = subscribe([onHidden(save)], load, (e) => (error = e));
    return () => {
      window.removeEventListener("blur", save);
      unsubscribe();
      clearTimeout(flashTimer);
    };
  });

  function showFlash() {
    flash = notice ? `${notice} — saved` : "Saved";
    notice = "";
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = ""), FLASH_MS);
  }

  /** Saves the current state; calls during a save merge into one more round, the last snapshot wins. */
  async function save() {
    if (!settings) return;
    if (saving) {
      again = true;
      return;
    }
    saving = true;
    busy = true;
    try {
      do {
        again = false;
        if (fieldErrors.length > 0) {
          settings.rules.forEach((r, i) => fieldError(`delay-${i}`) && openRules.add(r));
          break;
        }
        const snap = $state.snapshot(settings);
        const json = JSON.stringify(snap);
        if (json === lastSaved) {
          saveError = "";
          continue;
        }
        let warning: string | null;
        try {
          warning = await setSettings(snap);
        } catch (e) {
          saveError = errorText(e); // edits are kept for Retry
          break;
        }
        saveError = "";
        showFlash();
        if (warning) {
          // Not while typing: a blur flush would wipe a half-typed hotkey.
          if (settings.hotkey === snap.hotkey && document.activeElement !== hotkeyInput)
            settings.hotkey = savedHotkey;
          hotkeyWarning = warning;
          lastSaved = JSON.stringify({ ...snap, hotkey: savedHotkey });
        } else {
          savedHotkey = snap.hotkey;
          hotkeyWarning = "";
          lastSaved = json;
        }
      } while (again);
    } finally {
      saving = false;
      busy = false;
    }
  }

  function add(event: SubmitEvent) {
    event.preventDefault();
    if (!settings) return;
    const result = candidate;
    if ("error" in result) {
      addError = result.error;
    } else if (settings.rules.some((r) => r.exe.toLowerCase() === result.exe.toLowerCase())) {
      addError = `${result.exe} is already in the list`;
    } else if (settings.rules.length >= MAX_RULES) {
      addError = `At most ${MAX_RULES} apps are allowed`;
    } else {
      settings.rules.push({ exe: result.exe, enabled: true, press_enter: false, delay_ms: 15 });
      newExe = "";
      addError = "";
      notice = `Added ${result.exe}`;
      save();
    }
    addInput?.focus();
  }

  async function retry() {
    await save();
    // The Retry button disappears on success; don't leave focus on nothing.
    if (!saveError) addInput?.focus();
  }

  async function remove(i: number) {
    if (!settings) return;
    const [rule] = settings.rules.splice(i, 1);
    notice = `Removed ${rule.exe}`;
    save();
    await tick();
    const buttons = ruleList?.querySelectorAll<HTMLButtonElement>("button.remove") ?? [];
    (buttons[i] ?? buttons[i - 1] ?? addInput)?.focus();
  }

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
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if settings}
  <div class="status">
    <p role="status"><small>{status}</small></p>
    {#if !status && !saveError}<p><small class="muted">Changes are saved automatically.</small></p>{/if}
    {#if saveError}
      <p class="error" role="alert">
        Not saved: {saveError}
        <button type="button" class="outline compact" onclick={retry}>Retry</button>
      </p>
    {/if}
  </div>

  <section aria-labelledby="sec-autotype">
    <h2 id="sec-autotype">Auto-type</h2>
    <p class="help muted">Codes are typed into these apps when they have focus. Any other app gets the code on the clipboard.</p>
    <label>
      <input
        type="checkbox"
        role="switch"
        bind:checked={
          () => settings!.auto_type,
          (v) => {
            settings!.auto_type = v;
            save();
          }
        }
      />
      Auto-type codes into the apps below
    </label>
    {#if !settings.auto_type}
      <p class="help muted">Auto-type is off: every code goes to the clipboard. You can still edit the list.</p>
    {/if}

    <ul class="cards" aria-label="Apps that receive typed codes" bind:this={ruleList}>
      {#each settings.rules as rule, i (rule)}
        {@const delayError = fieldError(`delay-${i}`)}
        {@const terminal = rule.press_enter && isShell(rule.exe)}
        <li>
          <div class="head">
            <strong class="exe">{rule.exe}</strong>
            <button type="button" class="outline secondary compact remove" aria-label="Remove {rule.exe}" onclick={() => remove(i)}
              >✕</button
            >
          </div>
          <div class="opts">
            <label>
              <input
                type="checkbox"
                role="switch"
                bind:checked={
                  () => rule.enabled,
                  (v) => {
                    rule.enabled = v;
                    save();
                  }
                }
              />
              Auto-type<span class="visually-hidden"> into {rule.exe}</span>
            </label>
            <label>
              <input
                type="checkbox"
                aria-describedby={terminal ? `terminal-${i}` : undefined}
                bind:checked={
                  () => rule.press_enter,
                  (v) => {
                    rule.press_enter = v;
                    save();
                  }
                }
              />
              Press Enter after typing<span class="visually-hidden"> into {rule.exe}</span>
            </label>
          </div>
          {#if terminal}<p class="warn"><small id="terminal-{i}" class="error">{TERMINAL_WARNING}</small></p>{/if}
          <details
            open={openRules.has(rule)}
            ontoggle={(e) => (e.currentTarget.open ? openRules.add(rule) : openRules.delete(rule))}
          >
            <summary>Typing delay: {rule.delay_ms} ms</summary>
            <label for="delay-{i}">Pause between keystrokes, 0–{MAX_DELAY_MS} ms<span class="visually-hidden"> for {rule.exe}</span></label>
            <input
              id="delay-{i}"
              type="number"
              aria-invalid={delayError ? true : undefined}
              aria-describedby={delayError ? `delay-${i}-msg` : undefined}
              bind:value={rule.delay_ms}
              onchange={save}
              min="0"
              max={MAX_DELAY_MS}
              step="1"
            />
            {#if delayError}<small id="delay-{i}-msg" class="error">{delayError}</small>{/if}
          </details>
        </li>
      {/each}
    </ul>

    <form class="add" onsubmit={add}>
      <label for="new-exe">Add an app (name or path of its .exe)</label>
      <div role="group">
        <input
          id="new-exe"
          bind:this={addInput}
          bind:value={newExe}
          oninput={() => (addError = "")}
          placeholder="notepad"
          aria-invalid={addError ? true : undefined}
          aria-describedby="new-exe-msg"
        />
        <button type="submit">Add</button>
      </div>
      <small id="new-exe-msg" class:error={addError || ("exe" in candidate && isShell(candidate.exe))}>
        {#if addError}{addError}{:else if "exe" in candidate && isShell(candidate.exe)}{TERMINAL_WARNING}{/if}
      </small>
    </form>
    <p class="help muted">Use "Press Enter" only for apps you trust: anyone who can text your phone can trigger it.</p>
    <details class="howto">
      <summary>How do I find an app's name?</summary>
      <p>
        Open Task Manager, go to <strong>Details</strong>, and look for the app; the <strong>Name</strong> column shows
        its file name, e.g. <code>notepad.exe</code>. Citrix sessions run in <code>wfica32.exe</code> or
        <code>CDViewer.exe</code>.
      </p>
    </details>
  </section>

  <section aria-labelledby="sec-clipboard">
    <h2 id="sec-clipboard">Clipboard &amp; notifications</h2>
    <p class="help muted">Used when a code can't be typed.</p>
    <label for="clipboard-clear">Clear the clipboard after (seconds, 0 = never)</label>
    <input
      id="clipboard-clear"
      type="number"
      class="narrow"
      aria-invalid={clipboardError ? true : undefined}
      aria-describedby={clipboardError ? "clipboard-clear-msg" : undefined}
      bind:value={settings.clipboard_clear_secs}
      onchange={save}
      min="0"
      max={MAX_CLIPBOARD_CLEAR_SECS}
      step="1"
    />
    {#if clipboardError}<small id="clipboard-clear-msg" class="error">{clipboardError}</small>{/if}
    <label>
      <input
        type="checkbox"
        role="switch"
        bind:checked={
          () => settings!.notifications,
          (v) => {
            settings!.notifications = v;
            save();
          }
        }
      />
      Show notifications
    </label>
  </section>

  <section aria-labelledby="sec-hotkey">
    <h2 id="sec-hotkey">Hotkey</h2>
    <p class="help muted">Types the last code into the focused app.</p>
    <label>
      Hotkey (empty = off)
      <input
        bind:this={hotkeyInput}
        bind:value={settings.hotkey}
        onblur={save}
        onchange={save}
        placeholder="Ctrl+Alt+Shift+O"
        aria-invalid={hotkeyWarning ? true : undefined}
        aria-describedby={hotkeyWarning ? "hotkey-msg" : undefined}
      />
      {#if hotkeyWarning}<small id="hotkey-msg" class="error">{hotkeyWarning}</small>{/if}
    </label>
  </section>
{/if}

<section aria-labelledby="sec-startup">
  <h2 id="sec-startup">Startup</h2>
  <label>
    <input type="checkbox" role="switch" checked={autostart} onchange={(e) => toggleAutostart(e.currentTarget)} />
    Start with Windows
  </label>
  {#if autostartError}<p class="error" role="alert">{autostartError}</p>{/if}
</section>

<style>
  .status {
    position: sticky;
    top: 0;
    z-index: 1;
    margin: -0.25rem 0 1rem;
    padding-top: 0.25rem;
    border-bottom: 1px solid var(--pico-muted-border-color);
    background: var(--pico-background-color);
  }
  .status p {
    margin: 0 0 0.5rem;
  }
  section {
    margin-bottom: 1.75rem;
  }
  h2 {
    margin-bottom: 0.15rem;
    font-size: 1.05rem;
  }
  .help {
    margin-bottom: 0.75rem;
    font-size: 0.875rem;
    line-height: 1.4;
  }
  .cards {
    margin: 0.5rem 0 1rem;
    padding: 0;
  }
  .cards li {
    list-style: none;
    margin-bottom: 0.6rem;
    padding: 0.6rem 0.85rem 0.15rem;
    border: 1px solid var(--pico-muted-border-color);
    border-radius: var(--pico-border-radius);
    background: var(--pico-card-background-color);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.4rem;
  }
  .exe {
    overflow-wrap: anywhere;
  }
  .remove {
    line-height: 1;
  }
  .opts {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem 1.25rem;
  }
  .opts label {
    margin-bottom: 0.4rem;
    font-size: 0.9rem;
  }
  .warn {
    margin: 0 0 0.4rem;
    line-height: 1.35;
  }
  .cards details {
    margin-bottom: 0.4rem;
    padding: 0;
    border: 0;
  }
  .cards summary {
    color: var(--pico-muted-color);
    font-size: 0.875rem;
  }
  .cards details label {
    margin-top: 0.4rem;
    font-size: 0.875rem;
  }
  .cards details input,
  .narrow {
    max-width: 8rem;
  }
  .cards details input {
    margin-bottom: 0.25rem;
  }
  .add div[role="group"] {
    margin-bottom: 0.25rem;
  }
  .howto {
    margin-bottom: 0;
  }
  .howto p {
    font-size: 0.875rem;
  }
</style>
