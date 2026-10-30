import { invoke } from "@tauri-apps/api/core";
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import {
  BACKUP_FILE,
  chosenMetrics,
  hasOwnFigures,
  isLastMetric,
  OVERLAY_METRICS,
  OVERLAY_POSITIONS,
  OVERLAY_PRESETS,
  type OverlayPosition,
  type OverlaySettings,
  type OverlayStatus,
  overlayDescription,
  savedNote,
  toggleMetric,
} from "../../overlay.ts";
import type { Navigate } from "../../tabs.ts";
import { IDLE, type Loaded, type SaveState, Setting } from "../settings/Setting.tsx";
import { CommandRow } from "../settings/Shortcuts.tsx";

/**
 * The in-game overlay (D12): what MangoHud shows, where, its key and the launch option that turns
 * it on for a game. Every setting travels in one `set_overlay` call and the page shows the status
 * it answers with. A `MangoHud.conf` of the user's stays as it is until they agree to a take-over,
 * asked in the page.
 */
export function Overlay({ navigate }: { navigate: Navigate }) {
  const [status, setStatus] = useState<Loaded<OverlayStatus>>({ status: "loading" });

  const load = useCallback(() => {
    setStatus({ status: "loading" });
    return invoke<OverlayStatus>("overlay_status").then(
      (value) => setStatus({ status: "ready", value }),
      (e) => setStatus({ status: "error", message: String(e) }),
    );
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="overlay">
      <h2 className="record-title">Overlay</h2>
      {status.status === "loading" && <p className="setting-description">Reading the overlay settings…</p>}
      {status.status === "error" && (
        <div className="setting setting-wide">
          <div className="setting-text">
            <span className="setting-error" role="alert">
              Cannot read the overlay settings: {status.message}
            </span>
          </div>
          <div className="setting-control">
            <button type="button" className="button" onClick={() => void load()}>
              Check again
            </button>
          </div>
        </div>
      )}
      {status.status === "ready" &&
        (status.value.available ? (
          <OverlayControls status={status.value} onStatus={(value) => setStatus({ status: "ready", value })} />
        ) : (
          <Missing status={status.value} navigate={navigate} reload={() => void load()} />
        ))}
    </div>
  );
}

/** MangoHud is not here: what to do, and where the tools are installed. */
function Missing({ status, navigate, reload }: { status: OverlayStatus; navigate: Navigate; reload: () => void }) {
  return (
    <div className="settings">
      <div className="settings-notice tuning-unavailable" role="status">
        <p>{status.unavailable_reason ?? "The overlay is not available."}</p>
        <div className="tuning-actions">
          <button type="button" className="button" onClick={() => navigate("settings", "dependencies")}>
            Open Dependencies
          </button>
          <button type="button" className="button" onClick={reload}>
            Check again
          </button>
        </div>
      </div>
    </div>
  );
}

function OverlayControls({ status, onStatus }: { status: OverlayStatus; onStatus: (status: OverlayStatus) => void }) {
  const { settings } = status;
  const [save, setSave] = useState<SaveState>(IDLE);
  const [takeOver, setTakeOver] = useState<SaveState>(IDLE);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  /** The toggle key being typed; `null` shows the saved one. */
  const [draftKey, setDraftKey] = useState<string | null>(null);
  const key = draftKey ?? settings.toggle_key;

  // Writes run one at a time, and each is built on the newest saved settings when its turn comes.
  // Nothing is dropped: leaving the key field for another control starts a save, and the click
  // that caused it follows that save. On failure the controls keep showing the saved values, and
  // the backend's sentence stays until the next change.
  const latest = useRef(settings);
  latest.current = settings;
  const queue = useRef<Promise<void>>(Promise.resolve());
  const [pending, setPending] = useState(0);

  function enqueue(write: () => Promise<void>) {
    setNote(null);
    setPending((count) => count + 1);
    queue.current = queue.current.then(write).finally(() => setPending((count) => count - 1));
  }

  function accept(reply: OverlayStatus) {
    latest.current = reply.settings;
    onStatus(reply);
  }

  function saveSettings(change: (current: OverlaySettings) => OverlaySettings) {
    setSave(IDLE);
    enqueue(() => {
      const current = latest.current;
      const next = change(current);
      if (next === current) return Promise.resolve();
      return invoke<OverlayStatus>("set_overlay", { settings: next }).then(
        (reply) => {
          accept(reply);
          setDraftKey((draft) => (draft === reply.settings.toggle_key ? null : draft));
          setNote(savedNote(reply.config));
        },
        (e) => setSave({ status: "error", message: String(e) }),
      );
    });
  }

  /** A change to anything but the key: a key that was refused goes back to the saved one. */
  function saveChoice(change: (current: OverlaySettings) => OverlaySettings) {
    setDraftKey(null);
    saveSettings(change);
  }

  function saveKey() {
    if (key === settings.toggle_key) setDraftKey(null);
    else saveSettings((current) => ({ ...current, toggle_key: key }));
  }

  function onKeySubmit(event: FormEvent) {
    event.preventDefault();
    saveKey();
  }

  const takingOver = takeOver.status === "saving";

  function confirmTakeOver() {
    if (takingOver) return;
    setTakeOver({ status: "saving" });
    enqueue(() =>
      invoke<OverlayStatus>("take_over_overlay_config").then(
        (reply) => {
          accept(reply);
          setTakeOver(IDLE);
          setConfirming(false);
          setNote(`Redline now manages MangoHud.conf. Your file is kept as ${BACKUP_FILE}.`);
        },
        (e) => setTakeOver({ status: "error", message: String(e) }),
      ),
    );
  }

  const chosen = chosenMetrics(settings);
  const ownFigures = hasOwnFigures(settings);

  return (
    <div className="settings">
      {status.config === "user_config" && (
        <div className="setting setting-wide">
          <div className="setting-text">
            <span className="setting-title">This MangoHud.conf is your own</span>
            <span className="setting-description">
              Redline leaves it as it is. What you choose here is saved, but reaches your games only once Redline
              manages the file.
            </span>
            {takeOver.status === "error" && (
              <span className="setting-error" role="alert">
                {takeOver.message}
              </span>
            )}
          </div>
          <div className="setting-control">
            {confirming ? (
              <div className="tuning-plan" role="group" aria-label="Let Redline manage MangoHud.conf">
                <p className="setting-note">
                  Your current file will be kept as {BACKUP_FILE} in the same folder, and Redline will write a new
                  MangoHud.conf from the choices below.
                </p>
                <div className="tuning-actions">
                  <button
                    type="button"
                    className="button button-primary"
                    aria-disabled={takingOver}
                    onClick={confirmTakeOver}
                  >
                    Keep my file and continue
                  </button>
                  <button
                    type="button"
                    className="button"
                    autoFocus
                    aria-disabled={takingOver}
                    onClick={() => {
                      if (takingOver) return;
                      setConfirming(false);
                      setTakeOver(IDLE);
                    }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            ) : (
              <button type="button" className="button" onClick={() => setConfirming(true)}>
                Let Redline manage it
              </button>
            )}
          </div>
        </div>
      )}
      {status.config === "absent" && (
        <p className="settings-notice">
          There is no MangoHud.conf yet, so MangoHud uses its own defaults. Redline writes the file at your first change
          here.
        </p>
      )}

      <Setting
        title="Preset"
        description={overlayDescription(settings)}
        save={IDLE}
        note={ownFigures ? "Your own figures, under Advanced, replace the preset's." : null}
      >
        <div className="segmented" role="radiogroup" aria-label="Preset">
          {OVERLAY_PRESETS.map((option) => (
            <label className="segment" key={option.value}>
              <input
                type="radio"
                name="overlay-preset"
                value={option.value}
                checked={settings.preset === option.value}
                onChange={() => saveChoice((current) => ({ ...current, preset: option.value }))}
              />
              <span>{option.label}</span>
            </label>
          ))}
        </div>
      </Setting>

      <Setting title="Position" description="The corner of the screen the overlay sits in." save={IDLE}>
        <select
          className="setting-select"
          aria-label="Position"
          value={settings.position}
          onChange={(event) => {
            const position = event.target.value as OverlayPosition;
            saveChoice((current) => ({ ...current, position }));
          }}
        >
          {OVERLAY_POSITIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </Setting>

      <Setting
        title="Toggle key"
        description="Shows and hides the overlay in a game. Key names joined by +, such as Shift_R+F12."
        save={IDLE}
      >
        <form onSubmit={onKeySubmit}>
          <input
            type="text"
            className="setting-input overlay-key"
            aria-label="Toggle key"
            spellCheck={false}
            value={key}
            aria-invalid={save.status === "error" && draftKey !== null}
            onChange={(event) => setDraftKey(event.target.value)}
            onBlur={saveKey}
          />
        </form>
      </Setting>

      <details className="settings-advanced">
        <summary>Advanced</summary>
        <div className="settings">
          <Setting
            title="Figures"
            description="Your choice replaces the preset's figures. MangoHud adds the GPU load to a GPU figure, the CPU load to the CPU temperature and the FPS to the frametime."
            save={IDLE}
            wide
          >
            <div className="overlay-metrics">
              {OVERLAY_METRICS.map((metric) => (
                <label className="overlay-metric" key={metric.value}>
                  <span>{metric.label}</span>
                  <input
                    type="checkbox"
                    role="switch"
                    className="switch"
                    checked={chosen.includes(metric.value)}
                    aria-disabled={isLastMetric(settings, metric.value)}
                    onChange={() => saveChoice((current) => toggleMetric(current, metric.value))}
                  />
                </label>
              ))}
            </div>
            {ownFigures && (
              <button
                type="button"
                className="link-button"
                onClick={() => saveChoice((current) => ({ ...current, metrics: [] }))}
              >
                Use the preset's figures
              </button>
            )}
          </Setting>
        </div>
      </details>

      {save.status === "error" && (
        <p className="setting-error overlay-message" role="alert">
          {save.message}
        </p>
      )}
      {(pending > 0 || note !== null) && (
        <p className="setting-note overlay-message" role="status">
          {pending > 0 ? "Saving…" : note}
        </p>
      )}

      <CommandRow
        title="Turn it on in a game"
        description="In Steam, open the game's Properties and paste this into Launch options. The overlay appears only in games started this way."
        command={status.launch_hint}
      />
    </div>
  );
}
