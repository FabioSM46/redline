import { invoke } from "@tauri-apps/api/core";
import { type ReactNode, useEffect, useState } from "react";
import { applyTheme, parseThemeSetting, THEME_OPTIONS, type ThemeSetting } from "../../theme.ts";

/** `Settings` in src-tauri/src/settings.rs, as `get_settings` / `set_settings` return it. */
type Settings = { theme: ThemeSetting; start_minimised: boolean };

/** A value read from the backend: loading, unreadable, or known. */
type Loaded<T> = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; value: T };

/** What a control is doing after a change: nothing, saving, or the reason the last save failed. */
type SaveState = { status: "idle" } | { status: "saving" } | { status: "error"; message: string };

const IDLE: SaveState = { status: "idle" };

function toSettings(raw: { theme: unknown; start_minimised: unknown }): Settings {
  return { theme: parseThemeSetting(raw.theme), start_minimised: raw.start_minimised === true };
}

/** Theme, start with the system, start minimised to the tray (D8). */
export function General() {
  const [settings, setSettings] = useState<Loaded<Settings>>({ status: "loading" });
  const [themeSave, setThemeSave] = useState<SaveState>(IDLE);
  const [minimisedSave, setMinimisedSave] = useState<SaveState>(IDLE);
  const [autostart, setAutostart] = useState<Loaded<boolean>>({ status: "loading" });
  const [autostartSave, setAutostartSave] = useState<SaveState>(IDLE);
  const trayAvailable = useTrayAvailable();

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then((raw) => setSettings({ status: "ready", value: toSettings(raw) }))
      .catch((e) => setSettings({ status: "error", message: String(e) }));
    invoke<boolean>("get_autostart")
      .then((value) => setAutostart({ status: "ready", value }))
      .catch((e) => setAutostart({ status: "error", message: String(e) }));
  }, []);

  // Theme and start minimised travel together in one `set_settings` call, so both controls are
  // disabled while either saves: a second change never overwrites the first with stale values.
  // On failure the shown value stays the previous one and the error sits next to the control.
  const settingsBusy = themeSave.status === "saving" || minimisedSave.status === "saving";

  function saveSettings(next: Settings, setSave: (state: SaveState) => void) {
    setSave({ status: "saving" });
    invoke<Settings>("set_settings", { settings: next })
      .then((raw) => {
        const saved = toSettings(raw);
        setSettings({ status: "ready", value: saved });
        applyTheme(saved.theme);
        setSave(IDLE);
      })
      .catch((e) => setSave({ status: "error", message: String(e) }));
  }

  // The shown state is the one the operating system reports back, not the requested one.
  function saveAutostart(enabled: boolean) {
    setAutostartSave({ status: "saving" });
    invoke<boolean>("set_autostart", { enabled })
      .then((value) => {
        setAutostart({ status: "ready", value });
        setAutostartSave(IDLE);
      })
      .catch((e) => setAutostartSave({ status: "error", message: String(e) }));
  }

  return (
    <div className="settings">
      <Setting title="Theme" description="System follows the desktop's light or dark preference." save={themeSave}>
        <Value state={settings} what="the theme">
          {(current) => (
            <div className="segmented" role="radiogroup" aria-label="Theme">
              {THEME_OPTIONS.map((option) => (
                <label className="segment" key={option.value}>
                  <input
                    type="radio"
                    name="theme"
                    value={option.value}
                    checked={current.theme === option.value}
                    disabled={settingsBusy}
                    onChange={() => saveSettings({ ...current, theme: option.value }, setThemeSave)}
                  />
                  <span>{option.label}</span>
                </label>
              ))}
            </div>
          )}
        </Value>
      </Setting>

      <Setting title="Start with the system" description="Open Redline when you log in." save={autostartSave}>
        <Value state={autostart} what="whether Redline starts with the system">
          {(enabled) => (
            <Toggle
              label="Start with the system"
              checked={enabled}
              disabled={autostartSave.status === "saving"}
              onChange={saveAutostart}
            />
          )}
        </Value>
      </Setting>

      <Setting
        title="Start minimised to the tray"
        description="When Redline starts with the system, keep the window hidden and show only the tray icon."
        save={minimisedSave}
      >
        <Value state={settings} what="this setting">
          {(current) => (
            <Toggle
              label="Start minimised to the tray"
              checked={current.start_minimised}
              disabled={settingsBusy}
              onChange={(start_minimised) => saveSettings({ ...current, start_minimised }, setMinimisedSave)}
            />
          )}
        </Value>
      </Setting>

      {trayAvailable === false && (
        <p className="settings-notice" role="status">
          No tray icon is shown in this session: closing the window quits Redline, and “Start minimised to
          the tray” is ignored. On GNOME, install and enable an AppIndicator extension (for example
          “AppIndicator and KStatusNotifierItem Support”); this notice goes away once the icon is shown.
        </p>
      )}
    </div>
  );
}

/**
 * `app_status().tray_available`, read when the page opens and again whenever the window comes
 * back to the front: the tray host can appear or go away while Redline runs. `null` until the
 * first answer, and when the answer cannot be read: no notice is better than a wrong one.
 */
function useTrayAvailable(): boolean | null {
  const [available, setAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    let current = true;
    const read = () => {
      invoke<{ tray_available: boolean }>("app_status")
        .then((status) => current && setAvailable(status.tray_available === true))
        .catch(() => current && setAvailable(null));
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") read();
    };
    read();
    window.addEventListener("focus", read);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      current = false;
      window.removeEventListener("focus", read);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
  return available;
}

function Setting({
  title,
  description,
  save,
  children,
}: {
  title: string;
  description: string;
  save: SaveState;
  children: ReactNode;
}) {
  return (
    <div className="setting">
      <div className="setting-text">
        <span className="setting-title">{title}</span>
        <span className="setting-description">{description}</span>
        {save.status === "error" && (
          <span className="setting-error" role="alert">
            Not saved: {save.message}
          </span>
        )}
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

/** The control once its value is known; a short state line while loading or when unreadable. */
function Value<T>({ state, what, children }: { state: Loaded<T>; what: string; children: (value: T) => ReactNode }) {
  if (state.status === "loading") return <span className="setting-description">Loading…</span>;
  if (state.status === "error") {
    return (
      <span className="setting-error" role="alert">
        Cannot read {what}: {state.message}
      </span>
    );
  }
  return <>{children(state.value)}</>;
}

function Toggle({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      role="switch"
      className="switch"
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onChange={(event) => onChange(event.target.checked)}
    />
  );
}
