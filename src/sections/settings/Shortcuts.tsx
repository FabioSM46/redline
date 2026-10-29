import { useEffect, useState } from "react";
import { copyError, copyText, DESKTOP_STEPS, SHORTCUT_ACTIONS, shortcutCommand } from "../../shortcuts.ts";
import { IDLE, Setting } from "./Setting.tsx";

/** What a Copy button did last: nothing yet, copied, or why it could not. */
type CopyState = { status: "idle" } | { status: "copied" } | { status: "error"; message: string };

/** How long "Copied" stays next to a command. */
const COPIED_MS = 2000;

/** The commands to bind in the desktop's custom shortcuts (D5, D8), one row per action. */
export function Shortcuts() {
  return (
    <div className="settings">
      <p className="settings-notice">
        Bind these commands to keys in your desktop's custom shortcuts. They act on Redline while it runs, without
        showing the window; when it is not running, they start it hidden in the tray first.
      </p>

      {SHORTCUT_ACTIONS.map((action) => (
        <CommandRow
          key={action.arg}
          title={action.title}
          description={action.description}
          command={shortcutCommand(action.arg)}
        />
      ))}

      <div className="settings-notice">
        <ul className="shortcut-steps">
          {DESKTOP_STEPS.map((d) => (
            <li key={d.desktop}>
              <strong>{d.desktop}:</strong> {d.steps}. Paste the command and pick the keys.
            </li>
          ))}
        </ul>
        <p className="setting-description shortcut-later">
          Registering the keys from Redline itself, through the desktop's global-shortcuts portal, will come later.
        </p>
      </div>
    </div>
  );
}

function CommandRow({ title, description, command }: { title: string; description: string; command: string }) {
  const [copy, setCopy] = useState<CopyState>({ status: "idle" });

  useEffect(() => {
    if (copy.status !== "copied") return;
    const timer = window.setTimeout(() => setCopy({ status: "idle" }), COPIED_MS);
    return () => window.clearTimeout(timer);
  }, [copy]);

  function onCopy() {
    copyText(navigator.clipboard, command, copyWithSelection).then(
      () => setCopy({ status: "copied" }),
      (e) => setCopy({ status: "error", message: copyError(e) }),
    );
  }

  return (
    <Setting title={title} description={description} save={IDLE} wide>
      <div className="command">
        <code className="command-text">{command}</code>
        <button type="button" className="button" onClick={onCopy} aria-label={`Copy ${command}`}>
          Copy
        </button>
        {copy.status === "copied" && (
          <span className="setting-note" role="status">
            Copied
          </span>
        )}
      </div>
      {copy.status === "error" && (
        <span className="setting-error" role="alert">
          {copy.message}
        </span>
      )}
    </Setting>
  );
}

/**
 * The copy fallback for webviews without the async Clipboard API: select the text in a hidden,
 * read-only textarea and run the copy command. `true` only when the webview reports success.
 */
function copyWithSelection(text: string): boolean {
  const area = document.createElement("textarea");
  area.value = text;
  area.readOnly = true;
  area.className = "visually-hidden";
  const focused = document.activeElement;
  document.body.append(area);
  try {
    area.select();
    return document.execCommand("copy");
  } finally {
    area.remove();
    if (focused instanceof HTMLElement) focused.focus();
  }
}
