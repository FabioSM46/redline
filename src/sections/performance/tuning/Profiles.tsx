import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type FormEvent, useCallback, useEffect, useReducer, useRef, useState } from "react";
import {
  canSwitch,
  checkName,
  deleteBlocker,
  MAX_NAME_CHARS,
  PROFILE_EVENT,
  type Profiles,
  profileFromValue,
  profileLabel,
  profilesReducer,
  profileValue,
  switchDetail,
} from "../../../profiles.ts";
import type { PendingChange } from "../../../tuning.ts";
import type { TuningBlockProps } from "./block.ts";

/**
 * LACT profiles: the active one, a switch (a tuning change, so it goes through the confirmation
 * panel), "Save as new profile…" and "Delete…", both answered in the page. Profiles apply to every
 * GPU. The list comes from `list_profiles` and follows `tuning-profile`.
 */
export function ProfilesBlock(block: TuningBlockProps) {
  const [state, dispatch] = useReducer(profilesReducer, { events: 0, value: null });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [listenError, setListenError] = useState<string | null>(null);
  const events = useRef(0);
  events.current = state.events;

  const load = useCallback(() => {
    const since = events.current;
    setLoadError(null);
    invoke<Profiles>("list_profiles").then(
      (profiles) => dispatch({ type: "reply", profiles, since }),
      (e) => setLoadError(String(e)),
    );
  }, []);

  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    // The first read starts once the subscription is in place: an event that lands between the
    // two then counts, and the older reply is dropped. Without events the list is still read.
    listen<Profiles>(PROFILE_EVENT, (event) => dispatch({ type: "event", profiles: event.payload })).then(
      (unlisten) => {
        if (!active) return unlisten();
        stop = unlisten;
        load();
      },
      (e) => {
        if (!active) return;
        setListenError(String(e));
        load();
      },
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [load]);

  const profiles = state.value;
  return (
    <div className="tuning-block tuning-profiles">
      <h2 className="record-title">Profile</h2>
      {profiles === null && loadError === null && <p className="setting-description">Reading the profiles…</p>}
      {profiles === null && loadError !== null && (
        <div className="setting setting-wide">
          <div className="setting-text">
            <span className="setting-error" role="alert">
              Cannot read the profiles: {loadError}
            </span>
          </div>
          <div className="setting-control">
            <button type="button" className="button" onClick={load}>
              Check again
            </button>
          </div>
        </div>
      )}
      {profiles !== null && (
        <ProfilesControls
          {...block}
          profiles={profiles}
          onProfiles={(next, since) => dispatch({ type: "reply", profiles: next, since })}
          events={events}
        />
      )}
      {listenError !== null && (
        <p className="setting-error" role="alert">
          Redline cannot follow profile changes made elsewhere: {listenError}
        </p>
      )}
    </div>
  );
}

/** What the controls are doing besides showing the list. */
type Mode = { kind: "idle" } | { kind: "create" } | { kind: "delete"; name: string };

function ProfilesControls({
  gpuId,
  locked,
  apply,
  profiles,
  onProfiles,
  events,
}: TuningBlockProps & {
  profiles: Profiles;
  /** A create or delete reply, requested when the event count was `since`. */
  onProfiles: (profiles: Profiles, since: number) => void;
  events: { current: number };
}) {
  // The selection is made against one active profile: once another becomes active (a switch,
  // its switch back, LACT or another program), the selector follows it again. An open form or
  // confirmation is kept.
  const [pick, setPick] = useState<{ base: string | null; value: string | null } | null>(null);
  const selected = pick !== null && pick.base === profiles.active ? pick.value : profiles.active;
  const [requested, setMode] = useState<Mode>({ kind: "idle" });
  // A delete confirmation for a profile that became active or was deleted elsewhere closes.
  const mode: Mode =
    requested.kind === "delete" &&
    (!profiles.profiles.includes(requested.name) || deleteBlocker(profiles, requested.name) !== null)
      ? { kind: "idle" }
      : requested;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A selection that was deleted elsewhere falls back to the active profile.
  const target = selected === null || profiles.profiles.includes(selected) ? selected : profiles.active;
  const disabled = locked || busy;
  const blocker = deleteBlocker(profiles, target);

  const switchTo = () => {
    if (!canSwitch(profiles, target)) return;
    setError(null);
    setBusy(true);
    apply(
      () => invoke<PendingChange>("activate_profile", { gpuId, name: target }),
      switchDetail(profiles.active, target),
    )
      .then(setError)
      .finally(() => setBusy(false));
  };

  // Create and delete answer with the new list; the `tuning-profile` event carries it too.
  const run = (command: "create_profile" | "delete_profile", name: string) => {
    const since = events.current;
    setError(null);
    setBusy(true);
    return invoke<Profiles>(command, { name })
      .then(
        (next) => {
          onProfiles(next, since);
          setMode({ kind: "idle" });
          return null;
        },
        (e) => String(e),
      )
      .finally(() => setBusy(false));
  };

  return (
    <>
      <div className="setting setting-wide">
        <div className="setting-text">
          <span className="setting-title">
            Active: <span className="tuning-figure">{profileLabel(profiles.active)}</span>
          </span>
          <span className="setting-description">
            {profiles.auto_switch
              ? "LACT is switching profiles automatically. Turn automatic switching off in LACT to choose a profile here."
              : "A profile holds every tuning setting and applies to every GPU. Switching is confirmed like any other change."}
          </span>
          {error !== null && (
            <span className="setting-error" role="alert">
              {error}
            </span>
          )}
        </div>
        <div className="setting-control">
          <div className="tuning-actions">
            <select
              className="setting-select"
              aria-label="Profile"
              value={profileValue(target)}
              disabled={disabled || profiles.auto_switch || mode.kind !== "idle"}
              onChange={(e) => setPick({ base: profiles.active, value: profileFromValue(e.target.value) })}
            >
              <option value={profileValue(null)}>{profileLabel(null)}</option>
              {profiles.profiles.map((name) => (
                <option key={name} value={profileValue(name)}>
                  {name}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="button"
              disabled={disabled || mode.kind !== "idle" || !canSwitch(profiles, target)}
              onClick={switchTo}
            >
              Switch
            </button>
            <button
              type="button"
              className="button"
              disabled={disabled || mode.kind !== "idle"}
              onClick={() => {
                setError(null);
                setMode({ kind: "create" });
              }}
            >
              Save as new profile…
            </button>
            <button
              type="button"
              className="button"
              disabled={disabled || mode.kind !== "idle" || blocker !== null || target === null}
              title={blocker ?? undefined}
              onClick={() => {
                if (target === null) return;
                setError(null);
                setMode({ kind: "delete", name: target });
              }}
            >
              Delete…
            </button>
          </div>
        </div>
      </div>
      {mode.kind === "create" && (
        <CreateProfile
          profiles={profiles}
          disabled={disabled}
          onSave={(name) => run("create_profile", name)}
          onCancel={() => setMode({ kind: "idle" })}
        />
      )}
      {mode.kind === "delete" && (
        <DeleteProfile
          name={mode.name}
          disabled={disabled}
          onDelete={() => run("delete_profile", mode.name).then(setError)}
          onCancel={() => setMode({ kind: "idle" })}
        />
      )}
    </>
  );
}

/** "Save as new profile…": the name, checked as it is typed, then `create_profile`. */
function CreateProfile({
  profiles,
  disabled,
  onSave,
  onCancel,
}: {
  profiles: Profiles;
  disabled: boolean;
  /** Resolves to null once created, or to the sentence that says why not. */
  onSave: (name: string) => Promise<string | null>;
  onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const [touched, setTouched] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const check = checkName(text, profiles.profiles);
  const problem = check.ok ? refusal : touched ? check.message : null;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    setTouched(true);
    if (!check.ok || disabled) return;
    setRefusal(null);
    onSave(check.name).then(setRefusal);
  };

  return (
    <form className="tuning-plan" aria-label="Save as new profile" onSubmit={submit}>
      <p className="setting-note">
        Saves the current settings of {profileLabel(profiles.active)} as a new profile. Nothing changes on the GPU.
      </p>
      <div className="tuning-actions">
        <input
          type="text"
          className="setting-input profile-name"
          aria-label="Profile name"
          placeholder="Profile name"
          maxLength={MAX_NAME_CHARS * 2}
          value={text}
          autoFocus
          disabled={disabled}
          aria-invalid={problem !== null || undefined}
          onChange={(e) => {
            setText(e.target.value);
            setTouched(true);
            setRefusal(null);
          }}
        />
        <button type="submit" className="button button-primary" disabled={disabled || !check.ok}>
          Save profile
        </button>
        <button type="button" className="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
      {problem !== null && (
        <p className="setting-error" role="alert">
          {problem}
        </p>
      )}
    </form>
  );
}

/** "Delete…": the in-page confirmation, never a browser dialog. */
function DeleteProfile({
  name,
  disabled,
  onDelete,
  onCancel,
}: {
  name: string;
  disabled: boolean;
  onDelete: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="tuning-plan" role="group" aria-label={`Delete profile ${name}`}>
      <p className="setting-note">
        Delete the profile “{name}”? Its settings are removed from LACT and cannot be recovered. The GPU does not
        change.
      </p>
      <div className="tuning-actions">
        <button type="button" className="button button-primary" disabled={disabled} autoFocus onClick={onDelete}>
          Delete profile
        </button>
        <button type="button" className="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
