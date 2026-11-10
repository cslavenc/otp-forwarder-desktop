<script lang="ts">
  import { tick } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    detectApp,
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
  let addButton = $state<HTMLButtonElement>();
  let fileInput = $state<HTMLInputElement>();
  let detecting = $state(0); // seconds left, 0 = idle
  let detectLive = $state("");
  let addHint = $state("");
  let ruleList = $state<HTMLElement>();
  let recording = $state(false);
  let preview = $state("");
  let recordError = $state("");
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
  const shellCandidate = $derived("exe" in candidate && isShell(candidate.exe));

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
      addHint = "";
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

  /** Puts a detected or picked name into the add row; the user still confirms with Add. */
  function offer(raw: string, verb: string) {
    const result = normalizeExe(raw);
    newExe = "exe" in result ? result.exe : raw;
    if ("error" in result) {
      addError = result.error;
      addHint = "";
      addInput?.focus();
      return;
    }
    addError = "";
    addHint = `${verb} ${result.exe} — press Add to allow it`;
    addButton?.focus();
  }

  async function detect(button: HTMLButtonElement) {
    addError = "";
    addHint = "";
    detectLive = "Switch to the app now";
    detecting = 5;
    const timer = setInterval(() => (detecting = Math.max(1, detecting - 1)), 1000);
    let failed = false;
    try {
      offer(await detectApp(), "Detected");
    } catch (e) {
      addError = errorText(e);
      failed = true;
    } finally {
      clearInterval(timer);
      detecting = 0;
      detectLive = "";
    }
    if (failed) {
      await tick(); // the button is disabled until this render
      button.focus();
    }
  }

  function picked(input: HTMLInputElement) {
    const file = input.files?.[0];
    input.value = ""; // picking the same file again fires change again
    if (file) offer(file.name, "Picked");
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

  // Hotkey recorder. Names follow the global-shortcut parser; "Super" is shown as "Win".
  const MODIFIER_KEYS = ["Control", "Alt", "AltGraph", "Shift", "Meta", "OS"];
  const showHotkey = (hotkey: string) => hotkey.replace(/\bSuper\b/g, "Win") || "Off";

  function modifiers(e: KeyboardEvent): string[] {
    const m: string[] = [];
    if (e.ctrlKey) m.push("Ctrl");
    if (e.altKey) m.push("Alt");
    if (e.shiftKey) m.push("Shift");
    if (e.metaKey) m.push("Super");
    return m;
  }

  // keyCode, not key/code: it names the key by its layout label (Swiss Z is "Z") like Windows does.
  function mainKey(keyCode: number): string | null {
    if ((keyCode >= 0x41 && keyCode <= 0x5a) || (keyCode >= 0x30 && keyCode <= 0x39))
      return String.fromCharCode(keyCode);
    if (keyCode >= 0x70 && keyCode <= 0x87) return `F${keyCode - 0x6f}`;
    return null;
  }

  function stopRecording() {
    recording = false;
    preview = "";
  }

  function toggleRecording() {
    if (recording) return stopRecording();
    recording = true;
    recordError = "";
  }

  // shortcut: pressing the currently registered hotkey while recording triggers it (Windows
  // consumes it before the webview sees it) instead of being recorded.
  function recordKey(e: KeyboardEvent) {
    if (!recording || !settings) return;
    if (e.key === "Tab") return; // focus moves on; blur cancels
    e.preventDefault();
    if (e.repeat) return;
    const mods = modifiers(e);
    if (e.key === "Escape") return stopRecording();
    if ((e.key === "Backspace" || e.key === "Delete") && mods.length === 0) {
      stopRecording();
      recordError = "";
      settings.hotkey = "";
      save();
      return;
    }
    if (MODIFIER_KEYS.includes(e.key)) {
      preview = showHotkey([...mods, "…"].join("+"));
      return;
    }
    preview = "";
    const key = mainKey(e.keyCode);
    if (!key) recordError = "Use a letter, digit or F1–F24.";
    else if (mods.length < 2) recordError = "Add at least two of Ctrl, Alt, Shift, Win";
    else if (mods.join("+") === "Ctrl+Alt") recordError = "Ctrl+Alt is AltGr on many keyboards — add Shift or Win";
    else {
      stopRecording();
      recordError = "";
      settings.hotkey = [...mods, key].join("+");
      save();
    }
  }

  function recordKeyUp(e: KeyboardEvent) {
    if (!recording || e.key === "Tab") return;
    e.preventDefault(); // Space would otherwise click the button
    const mods = modifiers(e);
    preview = mods.length ? showHotkey([...mods, "…"].join("+")) : "";
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
          oninput={() => ((addError = ""), (addHint = ""))}
          placeholder="notepad"
          aria-invalid={addError ? true : undefined}
          aria-describedby="new-exe-msg"
        />
        <button type="submit" bind:this={addButton} aria-describedby="new-exe-msg">Add</button>
      </div>
      <small id="new-exe-msg" class:error={addError || shellCandidate}>
        {#if addError}{addError}{:else}{addHint}{#if addHint && shellCandidate}{" "}{/if}{#if shellCandidate}{TERMINAL_WARNING}{/if}{/if}
      </small>
      <div class="pick">
        <button
          type="button"
          class="compact outline secondary"
          disabled={detecting > 0}
          onclick={(e) => detect(e.currentTarget)}>Detect app…</button
        >
        <button type="button" class="compact outline secondary" onclick={() => fileInput?.click()}>Browse…</button>
        {#if detecting}<span class="countdown" aria-hidden="true">Switch to the app — reading it in {detecting} s</span>{/if}
      </div>
      <input
        type="file"
        accept=".exe"
        tabindex="-1"
        aria-hidden="true"
        class="visually-hidden"
        bind:this={fileInput}
        onchange={(e) => picked(e.currentTarget)}
      />
      <p class="visually-hidden" role="status">{detectLive}</p>
    </form>
    <p class="help muted">Use "Press Enter" only for apps you trust: anyone who can text your phone can trigger it.</p>
    <details class="howto">
      <summary>How do I find an app's name?</summary>
      <p>
        Click <strong>Detect app…</strong>, then switch to the app within 5 seconds. Or use <strong>Browse…</strong> to
        pick its <code>.exe</code>, or open Task Manager → <strong>Details</strong>: the <strong>Name</strong> column
        shows it, e.g. <code>notepad.exe</code>. Citrix sessions run in <code>wfica32.exe</code> or
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
    <div class="hotkey">
      <span id="hotkey-label">Shortcut</span>
      <button
        type="button"
        class="outline secondary"
        class:recording
        aria-labelledby="hotkey-label hotkey-value"
        aria-describedby="hotkey-msg hotkey-help"
        aria-pressed={recording}
        onclick={toggleRecording}
        onkeydown={recordKey}
        onkeyup={recordKeyUp}
        onblur={stopRecording}
      >
        <span id="hotkey-value"
          >{recording ? preview || "Press the new keys… (Esc to cancel)" : showHotkey(settings.hotkey)}</span
        >
      </button>
    </div>
    <small id="hotkey-msg" class="error" aria-live="polite">{recordError || hotkeyWarning}</small>
    <small id="hotkey-help" class="muted">Click, then press the keys, e.g. Ctrl+Shift+K. Backspace = off.</small>
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
  #new-exe-msg {
    display: block;
    line-height: 1.4;
  }
  .pick {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    margin: 0.5rem 0 0.75rem;
  }
  .countdown {
    font-size: 0.875rem;
    font-weight: 600;
  }
  .howto {
    margin-bottom: 0;
  }
  .howto p {
    font-size: 0.875rem;
  }
  .hotkey {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.35rem;
  }
  .hotkey button {
    min-width: 12rem;
    margin: 0;
    padding: 0.4rem 0.9rem;
    font-size: 0.9rem;
    font-weight: 600;
  }
  .hotkey button.recording {
    border-style: dashed;
    border-color: var(--pico-primary);
    color: var(--pico-primary);
    font-weight: normal;
  }
  #hotkey-msg,
  #hotkey-help {
    display: block;
    line-height: 1.4;
  }
</style>
