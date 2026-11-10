// Mirrors `is_bare_exe_name` in src-tauri/src/settings.rs.
export const MAX_EXE_CHARS = 64;

/** A bare `.exe` file name from a typed name or pasted path, or why it can't be used. */
export function normalizeExe(raw: string): { exe: string } | { error: string } {
  let name = raw.trim();
  if (name.length >= 2 && name.startsWith('"') && name.endsWith('"')) name = name.slice(1, -1).trim();
  // Windows ignores trailing dots and spaces in file names.
  name = (name.split(/[\\/]/).pop() ?? "").replace(/[. ]+$/, "");
  if (!name) return { error: "Enter an app name, e.g. notepad" };
  const ext = /\.([a-z0-9]{1,4})$/i.exec(name)?.[1].toLowerCase();
  if (ext && ext !== "exe") return { error: `${name} isn't a program — use the .exe it starts` };
  if (!ext) name += ".exe";
  const chars = [...name].length;
  if (chars < 5 || chars > MAX_EXE_CHARS || /[\\/:"<>|?*\p{Cc}]/u.test(name))
    return { error: `Use a file name like notepad.exe (at most ${MAX_EXE_CHARS} characters)` };
  return { exe: name };
}

if (import.meta.env.DEV) {
  const ok = (raw: string, exe: string) => {
    const got = normalizeExe(raw);
    console.assert("exe" in got && got.exe === exe, "normalizeExe", raw, got);
  };
  const bad = (raw: string) => {
    const got = normalizeExe(raw);
    console.assert("error" in got, "normalizeExe should fail", raw, got);
  };
  ok("notepad", "notepad.exe");
  ok('"C:\\Program Files\\Citrix\\ICA Client\\wfica32.exe"', "wfica32.exe");
  ok("notepad.", "notepad.exe");
  bad("x.lnk");
  bad(".exe");
  bad(`${"a".repeat(MAX_EXE_CHARS - 3)}.exe`);
  bad("C:x.exe");
  bad('"C:\\a\\app.exe" --x');
}
