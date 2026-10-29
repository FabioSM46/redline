import { invoke } from "@tauri-apps/api/core";
import { homeDir, videoDir } from "@tauri-apps/api/path";
import { useCallback, useEffect, useRef, useState } from "react";
import type { RecordingSettings } from "../../capture.ts";
import { IDLE, type Loaded, type SaveState } from "../settings/Setting.tsx";

/**
 * `Settings` as `get_settings` / `set_settings` return it. Every field but `recording` is sent
 * back exactly as read, including fields added later: the Record tab does not own them.
 */
type StoredSettings = { [field: string]: unknown; recording: RecordingSettings };

/** The recording settings the Record tab owns; the rest live in Settings → Capture. */
export type RecordField = "replay_seconds" | "replay_on_start" | "audio_tracks" | "replay_dir" | "recording_dir";

const ALL_IDLE: Record<RecordField, SaveState> = {
  replay_seconds: IDLE,
  replay_on_start: IDLE,
  audio_tracks: IDLE,
  replay_dir: IDLE,
  recording_dir: IDLE,
};

export type RecordingSettingsState = {
  recording: Loaded<RecordingSettings>;
  saves: Record<RecordField, SaveState>;
  /** A save is running: every control ignores input until it ends. */
  busy: boolean;
  /** Saves one field; resolves true once saved, false when refused (the reason is in `saves`). */
  save: <F extends RecordField>(field: F, value: RecordingSettings[F]) => Promise<boolean>;
};

/**
 * The recording settings, saved one field at a time with the pattern of Settings → Capture:
 * every save re-reads `get_settings` and changes only its own field on that fresh copy, so a
 * value changed elsewhere since the tab opened (Capture, General) is sent back as it is now,
 * and so is any top-level field this tab does not know.
 * One save at a time; a failed save keeps the saved value and puts the reason in `saves`.
 */
export function useRecordingSettings(): RecordingSettingsState {
  const [recording, setRecording] = useState<Loaded<RecordingSettings>>({ status: "loading" });
  const [saves, setSaves] = useState(ALL_IDLE);
  const busyRef = useRef(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    invoke<StoredSettings>("get_settings").then(
      (value) => mounted.current && setRecording({ status: "ready", value: value.recording }),
      (e) => mounted.current && setRecording({ status: "error", message: String(e) }),
    );
    return () => {
      mounted.current = false;
    };
  }, []);

  const save = useCallback(<F extends RecordField>(field: F, value: RecordingSettings[F]) => {
    if (busyRef.current) return Promise.resolve(false);
    busyRef.current = true;
    const setSave = (state: SaveState) => mounted.current && setSaves((all) => ({ ...all, [field]: state }));
    setSave({ status: "saving" });
    return invoke<StoredSettings>("get_settings")
      .then((fresh) => {
        const recording = { ...fresh.recording, [field]: value };
        return invoke<StoredSettings>("set_settings", { settings: { ...fresh, recording } });
      })
      .then(
        (saved) => {
          if (mounted.current) setRecording({ status: "ready", value: saved.recording });
          setSave(IDLE);
          return true;
        },
        (e) => {
          setSave({ status: "error", message: String(e) });
          return false;
        },
      )
      .finally(() => {
        busyRef.current = false;
      });
  }, []);

  const busy = Object.values(saves).some((s) => s.status === "saving");
  return { recording, saves, busy, save };
}

/**
 * The video folder the default replay and recording folders live under, asked of Tauri as the
 * backend does (`video_dir`, then `<home>/Videos`); null when neither is known.
 */
export function useVideoDir(): string | null {
  const [dir, setDir] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    videoDir()
      .catch(() => homeDir().then((home) => `${home.replace(/\/+$/, "")}/Videos`))
      .then(
        (value) => active && setDir(value),
        () => {},
      );
    return () => {
      active = false;
    };
  }, []);
  return dir;
}
