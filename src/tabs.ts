// The shell's navigation, fixed by D8 in docs/decisions.md. No router: the active tab is state.

export const TABS = [
  { id: "home", label: "Home" },
  { id: "performance", label: "Performance" },
  { id: "record", label: "Record" },
  { id: "settings", label: "Settings" },
] as const;

export type TabId = (typeof TABS)[number]["id"];

export const PERFORMANCE_TABS = [
  { id: "metrics", label: "Metrics" },
  { id: "tuning", label: "Tuning" },
] as const;

export type PerformanceTabId = (typeof PERFORMANCE_TABS)[number]["id"];

/** Capture and Shortcuts (D8) join this list when their milestone arrives. */
export const SETTINGS_TABS = [{ id: "general", label: "General" }] as const;

export type SettingsTabId = (typeof SETTINGS_TABS)[number]["id"];
