// The Record tab: replay length, audio tracks, folders, the recording timer and the gallery
// figures. Pure functions, tested in record.test.ts.
//
// REPLAY_SECONDS_MIN/MAX MIRROR `redline-core::recorder::REPLAY_SECONDS`. The backend is the
// authority; `record.test.ts` reads the Rust file and fails when they drift apart. The default
// folders are not mirrored: the `recording_defaults` command returns them.

/** `redline-core::recorder::REPLAY_SECONDS`. */
export const REPLAY_SECONDS_MIN = 2;
export const REPLAY_SECONDS_MAX = 86_400;

/** The replay lengths offered; the saved one is added when it is not among them. */
export const REPLAY_PRESETS: readonly number[] = [15, 30, 60, 120, 180, 300, 600, 900, 1800];

/** The length choices for a saved value: the presets plus the saved one, ascending. */
export function replayLengthOptions(saved: number): number[] {
  const valid = Number.isInteger(saved) && saved >= REPLAY_SECONDS_MIN && saved <= REPLAY_SECONDS_MAX;
  const values = valid && !REPLAY_PRESETS.includes(saved) ? [...REPLAY_PRESETS, saved] : [...REPLAY_PRESETS];
  return values.sort((a, b) => a - b);
}

/** A length in words: "45 s", "2 min", "1 min 30 s", "1 h 5 min". */
export function formatLength(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const parts: string[] = [];
  if (h > 0) parts.push(`${h} h`);
  if (m > 0) parts.push(`${m} min`);
  if (s > 0 || parts.length === 0) parts.push(`${s} s`);
  return parts.join(" ");
}

// ---------------------------------------------------------------------------------------------
// Audio tracks: one entry per track of the file, the sources of a track are mixed. A track is
// never empty (the backend refuses it), so removing its last source removes the track.

/** One choice of `list_audio_sources` (`Choice` in src-tauri/src/sources.rs). */
export type AudioSource = { id: string; label: string };

export type AudioTracks = readonly (readonly string[])[];

/** How a source is named: its listed label, otherwise a label built from the saved id. */
export function sourceLabel(id: string, listed: readonly AudioSource[]): string {
  const found = listed.find((source) => source.id === id);
  if (found) return found.label;
  if (id === "default_output") return "Default output";
  if (id === "default_input") return "Default input";
  for (const [prefix, label] of [
    ["app-inverse:", (name: string) => `Everything except ${name}`],
    ["app:", (name: string) => `${name} (not running)`],
    ["device:", (name: string) => `${name} (not connected)`],
  ] as const) {
    if (id.startsWith(prefix)) return label(id.slice(prefix.length));
  }
  return id;
}

/** The listed sources a track does not have yet, in the listed order. */
export function sourcesToAdd(listed: readonly AudioSource[], track: readonly string[]): AudioSource[] {
  return listed.filter((source) => !track.includes(source.id));
}

/** Adds `id` to track `index`; a source already in the track is not added twice. */
export function addSource(tracks: AudioTracks, index: number, id: string): string[][] {
  return tracks.map((track, i) => (i === index && !track.includes(id) ? [...track, id] : [...track]));
}

/** Removes `id` from track `index`; a track left without sources is removed. */
export function removeSource(tracks: AudioTracks, index: number, id: string): string[][] {
  return tracks
    .map((track, i) => (i === index ? track.filter((source) => source !== id) : [...track]))
    .filter((track) => track.length > 0);
}

/** A new track at the end with one source. */
export function addTrack(tracks: AudioTracks, id: string): string[][] {
  return [...tracks.map((track) => [...track]), [id]];
}

export function removeTrack(tracks: AudioTracks, index: number): string[][] {
  return tracks.filter((_, i) => i !== index).map((track) => [...track]);
}

// ---------------------------------------------------------------------------------------------
// Folders

/** Why a folder typed by the user cannot be saved, or null when it can be sent. */
export function folderProblem(text: string): string | null {
  const folder = text.trim();
  if (folder === "") return "Enter a folder.";
  if (!folder.startsWith("/")) return "Use a full path, starting with /.";
  return null;
}

// ---------------------------------------------------------------------------------------------
// Recording timer

/**
 * When the running recording started, as seen by this window: set when `recording` turns true,
 * cleared when it turns false. A recording already running at the first reading has no known
 * start (`null`): the timer is not shown rather than shown wrong.
 */
export function recordingSince(
  previous: { recording: boolean; since: number | null } | null,
  recording: boolean,
  now: number,
): number | null {
  if (!recording) return null;
  if (previous === null) return null;
  return previous.recording ? previous.since : now;
}

/** Elapsed time as m:ss, or h:mm:ss past an hour. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

// ---------------------------------------------------------------------------------------------
// Gallery

/** A file size in decimal units, as file managers show it: "820 kB", "1.4 GB". */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1000) return `${bytes} B`;
  const units = ["kB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  // 999.5 and not 1000: "1000 kB" would be shown for what rounds up to the next unit.
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = value < 10 ? 1 : 0;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

export function kindLabel(kind: "replay" | "recording"): string {
  return kind === "replay" ? "Replay" : "Recording";
}
