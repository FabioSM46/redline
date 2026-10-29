// The interface theme (Settings → General). The saved setting is `system`, `light` or `dark`;
// the root element always carries the resolved `data-theme="light" | "dark"` once the setting
// is known, and `src/App.css` picks the tokens from it. Before that (no attribute) the CSS
// follows `prefers-color-scheme` on its own.

/** The theme as saved by `set_settings` (`Theme` in src-tauri/src/settings.rs). */
export type ThemeSetting = "system" | "light" | "dark";

/** The value of the `data-theme` attribute on the root element. */
export type ResolvedTheme = "light" | "dark";

export const THEME_OPTIONS: readonly { value: ThemeSetting; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** A saved value outside the three known ones counts as `system`, like the backend does. */
export function parseThemeSetting(value: unknown): ThemeSetting {
  return value === "light" || value === "dark" ? value : "system";
}

/** The attribute for a setting: `system` follows the desktop's preference. */
export function resolveTheme(setting: ThemeSetting, prefersDark: boolean): ResolvedTheme {
  if (setting === "system") return prefersDark ? "dark" : "light";
  return setting;
}

const DARK_QUERY = "(prefers-color-scheme: dark)";
let stopFollowingSystem: (() => void) | null = null;

/**
 * Sets `data-theme` on `document.documentElement`. With `system` it also follows later changes
 * of the desktop preference, so anything watching the attribute (the charts) sees them too.
 */
export function applyTheme(setting: ThemeSetting): void {
  stopFollowingSystem?.();
  stopFollowingSystem = null;
  const root = document.documentElement;
  const media = window.matchMedia(DARK_QUERY);
  root.dataset.theme = resolveTheme(setting, media.matches);
  if (setting !== "system") return;
  const onChange = (event: MediaQueryListEvent) => {
    root.dataset.theme = resolveTheme("system", event.matches);
  };
  media.addEventListener("change", onChange);
  stopFollowingSystem = () => media.removeEventListener("change", onChange);
}
