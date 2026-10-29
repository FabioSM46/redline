// Settings → Shortcuts (D5, D8): the commands to bind in the desktop's custom shortcuts until the
// GlobalShortcuts portal is available.

/**
 * The executable the `.deb` installs (`/usr/bin/redline`): the Cargo package name, which is also
 * the Linux `productName` in src-tauri/tauri.linux.conf.json. `shortcuts.test.ts` checks both.
 */
export const EXECUTABLE = "redline";

/**
 * Mirror of `ACTION_ARGS` in src-tauri/src/actions.rs, in the same order: the backend accepts an
 * argument only if it is in that list (exact match, case-sensitive). `shortcuts.test.ts` reads
 * actions.rs and fails if the two drift.
 */
export const SHORTCUT_ACTIONS = [
  {
    arg: "--save-replay",
    action: "SaveReplay",
    title: "Save replay",
    description: "Saves the last seconds of the replay buffer.",
  },
  {
    arg: "--toggle-replay",
    action: "ToggleReplay",
    title: "Toggle replay",
    description: "Starts the replay buffer, or stops it when it is on.",
  },
  {
    arg: "--toggle-recording",
    action: "ToggleRecording",
    title: "Toggle recording",
    description: "Starts a recording, or stops the one in progress.",
  },
] as const;

/** The full command line to bind for one action argument. */
export function shortcutCommand(arg: string): string {
  return `${EXECUTABLE} ${arg}`;
}

/** Where each desktop keeps its custom shortcuts. */
export const DESKTOP_STEPS = [
  {
    desktop: "GNOME",
    steps: "Settings → Keyboard → View and Customise Shortcuts → Custom Shortcuts → Add Shortcut",
  },
  {
    desktop: "KDE Plasma",
    steps: "System Settings → Shortcuts → Custom Shortcuts (or Add New → Command)",
  },
] as const;

/** The part of the Clipboard API used here. */
export type Clipboard = { writeText: (text: string) => Promise<void> };

/**
 * Writes `text` to the clipboard. The Clipboard API comes first; when it is missing (some WebKitGTK
 * builds) or refuses, `fallback` (a synchronous copy, see `copyWithSelection`) is tried. Always
 * returns a promise, even when `writeText` or `fallback` throws synchronously; it rejects with the
 * Clipboard API's reason, or a readable one when there is no clipboard at all.
 */
export function copyText(
  clipboard: Clipboard | undefined,
  text: string,
  fallback?: (text: string) => boolean,
): Promise<void> {
  const tryFallback = (reason: unknown): Promise<void> => {
    try {
      if (fallback?.(text)) return Promise.resolve();
    } catch {
      // The fallback failing is reported as the original reason.
    }
    return Promise.reject(reason);
  };
  if (!clipboard || typeof clipboard.writeText !== "function") {
    return tryFallback(new Error("the clipboard is not available"));
  }
  try {
    return Promise.resolve(clipboard.writeText(text)).catch(tryFallback);
  } catch (e) {
    return tryFallback(e);
  }
}

/** The sentence shown when a copy fails, from whatever the Clipboard API rejected with. */
export function copyError(e: unknown): string {
  // DOMException is an Error; its messages often end with a full stop of their own.
  const reason = (e instanceof Error ? e.message : String(e)).trim().replace(/\.+$/, "");
  return `Not copied: ${reason || "unknown error"}. Select the command and copy it by hand.`;
}
