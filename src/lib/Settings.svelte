<script lang="ts">
  import { tick } from "svelte";
  import {
    errorText,
    getAutostart,
    getSettings,
    onHidden,
    setAutostart,
    setSettings,
    subscribe,
    type Settings,
  } from "./ipc";

  // Mirror src-tauri/src/settings.rs.
  const MAX_RULES = 50;
  const MAX_EXE_CHARS = 64;
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
  let rulesBody = $state<HTMLElement>();

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

  const status = $derived(
    fieldErrors.length > 0 ? `Not saved: fix ${fieldErrors[0].label}` : busy ? "Saving…" : flash,
  );

  /** A bare `.exe` file name from a typed name or pasted path, or why it can't be used. */
  function normalizeExe(raw: string): { exe: string } | { error: string } {
    let name = raw.trim();
    if (name.length >= 2 && name.startsWith('"') && name.endsWith('"')) name = name.slice(1, -1).trim();
    name = name.split(/[\\/]/).pop() ?? "";
    if (!name) return { error: "Enter an app name, e.g. notepad" };
    const ext = /\.([a-z0-9]{1,4})$/i.exec(name)?.[1].toLowerCase();
    if (ext && ext !== "exe") return { error: `${name} isn't a program — use the .exe it starts` };
    if (!ext) name += ".exe";
    const chars = [...name].length;
    if (chars < 5 || chars > MAX_EXE_CHARS || /[\\/:\p{Cc}]/u.test(name))
      return { error: `Use a file name like notepad.exe (at most ${MAX_EXE_CHARS} characters)` };
    return { exe: name };
  }

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
        if (fieldErrors.length > 0) break;
        const snap = $state.snapshot(settings);
        const json = JSON.stringify(snap);
        if (json === lastSaved) continue;
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
          if (settings.hotkey === snap.hotkey) settings.hotkey = savedHotkey;
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

  async function remove(i: number) {
    if (!settings) return;
    const [rule] = settings.rules.splice(i, 1);
    notice = `Removed ${rule.exe}`;
    save();
    await tick();
    const buttons = rulesBody?.querySelectorAll<HTMLButtonElement>("button.remove") ?? [];
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
        <button type="button" class="outline compact" onclick={save}>Retry</button>
      </p>
    {/if}
  </div>
{/if}

<label>
  <input type="checkbox" role="switch" checked={autostart} onchange={(e) => toggleAutostart(e.currentTarget)} />
  Start with Windows
</label>
{#if autostartError}<p class="error" role="alert">{autostartError}</p>{/if}

{#if settings}
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
    <tbody bind:this={rulesBody}>
      {#each settings.rules as rule, i (rule)}
        {@const delayError = fieldError(`delay-${i}`)}
        <tr>
          <td class="exe">{rule.exe}</td>
          <td>
            <input
              type="checkbox"
              aria-label="Auto-type into {rule.exe}"
              bind:checked={
                () => rule.enabled,
                (v) => {
                  rule.enabled = v;
                  save();
                }
              }
            />
          </td>
          <td>
            <input
              type="checkbox"
              aria-label="Press Enter after typing into {rule.exe}"
              bind:checked={
                () => rule.press_enter,
                (v) => {
                  rule.press_enter = v;
                  save();
                }
              }
            />
          </td>
          <td>
            <input
              id="delay-{i}"
              type="number"
              aria-label="Delay between keystrokes into {rule.exe}, in ms"
              aria-invalid={delayError ? true : undefined}
              aria-describedby={delayError ? `delay-${i}-msg` : undefined}
              bind:value={rule.delay_ms}
              onchange={save}
              min="0"
              max={MAX_DELAY_MS}
              step="1"
            />
            {#if delayError}<small id="delay-{i}-msg" class="error">{delayError}</small>{/if}
          </td>
          <td>
            <button type="button" class="outline secondary remove" aria-label="Remove {rule.exe}" onclick={() => remove(i)}
              >×</button
            >
          </td>
        </tr>
        {#if rule.press_enter && isShell(rule.exe)}
          <tr><td colspan="5"><small class="error">{TERMINAL_WARNING}</small></td></tr>
        {/if}
      {/each}
    </tbody>
  </table>

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
  <p><small>Enable "Enter" only for apps you trust: anyone who can text your phone can trigger it.</small></p>

  {@const clipboardError = fieldError("clipboard-clear")}
  <label>
    Clear clipboard after (seconds, 0 = never)
    <input
      id="clipboard-clear"
      type="number"
      aria-invalid={clipboardError ? true : undefined}
      aria-describedby={clipboardError ? "clipboard-clear-msg" : undefined}
      bind:value={settings.clipboard_clear_secs}
      onchange={save}
      min="0"
      max={MAX_CLIPBOARD_CLEAR_SECS}
      step="1"
    />
    {#if clipboardError}<small id="clipboard-clear-msg" class="error">{clipboardError}</small>{/if}
  </label>
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
  <label>
    Hotkey to type the last code (empty = off)
    <input
      bind:value={settings.hotkey}
      onchange={save}
      placeholder="Ctrl+Alt+Shift+O"
      aria-invalid={hotkeyWarning ? true : undefined}
      aria-describedby={hotkeyWarning ? "hotkey-msg" : undefined}
    />
    {#if hotkeyWarning}<small id="hotkey-msg" class="error">{hotkeyWarning}</small>{/if}
  </label>
{/if}

<style>
  .status {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--pico-background-color);
  }
  .status p {
    margin: 0 0 0.5rem;
  }
  table {
    font-size: 0.85rem;
  }
  th,
  td {
    padding: 0.25rem;
  }
  td.exe {
    word-break: break-all;
  }
  td input:not([type="checkbox"]) {
    margin: 0;
    padding: 0.25rem 0.4rem;
    height: auto;
  }
  input[type="number"] {
    min-width: 4.5rem;
  }
  td button,
  .compact {
    margin: 0;
    padding: 0.2rem 0.6rem;
  }
  .add div[role="group"] {
    margin-bottom: 0.25rem;
  }
  .muted {
    color: var(--pico-muted-color);
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
