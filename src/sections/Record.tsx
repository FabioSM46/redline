import { invoke } from "@tauri-apps/api/core";
import { type ReactNode, useEffect, useState } from "react";
import type { RecordingSettings } from "../capture.ts";
import { defaultFolder, formatElapsed, formatLength, replayLengthOptions } from "../record.ts";
import { type RecorderStatus, replayIsOn, unavailableNotice } from "../recorder.ts";
import { useRecorder } from "../RecorderContext.tsx";
import { type AudioSources, AudioTracks } from "./record/AudioTracks.tsx";
import { Folder } from "./record/Folder.tsx";
import { Gallery } from "./record/Gallery.tsx";
import { type RecordingSettingsState, useRecordingSettings, useVideoDir } from "./record/useRecordingSettings.ts";
import { type Loaded, Setting } from "./settings/Setting.tsx";

/**
 * Replay, recording and the gallery (D8). Codec, quality, frame rate and the screen live in
 * Settings → Capture; this tab holds what changes from one session to the next.
 */
export function Record() {
  const { recorder } = useRecorder();
  const settings = useRecordingSettings();
  const videoDir = useVideoDir();
  const [sources, setSources] = useState<Loaded<AudioSources>>({ status: "loading" });

  const status = recorder.status === "ready" ? recorder.value : null;
  const available = status?.available ?? false;
  // Listed again when GPU Screen Recorder becomes available (the first detection, an install).
  useEffect(() => {
    let active = true;
    setSources({ status: "loading" });
    invoke<AudioSources>("list_audio_sources").then(
      (value) => active && setSources({ status: "ready", value }),
      (e) => active && setSources({ status: "error", message: String(e) }),
    );
    return () => {
      active = false;
    };
  }, [available]);

  const notice = status === null ? null : unavailableNotice(status);
  // A settings change restarts a running replay, and that ends a recording made inside it.
  const recordingInReplay = status !== null && status.recording && replayIsOn(status);
  const lock: Lock = {
    disabled: status === null || notice !== null || recordingInReplay,
    reason: recordingInReplay
      ? "Stop the recording to change these settings: a change restarts the replay, which ends the recording."
      : null,
  };

  return (
    <section className="section record">
      {recorder.status === "loading" && <p className="state">Reading the recorder…</p>}
      {recorder.status === "error" && (
        <p className="setting-error" role="alert">
          Cannot read the recorder: {recorder.message}
        </p>
      )}
      {notice !== null && (
        <p className="settings-notice" role="status">
          {notice} Until then, replays and recordings cannot be started or set up. Saved clips are listed below.
        </p>
      )}
      <ReplayBlock status={status} settings={settings} sources={sources} videoDir={videoDir} lock={lock} />
      <RecordingBlock status={status} settings={settings} videoDir={videoDir} lock={lock} />
      <Gallery />
    </section>
  );
}

/** Whether the settings rows take input, and the sentence that says why not when it is not obvious. */
type Lock = { disabled: boolean; reason: string | null };

function ReplayBlock({
  status,
  settings,
  sources,
  videoDir,
  lock,
}: {
  status: RecorderStatus | null;
  settings: RecordingSettingsState;
  sources: Loaded<AudioSources>;
  videoDir: string | null;
  lock: Lock;
}) {
  const { recording, saves, busy, save } = settings;
  return (
    <div className="record-block">
      <h2 className="record-title">Instant replay</h2>
      <p className="setting-description record-intro">
        Keeps the last moments of the screen in memory, ready to be saved. Changing a setting restarts a running
        replay, which empties it.
      </p>
      {lock.reason !== null && (
        <p className="setting-note record-intro" role="status">
          {lock.reason}
        </p>
      )}
      <div className="settings">
        <ReplaySwitch status={status} />
        <Rows state={recording}>
          {(current) => (
            <>
              <Setting
                title="Start with Redline"
                description="Turns the replay on when Redline starts, also when it starts hidden in the tray."
                save={saves.replay_on_start}
              >
                <input
                  type="checkbox"
                  role="switch"
                  className="switch"
                  aria-label="Start the replay with Redline"
                  checked={current.replay_on_start}
                  disabled={lock.disabled}
                  aria-disabled={busy}
                  onChange={(event) => {
                    if (!busy) save("replay_on_start", event.target.checked);
                  }}
                />
              </Setting>
              <Setting
                title="Length"
                description="How much the replay keeps. Longer replays use more memory."
                save={saves.replay_seconds}
              >
                <select
                  className="setting-select"
                  aria-label="Replay length"
                  value={current.replay_seconds}
                  disabled={lock.disabled}
                  aria-disabled={busy}
                  onChange={(event) => {
                    if (!busy) save("replay_seconds", Number(event.target.value));
                  }}
                >
                  {replayLengthOptions(current.replay_seconds).map((seconds) => (
                    <option key={seconds} value={seconds}>
                      {formatLength(seconds)}
                    </option>
                  ))}
                </select>
              </Setting>
              <AudioTracks
                tracks={current.audio_tracks}
                sources={sources}
                save={saves.audio_tracks}
                disabled={lock.disabled}
                busy={busy}
                onSave={(tracks) => save("audio_tracks", tracks)}
              />
              <Folder
                title="Replay folder"
                description="Where saved replays go."
                saved={current.replay_dir}
                defaultValue={defaultFolder(videoDir, "replay")}
                save={saves.replay_dir}
                disabled={lock.disabled}
                busy={busy}
                onSave={(folder) => save("replay_dir", folder)}
              />
            </>
          )}
        </Rows>
      </div>
    </div>
  );
}

/** The replay switch (the same state as the status bar and Home), and saving what it holds. */
function ReplaySwitch({ status }: { status: RecorderStatus | null }) {
  const { busy, actionError, toggleReplay, saveReplay } = useRecorder();
  const unavailable = status === null || !status.available;
  const on = status !== null && replayIsOn(status);
  const error = actionError ?? (status?.replay === "error" ? status.error : null);
  return (
    <div className="setting">
      <div className="setting-text">
        <span className="setting-title">Instant replay</span>
        <span className="setting-description">
          {status === null
            ? "Reading the recorder…"
            : status.replay === "starting"
              ? "Starting…"
              : on
                ? `On, keeping the last ${formatLength(status.replay_seconds)}.`
                : "Off."}
        </span>
        {error !== null && (
          <span className="setting-error" role="alert">
            {error}
          </span>
        )}
        {status?.warning != null && <span className="setting-description">{status.warning}</span>}
      </div>
      <div className="setting-control record-controls">
        {status?.replay === "on" && (
          <button type="button" className="button" aria-disabled={busy} onClick={() => !busy && saveReplay()}>
            Save replay
          </button>
        )}
        <input
          type="checkbox"
          role="switch"
          className="switch"
          aria-label="Instant replay"
          checked={on}
          disabled={unavailable}
          aria-disabled={busy || unavailable}
          onChange={() => {
            if (!busy) toggleReplay();
          }}
        />
      </div>
    </div>
  );
}

function RecordingBlock({
  status,
  settings,
  videoDir,
  lock,
}: {
  status: RecorderStatus | null;
  settings: RecordingSettingsState;
  videoDir: string | null;
  lock: Lock;
}) {
  const { busy, recordingError, recordingSince, toggleRecording } = useRecorder();
  const recording = status?.recording ?? false;
  const now = useNow(recording && recordingSince !== null);
  const unavailable = status === null || !status.available;
  // The replay process serves requests only once it runs: the backend refuses a start meanwhile.
  const starting = status?.replay === "starting";
  const blocked = busy || (starting && !recording);

  return (
    <div className="record-block">
      <h2 className="record-title">Recording</h2>
      <div className="settings">
        <div className="setting">
          <div className="setting-text">
            <span className="setting-title">
              {recording ? "Recording" : "Not recording"}
              {recording && recordingSince !== null && (
                <span className="record-elapsed" role="timer">
                  {" "}
                  ⏺ {formatElapsed(now - recordingSince)}
                </span>
              )}
            </span>
            <span className="setting-description">
              {recording
                ? "Recording until you stop it."
                : status !== null && replayIsOn(status)
                  ? "Records a file while the replay keeps running."
                  : "Records the screen into a file until you stop it."}
            </span>
            {starting && !recording && (
              <span className="setting-note">The replay is still starting: wait for it before recording.</span>
            )}
            {recordingError !== null && (
              <span className="setting-error" role="alert">
                {recordingError}
              </span>
            )}
          </div>
          <div className="setting-control">
            <button
              type="button"
              className={recording ? "button button-stop" : "button"}
              disabled={unavailable && !recording}
              aria-disabled={blocked}
              onClick={() => {
                if (!blocked) toggleRecording();
              }}
            >
              {recording ? "Stop recording" : "Start recording"}
            </button>
          </div>
        </div>
        <Rows state={settings.recording}>
          {(current) => (
            <Folder
              title="Recording folder"
              description="Where recordings go, with or without the replay."
              saved={current.recording_dir}
              defaultValue={defaultFolder(videoDir, "recording")}
              save={settings.saves.recording_dir}
              disabled={lock.disabled}
              busy={settings.busy}
              onSave={(folder) => settings.save("recording_dir", folder)}
            />
          )}
        </Rows>
      </div>
    </div>
  );
}

/** The settings rows once the settings are read; a state line while loading or when unreadable. */
function Rows({ state, children }: { state: Loaded<RecordingSettings>; children: (value: RecordingSettings) => ReactNode }) {
  if (state.status === "loading") return <p className="setting-description">Reading the settings…</p>;
  if (state.status === "error") {
    return (
      <p className="setting-error" role="alert">
        Cannot read the settings: {state.message}
      </p>
    );
  }
  return <>{children(state.value)}</>;
}

/** The current time, ticking every second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}
