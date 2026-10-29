import {
  addSource,
  addTrack,
  type AudioSource,
  type AudioTracks as Tracks,
  removeSource,
  removeTrack,
  sourceLabel,
  sourcesToAdd,
} from "../../record.ts";
import { type Loaded, type SaveState, Setting } from "../settings/Setting.tsx";

/** `list_audio_sources` (`Choices` in src-tauri/src/sources.rs). */
export type AudioSources = { available: boolean; unavailable_reason: string | null; items: AudioSource[] };

/**
 * The audio tracks of replays and recordings: one row per track, the sources of a track mixed
 * together. Every change is saved at once; a refused one leaves the saved tracks on screen.
 */
export function AudioTracks({
  tracks,
  sources,
  save,
  disabled,
  busy,
  onSave,
}: {
  tracks: Tracks;
  sources: Loaded<AudioSources>;
  save: SaveState;
  disabled: boolean;
  busy: boolean;
  onSave: (tracks: string[][]) => void;
}) {
  const listed = sources.status === "ready" ? sources.value.items : [];
  // Without the list the saved sources stay visible and removable, but nothing can be added.
  const canAdd = sources.status === "ready" && sources.value.available && !disabled;
  const change = (next: string[][]) => {
    if (!busy && !disabled) onSave(next);
  };

  return (
    <Setting
      title="Audio tracks"
      description="Each track is a separate audio track in the file, and the sources of a track are mixed together. Most players play only the first track."
      save={save}
      wide
    >
      {sources.status === "loading" && <span className="setting-description">Listing the audio sources…</span>}
      {/* When the rows are disabled, the page already says why (missing tool, running recording). */}
      {sources.status === "ready" && !sources.value.available && !disabled && (
        <span className="setting-description">
          {sources.value.unavailable_reason ?? "The audio sources cannot be listed."} New sources cannot be added until
          then.
        </span>
      )}
      {sources.status === "error" && (
        <span className="setting-error" role="alert">
          Cannot list the audio sources: {sources.message}
        </span>
      )}
      {tracks.length === 0 ? (
        <span className="setting-description">No audio: files have video only.</span>
      ) : (
        <ol className="tracks">
          {tracks.map((track, index) => {
            const number = index + 1;
            return (
              <li className="track" key={index}>
                <span className="track-name">Track {number}</span>
                <ul className="chips">
                  {track.map((id) => {
                    const label = sourceLabel(id, listed);
                    return (
                      <li className="chip" key={id}>
                        {label}
                        <button
                          type="button"
                          className="chip-remove"
                          aria-label={`Remove ${label} from track ${number}`}
                          disabled={disabled}
                          aria-disabled={busy}
                          onClick={() => change(removeSource(tracks, index, id))}
                        >
                          ×
                        </button>
                      </li>
                    );
                  })}
                </ul>
                <SourcePicker
                  label={`Add a source to track ${number}`}
                  placeholder="Add a source…"
                  options={sourcesToAdd(listed, track)}
                  disabled={!canAdd}
                  busy={busy}
                  onPick={(id) => change(addSource(tracks, index, id))}
                />
                <button
                  type="button"
                  className="link-button"
                  disabled={disabled}
                  aria-disabled={busy}
                  onClick={() => change(removeTrack(tracks, index))}
                >
                  Remove track
                </button>
              </li>
            );
          })}
        </ol>
      )}
      <SourcePicker
        label="Add a track"
        placeholder="Add a track with…"
        options={listed}
        disabled={!canAdd}
        busy={busy}
        onPick={(id) => change(addTrack(tracks, id))}
      />
    </Setting>
  );
}

/** A select that acts as a menu: picking a source calls `onPick` and the select goes back to its placeholder. */
function SourcePicker({
  label,
  placeholder,
  options,
  disabled,
  busy,
  onPick,
}: {
  label: string;
  placeholder: string;
  options: readonly AudioSource[];
  disabled: boolean;
  busy: boolean;
  onPick: (id: string) => void;
}) {
  return (
    <select
      className="setting-select"
      aria-label={label}
      value=""
      disabled={disabled || options.length === 0}
      aria-disabled={busy}
      onChange={(event) => {
        if (event.target.value !== "") onPick(event.target.value);
      }}
    >
      <option value="">{placeholder}</option>
      {options.map((source) => (
        <option key={source.id} value={source.id}>
          {source.label}
        </option>
      ))}
    </select>
  );
}
