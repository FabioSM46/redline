// Settings → Shortcuts (D5, D8): the commands to bind in the desktop's custom shortcuts until the
// GlobalShortcuts portal is available.

/** The executable the `.deb` installs (`/usr/bin/redline`). */
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

/** Writes `text` to the clipboard; rejects with a readable reason when there is none. */
export function copyText(clipboard: Clipboard | undefined, text: string): Promise<void> {
  if (!clipboard || typeof clipboard.writeText !== "function") {
    return Promise.reject(new Error("the clipboard is not available"));
  }
  return clipboard.writeText(text);
}

/** The sentence shown when a copy fails, from whatever the Clipboard API rejected with. */
export function copyError(e: unknown): string {
  // DOMException is an Error; its messages often end with a full stop of their own.
  const reason = (e instanceof Error ? e.message : String(e)).trim().replace(/\.+$/, "");
  return `Not copied: ${reason || "unknown error"}. Select the command and copy it by hand.`;
}
