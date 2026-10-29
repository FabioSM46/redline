import { useState } from "react";
import { folderProblem } from "../../record.ts";
import { type SaveState, Setting } from "../settings/Setting.tsx";

/**
 * A folder shown and edited as text (there is no native folder picker), with Save, Cancel and
 * "Reset to default". A refused save keeps the typed text and says the folder did not change.
 */
export function Folder({
  title,
  description,
  saved,
  defaultValue,
  save,
  disabled,
  busy,
  onSave,
}: {
  title: string;
  description: string;
  saved: string;
  /** Null when the default is not known: no reset is offered. */
  defaultValue: string | null;
  save: SaveState;
  disabled: boolean;
  busy: boolean;
  onSave: (folder: string) => Promise<boolean>;
}) {
  // Null while not edited: the field shows the saved folder.
  const [draft, setDraft] = useState<string | null>(null);
  const editing = draft !== null && draft.trim() !== saved;
  const problem = editing ? folderProblem(draft) : null;
  const inactive = disabled || busy;

  function submit(folder: string) {
    if (inactive) return;
    // Typing stays possible while the save runs: the draft is cleared only if it was not
    // changed meanwhile, so a correction typed during the save is kept.
    const sent = draft;
    onSave(folder).then((ok) => {
      if (ok) setDraft((current) => (current === sent ? null : current));
    });
  }

  return (
    <Setting title={title} description={description} save={save} wide>
      <form
        className="folder"
        onSubmit={(event) => {
          event.preventDefault();
          if (editing && problem === null) submit(draft.trim());
        }}
      >
        <input
          className="setting-input"
          type="text"
          aria-label={title}
          spellCheck={false}
          value={draft ?? saved}
          disabled={disabled}
          aria-invalid={problem !== null}
          onChange={(event) => setDraft(event.target.value)}
        />
        <span className="folder-actions">
          {editing && (
            <>
              <button type="submit" className="button" disabled={disabled || problem !== null} aria-disabled={busy}>
                Save
              </button>
              <button type="button" className="button" onClick={() => setDraft(null)}>
                Cancel
              </button>
            </>
          )}
          {!editing && defaultValue !== null && saved !== defaultValue && (
            <button
              type="button"
              className="link-button"
              disabled={disabled}
              aria-disabled={busy}
              onClick={() => submit(defaultValue)}
            >
              Reset to default
            </button>
          )}
        </span>
      </form>
      {problem !== null && <span className="setting-error">{problem}</span>}
      {save.status === "error" && <span className="setting-description">The folder is still {saved}.</span>}
    </Setting>
  );
}
