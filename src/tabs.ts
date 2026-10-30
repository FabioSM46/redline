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

export const SETTINGS_TABS = [
  { id: "general", label: "General" },
  { id: "capture", label: "Capture" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "dependencies", label: "Dependencies" },
  { id: "about", label: "About" },
] as const;

export type SettingsTabId = (typeof SETTINGS_TABS)[number]["id"];

/** Opens a tab; for Settings, optionally on one of its pages (Tuning → Settings → Dependencies). */
export type Navigate = (tab: TabId, settingsTab?: SettingsTabId) => void;

/** What the shell gives every section: Home's "Show all" opens the Record tab with it. */
export type SectionProps = {
  navigate: Navigate;
  /** The Settings page the last `navigate` asked for; the others ignore it. */
  settingsTab?: SettingsTabId;
};
