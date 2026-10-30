// LACT tuning profiles: the types the backend returns and the pure helpers of the profiles block
// in Performance → Tuning. Tested in profiles.test.ts, which also checks these mirrors against
// src-tauri/src/tuning/profiles.rs.

/** Mirrors `tuning::profiles::Profiles`: the reply of `list_profiles` and the `tuning-profile` payload. */
export type Profiles = {
  /** The stored profiles, sorted by name. */
  profiles: string[];
  /** The active profile; null is LACT's default profile. */
  active: string | null;
  /** LACT switches profiles by itself: a manual switch is refused. */
  auto_switch: boolean;
};

/** Mirrors `profiles::PROFILE_EVENT`. */
export const PROFILE_EVENT = "tuning-profile";

/** Mirrors `profiles::MAX_NAME_CHARS`. */
export const MAX_NAME_CHARS = 64;

/** How the page names LACT's default profile (null). */
export const DEFAULT_PROFILE_LABEL = "Default";

/**
 * The `<select>` value of a profile. Names are never empty (the backend refuses them), so the
 * empty string stands for LACT's default profile.
 */
export function profileValue(name: string | null): string {
  return name ?? "";
}

/** The profile a `<select>` value stands for: null is LACT's default profile. */
export function profileFromValue(value: string): string | null {
  return value === "" ? null : value;
}

/** A profile as the page shows it. */
export function profileLabel(name: string | null): string {
  return name ?? DEFAULT_PROFILE_LABEL;
}

/** What the confirmation panel says a switch changes. */
export function switchDetail(from: string | null, to: string | null): string {
  return `${profileLabel(from)} → ${profileLabel(to)}`;
}

/** A switch to `target` makes sense: it exists, it is not active, and LACT is not switching by itself. */
export function canSwitch(profiles: Profiles, target: string | null): boolean {
  if (profiles.auto_switch || target === profiles.active) return false;
  return target === null || profiles.profiles.includes(target);
}

/** Why `target` cannot be deleted, or null when it can. LACT's default profile is never deleted. */
export function deleteBlocker(profiles: Profiles, target: string | null): string | null {
  if (target === null) return "LACT's default profile cannot be deleted.";
  if (!profiles.profiles.includes(target)) return null;
  if (target === profiles.active) return "This profile is active: switch to another profile before deleting it.";
  return null;
}

export type NameCheck = { ok: true; name: string } | { ok: false; message: string };

/**
 * A typed name as a new profile next to `existing`, with the backend's rules and sentences
 * (`profiles::check_name`): trimmed, not empty, at most 64 characters, no control characters,
 * not "Default" in any case, unique ignoring case. The backend checks again: this only answers
 * before the request.
 */
export function checkName(input: string, existing: readonly string[]): NameCheck {
  const name = input.trim();
  if (name === "") return { ok: false, message: "A profile needs a name." };
  if ([...name].length > MAX_NAME_CHARS) {
    return { ok: false, message: `A profile name can be at most ${MAX_NAME_CHARS} characters long.` };
  }
  if (/\p{Cc}/u.test(name)) {
    return { ok: false, message: "A profile name cannot contain line breaks or other control characters." };
  }
  // ASCII-only, as Rust's `eq_ignore_ascii_case`.
  if (name.replace(/[A-Z]/g, (c) => c.toLowerCase()) === "default") {
    return { ok: false, message: '"Default" is the name of LACT\'s default profile: choose another name.' };
  }
  const lower = name.toLowerCase();
  if (existing.some((other) => other.toLowerCase() === lower)) {
    return { ok: false, message: `A profile named "${name}" already exists.` };
  }
  return { ok: true, name };
}

/**
 * Keeps the newest list: a `list_profiles` reply requested before the last `tuning-profile`
 * event is older than that event and is dropped.
 */
export type ProfilesState = {
  /** Bumped by every event, so a reply can tell whether one arrived since its request. */
  events: number;
  value: Profiles | null;
};

export type ProfilesAction =
  | { type: "event"; profiles: Profiles }
  /** A `list_profiles` reply (or the reply of create/delete) requested when `events` was `since`. */
  | { type: "reply"; profiles: Profiles; since: number };

export function profilesReducer(state: ProfilesState, action: ProfilesAction): ProfilesState {
  switch (action.type) {
    case "event":
      return { events: state.events + 1, value: action.profiles };
    case "reply":
      return action.since === state.events ? { ...state, value: action.profiles } : state;
  }
}
