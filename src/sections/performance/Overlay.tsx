import { invoke } from "@tauri-apps/api/core";
import { type FormEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { type LaunchChoice, type LaunchOptionsStatus, launchSwitches, withChoice } from "../../launch.ts";
import {
  BACKUP_FILE,
  chosenMetrics,
  DEFAULT_FPS_LIMIT,
  FPS_LIMIT_REFUSED,
  hasOwnFigures,
  isLastMetric,
  OVERLAY_METRICS,
  OVERLAY_POSITIONS,
  OVERLAY_PRESETS,
  type OverlayPosition,
  type OverlaySettings,
  type OverlayStatus,
  overlayDescription,
  readFpsLimit,
  savedNote,
  toggleMetric,
  withFpsLimit,
} from "../../overlay.ts";
import type { Navigate } from "../../tabs.ts";
import { IDLE, type Loaded, type SaveState, Setting } from "../settings/Setting.tsx";
import { CommandLine } from "../settings/Shortcuts.tsx";

/**
 * The in-game overlay (D12): what MangoHud shows, where, its key and the launch option that turns
 * it on for a game. Every setting travels in one `set_overlay` call and the page shows the status
 * it answers with. A `MangoHud.conf` of the user's stays as it is until they agree to a take-over,
 * asked in the page. The launch option (D15) is a block of its own, also there without MangoHud:
 * its other choices do not need it.
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
          <OverlayControls status={status.value} onStatus={(value) => setStatus({ status: "ready", value })}>
            <LaunchOptions />
          </OverlayControls>
        ) : (
          <Missing status={status.value} navigate={navigate} reload={() => void load()}>
            <LaunchOptions />
          </Missing>
        ))}
    </div>
  );
}

/** MangoHud is not here: what to do, and where the tools are installed. `children` is the launch option. */
function Missing({
  status,
  navigate,
  reload,
  children,
}: {
  status: OverlayStatus;
  navigate: Navigate;
  reload: () => void;
  children: ReactNode;
}) {
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
      {children}
    </div>
  );
}

/** The overlay's settings, then `children`: the launch option. */
function OverlayControls({
  status,
  onStatus,
  children,
}: {
  status: OverlayStatus;
  onStatus: (status: OverlayStatus) => void;
  children: ReactNode;
}) {
  const { settings } = status;
  const [save, setSave] = useState<SaveState>(IDLE);
  const [takeOver, setTakeOver] = useState<SaveState>(IDLE);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  /** The toggle key being typed; `null` shows the saved one. */
  const [draftKey, setDraftKey] = useState<string | null>(null);
  const key = draftKey ?? settings.toggle_key;
  /** The FPS limit being typed; `null` shows the saved one. */
  const [draftLimit, setDraftLimit] = useState<string | null>(null);
  const limit = draftLimit ?? (settings.fps_limit === null ? "" : String(settings.fps_limit));

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

  function saveSettings(change: (current: OverlaySettings) => OverlaySettings, refused?: () => void) {
    setSave(IDLE);
    enqueue(() => {
      const current = latest.current;
      const next = change(current);
      if (next === current) return Promise.resolve();
      return invoke<OverlayStatus>("set_overlay", { settings: next }).then(
        (reply) => {
          accept(reply);
          setDraftKey((draft) => (draft === reply.settings.toggle_key ? null : draft));
          // With the limit off there is no field, so no text of it is left to show later.
          const saved = reply.settings.fps_limit;
          setDraftLimit((draft) => (saved === null || (draft !== null && readFpsLimit(draft) === saved) ? null : draft));
          setNote(savedNote(reply.config));
        },
        (e) => {
          refused?.();
          setSave({ status: "error", message: String(e) });
        },
      );
    });
  }

  /** A change to anything but the key: a key that was refused goes back to the saved one. */
  function saveChoice(change: (current: OverlaySettings) => OverlaySettings, refused?: () => void) {
    setDraftKey(null);
    saveSettings(change, refused);
  }

  function saveKey() {
    if (key === settings.toggle_key) setDraftKey(null);
    else saveSettings((current) => ({ ...current, toggle_key: key }));
  }

  function onKeySubmit(event: FormEvent) {
    event.preventDefault();
    saveKey();
  }

  /**
   * The typed limit, saved when the field is left or Enter is pressed. The field keeps the text
   * while it is saved, then shows the saved limit again: the new one, or the old one when the
   * text cannot be sent or the backend refuses it.
   */
  function saveLimit() {
    if (draftLimit === null) return;
    const text = draftLimit;
    const typed = readFpsLimit(text);
    const settle = () => setDraftLimit((draft) => (draft === text ? null : draft));
    if (typed === null || typed === settings.fps_limit) {
      setDraftLimit(null);
    } else if (typed === "refused") {
      setDraftLimit(null);
      setNote(null);
      setSave({ status: "error", message: FPS_LIMIT_REFUSED });
    } else {
      // A limit switched off while this waited in the queue stays off.
      saveChoice((current) => {
        const next = current.fps_limit === null ? current : withFpsLimit(current, typed);
        if (next === current) settle();
        return next;
      }, settle);
    }
  }

  function onLimitSubmit(event: FormEvent) {
    event.preventDefault();
    saveLimit();
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

      <Setting
        title="FPS limit"
        description="Caps the frame rate of games started with MangoHud, also while the overlay is hidden."
        save={IDLE}
      >
        <form className="overlay-limit" noValidate onSubmit={onLimitSubmit}>
          {settings.fps_limit !== null && (
            <label className="overlay-limit-value">
              <input
                type="text"
                inputMode="numeric"
                className="setting-input overlay-limit-number"
                aria-label="FPS limit in frames per second"
                value={limit}
                onChange={(event) => setDraftLimit(event.target.value)}
                onBlur={saveLimit}
              />
              <span>FPS</span>
            </label>
          )}
          <input
            type="checkbox"
            role="switch"
            className="switch"
            aria-label="FPS limit"
            checked={settings.fps_limit !== null}
            onChange={(event) => {
              const on = event.target.checked;
              saveChoice((current) =>
                (current.fps_limit !== null) === on ? current : withFpsLimit(current, on ? DEFAULT_FPS_LIMIT : null),
              );
            }}
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

      {children}
    </div>
  );
}

/**
 * "Turn it on in a game" (D15): the four switches and the line they compose. The backend composes
 * the line and says which tools are there; a change sends all four choices to
 * `set_launch_options` and the block shows the status it answers with. On failure the switches
 * keep the saved values and the backend's sentence stays until the next change.
 */
function LaunchOptions() {
  const [status, setStatus] = useState<Loaded<LaunchOptionsStatus>>({ status: "loading" });
  const [save, setSave] = useState<SaveState>(IDLE);

  const load = useCallback(() => {
    setStatus({ status: "loading" });
    setSave(IDLE);
    return invoke<LaunchOptionsStatus>("launch_options_status").then(
      (value) => setStatus({ status: "ready", value }),
      (e) => setStatus({ status: "error", message: String(e) }),
    );
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const saving = save.status === "saving";

  function choose(current: LaunchOptionsStatus, choice: LaunchChoice, on: boolean) {
    if (saving) return;
    setSave({ status: "saving" });
    invoke<LaunchOptionsStatus>("set_launch_options", { choices: withChoice(current.choices, choice, on) }).then(
      (value) => {
        setStatus({ status: "ready", value });
        setSave(IDLE);
      },
      (e) => setSave({ status: "error", message: String(e) }),
    );
  }

  return (
    <Setting
      title="Turn it on in a game"
      description="In Steam, open the game's Properties and paste this into Launch options. Lutris has no launch option line: open In Lutris below for where each part goes. The overlay appears only in games started with MangoHud from one of these launchers."
      save={IDLE}
      wide
    >
      {status.status === "loading" && <span className="setting-description">Reading the launch option…</span>}
      {status.status === "error" && (
        <>
          <span className="setting-error" role="alert">
            Cannot read the launch option: {status.message}
          </span>
          <button type="button" className="button" onClick={() => void load()}>
            Check again
          </button>
        </>
      )}
      {status.status === "ready" && (
        <>
          <div className="launch-options">
            {launchSwitches(status.value).map((option) => (
              <label className="launch-option" key={option.choice}>
                <span className="setting-text">
                  <span className="setting-title">{option.label}</span>
                  <span className="setting-description">{option.description}</span>
                  {option.note && <span className="setting-note">{option.note}</span>}
                </span>
                <input
                  type="checkbox"
                  role="switch"
                  className="switch"
                  aria-label={option.label}
                  checked={option.checked}
                  disabled={option.disabled}
                  aria-disabled={saving}
                  onChange={(event) => choose(status.value, option.choice, event.target.checked)}
                />
              </label>
            ))}
          </div>
          {save.status === "error" && (
            <span className="setting-error" role="alert">
              Not saved: {save.message}
            </span>
          )}
          <CommandLine command={status.value.command} />
        </>
      )}
      <LutrisSteps />
    </Setting>
  );
}

/**
 * Where each part of the composed line goes in Lutris, behind a disclosure: Lutris has no launch
 * option line. Option names and places checked against Lutris 0.5.22 (`lutris/sysoptions.py`):
 * Environment variables and FPS counter (MangoHud) are shown by default, Command prefix only with
 * Advanced on.
 */
function LutrisSteps() {
  return (
    <details className="settings-advanced">
      <summary>In Lutris</summary>
      <ul className="launch-lutris">
        <li>Open the game's Configure → System options.</li>
        <li>
          Add each NAME=1 pair of the line under Environment variables, with NAME as the key and 1 as the value.
        </li>
        <li>
          For the overlay, turn on FPS counter (MangoHud), or turn on Advanced and enter mangohud as the Command prefix.
          Under Wine the overlay is reliable only on Vulkan, which normally means DXVK.
        </li>
        <li>Leave out %command%: Lutris adds the game's command itself.</li>
      </ul>
    </details>
  );
}
