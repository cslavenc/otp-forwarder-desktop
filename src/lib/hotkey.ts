// Hotkey names follow the global-shortcut parser (pipeline::parse_hotkey); "Super" is shown as "Win".

export type KeyPress = { keyCode: number; ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean };

export const showHotkey = (hotkey: string) => hotkey.replace(/\bSuper\b/g, "Win") || "Off";

export function modifiers(e: Omit<KeyPress, "keyCode">): string[] {
  const m: string[] = [];
  if (e.ctrlKey) m.push("Ctrl");
  if (e.altKey) m.push("Alt");
  if (e.shiftKey) m.push("Shift");
  if (e.metaKey) m.push("Super");
  return m;
}

// keyCode, not key/code: it names the key by its layout label (Swiss Z is "Z") like Windows does.
function mainKey(keyCode: number): string | null {
  if ((keyCode >= 0x41 && keyCode <= 0x5a) || (keyCode >= 0x30 && keyCode <= 0x39)) return String.fromCharCode(keyCode);
  if (keyCode >= 0x70 && keyCode <= 0x87) return `F${keyCode - 0x6f}`;
  return null;
}

/** The hotkey for a non-modifier key press, or why it can't be one. */
export function hotkeyFrom(e: KeyPress): { hotkey: string } | { error: string } {
  const key = mainKey(e.keyCode);
  const mods = modifiers(e);
  if (!key) return { error: "Use a letter, digit or F1–F24." };
  if (mods.length < 2) return { error: "Add at least two of Ctrl, Alt, Shift, Win" };
  if (mods.join("+") === "Ctrl+Alt") return { error: "Ctrl+Alt is AltGr on many keyboards — add Shift or Win" };
  return { hotkey: [...mods, key].join("+") };
}

if (import.meta.env.DEV) {
  const press = (keyCode: number, mods: string) => ({
    keyCode,
    ctrlKey: mods.includes("c"),
    altKey: mods.includes("a"),
    shiftKey: mods.includes("s"),
    metaKey: mods.includes("w"),
  });
  const ok = (keyCode: number, mods: string, hotkey: string) => {
    const got = hotkeyFrom(press(keyCode, mods));
    console.assert("hotkey" in got && got.hotkey === hotkey, "hotkeyFrom", keyCode, mods, got);
  };
  const bad = (keyCode: number, mods: string) => {
    const got = hotkeyFrom(press(keyCode, mods));
    console.assert("error" in got, "hotkeyFrom should fail", keyCode, mods, got);
  };
  ok(0x41, "cs", "Ctrl+Shift+A");
  ok(0x5a, "cs", "Ctrl+Shift+Z");
  ok(0x30, "sw", "Shift+Super+0");
  ok(0x39, "cs", "Ctrl+Shift+9");
  ok(0x70, "cs", "Ctrl+Shift+F1");
  ok(0x87, "casw", "Ctrl+Alt+Shift+Super+F24");
  bad(229, "cs"); // IME composition
  bad(0x4b, "s");
  bad(0x4b, "ca");
  ok(0x4b, "cas", "Ctrl+Alt+Shift+K");
  console.assert(showHotkey("Shift+Super+0") === "Shift+Win+0" && showHotkey("") === "Off", "showHotkey");
}
