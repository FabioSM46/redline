// Settings → Shortcuts (D5, D8, D10): the keys the GlobalShortcuts portal reports where the desktop
// has it, and the commands to bind in the desktop's custom shortcuts everywhere else.

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

/**
 * The event `global_shortcuts.rs` sends with every new `PortalShortcuts` (`STATUS_EVENT` there;
 * `shortcuts.test.ts` checks the two match).
 */
export const PORTAL_SHORTCUTS_EVENT = "global-shortcuts";

/** One action and the keys the desktop reports for it; `trigger` is null when nothing is bound. */
export type PortalBinding = { arg: string; trigger: string | null };

/** `ShortcutsStatus` in src-tauri/src/global_shortcuts.rs. */
export type PortalShortcuts =
  | { state: "checking" }
  | { state: "unavailable" }
  | { state: "ready"; version: number; can_configure: boolean; bindings: PortalBinding[] }
  | { state: "failed"; error: string };

/** What a portal row shows for an action. */
export type PortalRow = { arg: string; title: string; description: string; trigger: string | null };

/**
 * One row per `SHORTCUT_ACTIONS` entry, in that order, with the keys the portal reports for it.
 * Bindings for arguments Redline does not know are ignored; a missing or blank trigger is null.
 * A payload without a bindings list (a backend that drifted from this mirror) shows every action
 * as unbound instead of breaking the screen.
 */
export function portalRows(bindings: readonly PortalBinding[] | null | undefined): PortalRow[] {
  const reported = Array.isArray(bindings) ? bindings : [];
  return SHORTCUT_ACTIONS.map(({ arg, title, description }) => {
    const found = reported.find((b) => b?.arg === arg)?.trigger;
    const trigger = typeof found === "string" ? found.trim() : null;
    return { arg, title, description, trigger: trigger ? trigger : null };
  });
}
