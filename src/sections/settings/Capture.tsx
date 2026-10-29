import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import {
  type CaptureTarget,
  captureChangeNotice,
  captureExplanation,
  captureOptions,
  CODEC_GROUPS,
  fpsOptions,
  QUALITY_OPTIONS,
  type RecordingSettings,
} from "../../capture.ts";
import { IDLE, type Loaded, type SaveState, Setting, Value } from "./Setting.tsx";

/**
 * `Settings` as `get_settings` / `set_settings` return it. `theme` and `start_minimised` are sent
 * back unchanged: `set_settings` requires them, and this page does not own them.
 */
type StoredSettings = { theme: unknown; start_minimised: unknown; recording: RecordingSettings };

/** `list_capture_targets` (`Choices` in src-tauri/src/sources.rs). */
type CaptureTargets = { available: boolean; unavailable_reason: string | null; items: CaptureTarget[] };

type Field = "capture" | "codec" | "quality" | "fps";

const ALL_IDLE: Record<Field, SaveState> = { capture: IDLE, codec: IDLE, quality: IDLE, fps: IDLE };

/** Screen, codec, quality and frame rate of replays and recordings (D8). */
export function Capture() {
  const [settings, setSettings] = useState<Loaded<StoredSettings>>({ status: "loading" });
  const [targets, setTargets] = useState<Loaded<CaptureTargets>>({ status: "loading" });
  const [saves, setSaves] = useState(ALL_IDLE);
  const [captureNote, setCaptureNote] = useState<string | null>(null);

  useEffect(() => {
    invoke<StoredSettings>("get_settings").then(
      (value) => setSettings({ status: "ready", value }),
      (e) => setSettings({ status: "error", message: String(e) }),
    );
    invoke<CaptureTargets>("list_capture_targets").then(
      (value) => setTargets({ status: "ready", value }),
      (e) => setTargets({ status: "error", message: String(e) }),
    );
  }, []);

  // Every control sends the whole `recording` object, so all four are busy while one saves: a
  // second change never overwrites the first with stale values. Busy is `aria-disabled` plus an
  // early return, so keyboard focus stays on the control. On failure the shown value stays the
  // saved one and the reason sits next to it.
  const busy = Object.values(saves).some((s) => s.status === "saving");
  // Without GPU Screen Recorder nothing here has an effect: the page says what to install.
  const missing = targets.status === "ready" && !targets.value.available;

  function save(field: Field, current: StoredSettings, recording: RecordingSettings) {
    if (busy || missing) return;
    const setSave = (state: SaveState) => setSaves((all) => ({ ...all, [field]: state }));
    setSave({ status: "saving" });
    const { theme, start_minimised } = current;
    invoke<StoredSettings>("set_settings", { settings: { theme, start_minimised, recording } }).then(
      (saved) => {
        setSettings({ status: "ready", value: saved });
        setSave(IDLE);
        if (field === "capture") {
          setCaptureNote(captureChangeNotice(current.recording.capture, saved.recording.capture));
        }
      },
      (e) => setSave({ status: "error", message: String(e) }),
    );
  }

  const unavailableReason = targets.status === "ready" ? targets.value.unavailable_reason : null;

  return (
    <div className="settings">
      {missing && (
        <p className="settings-notice" role="status">
          {unavailableReason ?? "GPU Screen Recorder is not installed."} Until then these settings cannot be changed.
        </p>
      )}

      <Setting
        title="Screen"
        description={
          settings.status === "ready"
            ? captureExplanation(settings.value.recording.capture)
            : "Which screen replays and recordings capture."
        }
        save={saves.capture}
        note={captureNote}
      >
        <Value state={settings} what="the screen">
          {(current) =>
            targets.status === "error" ? (
              <span className="setting-error" role="alert">
                Cannot list the screens: {targets.message}
              </span>
            ) : targets.status === "loading" ? (
              <span className="setting-description">Loading…</span>
            ) : (
              <select
                className="setting-select"
                aria-label="Screen"
                value={current.recording.capture}
                disabled={missing}
                aria-disabled={busy}
                onChange={(event) =>
                  save("capture", current, { ...current.recording, capture: event.target.value })
                }
              >
                {captureOptions(targets.value.items, current.recording.capture).map((target) => (
                  <option key={target.id} value={target.id}>
                    {target.label}
                  </option>
                ))}
              </select>
            )
          }
        </Value>
      </Setting>

      <Setting
        title="Codec"
        description="Automatic lets GPU Screen Recorder pick what the GPU encodes best. HDR codecs do not work with the portal."
        save={saves.codec}
      >
        <Value state={settings} what="the codec">
          {(current) => (
            <select
              className="setting-select"
              aria-label="Codec"
              value={current.recording.codec ?? ""}
              disabled={missing}
              aria-disabled={busy}
              onChange={(event) =>
                save("codec", current, { ...current.recording, codec: event.target.value || null })
              }
            >
              {CODEC_GROUPS.map((group) => {
                const options = group.options.map((option) => (
                  <option key={option.value ?? ""} value={option.value ?? ""}>
                    {option.label}
                  </option>
                ));
                return group.label === null ? (
                  options
                ) : (
                  <optgroup key={group.label} label={group.label}>
                    {options}
                  </optgroup>
                );
              })}
            </select>
          )}
        </Value>
      </Setting>

      <Setting title="Quality" description="Higher quality makes larger files." save={saves.quality}>
        <Value state={settings} what="the quality">
          {(current) => (
            <select
              className="setting-select"
              aria-label="Quality"
              value={current.recording.quality}
              disabled={missing}
              aria-disabled={busy}
              onChange={(event) => save("quality", current, { ...current.recording, quality: event.target.value })}
            >
              {QUALITY_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          )}
        </Value>
      </Setting>

      <Setting
        title="Frame rate"
        description="Frames per second captured. More than the monitor's refresh rate brings no benefit."
        save={saves.fps}
      >
        <Value state={settings} what="the frame rate">
          {(current) => (
            <select
              className="setting-select"
              aria-label="Frame rate"
              value={current.recording.fps}
              disabled={missing}
              aria-disabled={busy}
              onChange={(event) => save("fps", current, { ...current.recording, fps: Number(event.target.value) })}
            >
              {fpsOptions(current.recording.fps).map((fps) => (
                <option key={fps} value={fps}>
                  {fps} fps
                </option>
              ))}
            </select>
          )}
        </Value>
      </Setting>
    </div>
  );
}
