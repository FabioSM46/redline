import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  copyError,
  copyText,
  DESKTOP_STEPS,
  PORTAL_SHORTCUTS_EVENT,
  type PortalShortcuts,
  portalRows,
  SHORTCUT_ACTIONS,
  shortcutCommand,
} from "../../shortcuts.ts";
import { IDLE, Setting } from "./Setting.tsx";

/** What a Copy button did last: nothing yet, copied, or why it could not. */
type CopyState = { status: "idle" } | { status: "copied" } | { status: "error"; message: string };

/** What the "Change keys" button did last: nothing, waiting on the desktop, or why it failed. */
type ConfigureState = { status: "idle" } | { status: "opening" } | { status: "error"; message: string };

/** How long "Copied" stays next to a command. */
const COPIED_MS = 2000;

/**
 * Settings → Shortcuts (D5, D10). With the desktop's GlobalShortcuts portal: the keys it reports
 * and a button for its own dialog, the commands behind a disclosure. Without it (GNOME before 48),
 * while it is looked for, or when it failed: the commands to bind, as before.
 */
export function Shortcuts() {
  const portal = usePortalShortcuts();

  if (portal.state === "ready") {
    return (
      <div className="settings">
        <PortalShortcuts canConfigure={portal.can_configure} bindings={portal.bindings} />
        <details className="settings-advanced">
          <summary>Commands to bind instead</summary>
          <div className="settings">
            <Commands />
          </div>
        </details>
      </div>
    );
  }

  return (
    <div className="settings">
      {portal.state === "failed" && (
        <p className="settings-notice setting-error" role="alert">
          {portal.error} Bind the commands below instead.
        </p>
      )}
      <Commands />
      {portal.state === "unavailable" && (
        <p className="setting-description shortcut-later">
          This desktop has no global-shortcuts portal (GNOME 48 and later and KDE Plasma have it), so the keys are set
          in its custom shortcuts.
        </p>
      )}
    </div>
  );
}

/** `global_shortcuts_status`, then every `global-shortcuts` event. */
function usePortalShortcuts(): PortalShortcuts {
  const [status, setStatus] = useState<PortalShortcuts>({ state: "checking" });

  useEffect(() => {
    let current = true;
    let heard = false;
    const unlisten = listen<PortalShortcuts>(PORTAL_SHORTCUTS_EVENT, (event) => {
      heard = true;
      if (current) setStatus(event.payload);
    });
    invoke<PortalShortcuts>("global_shortcuts_status").then(
      (value) => {
        // An event is newer than the answer to a call made before it.
        if (current && !heard) setStatus(value);
      },
      (e) => {
        if (current && !heard) setStatus({ state: "failed", error: `The shortcut status is not readable: ${String(e)}.` });
      },
    );
    return () => {
      current = false;
      unlisten.then((stop) => stop()).catch(() => {});
    };
  }, []);

  return status;
}

/** The keys the desktop reports, one row per action, and the way to change them. */
function PortalShortcuts({ canConfigure, bindings }: { canConfigure: boolean; bindings: PortalShortcutsBindings }) {
  const [configure, setConfigure] = useState<ConfigureState>({ status: "idle" });

  function onConfigure() {
    if (configure.status === "opening") return;
    setConfigure({ status: "opening" });
    invoke("global_shortcuts_configure").then(
      () => setConfigure({ status: "idle" }),
      (e) => setConfigure({ status: "error", message: String(e) }),
    );
  }

  return (
    <>
      <p className="settings-notice">
        These keys work in every app, also while Redline is hidden in the tray. The desktop keeps them:{" "}
        {canConfigure ? "change them in its shortcut dialog." : "change them in its keyboard settings."}
      </p>

      {portalRows(bindings).map((row) => (
        <Setting key={row.arg} title={row.title} description={row.description} save={IDLE}>
          {row.trigger === null ? (
            <span className="setting-description">Not set</span>
          ) : (
            <kbd className="shortcut-trigger">{row.trigger}</kbd>
          )}
        </Setting>
      ))}

      {canConfigure && (
        <Setting title="Change keys" description="Opens the desktop's dialog for Redline's shortcuts." save={IDLE}>
          <div className="command">
            <button
              type="button"
              className="button"
              onClick={onConfigure}
              aria-disabled={configure.status === "opening"}
            >
              Change…
            </button>
          </div>
          {configure.status === "error" && (
            <span className="setting-error" role="alert">
              {configure.message}
            </span>
          )}
        </Setting>
      )}
    </>
  );
}

type PortalShortcutsBindings = Extract<PortalShortcuts, { state: "ready" }>["bindings"];

/** The commands to bind in the desktop's custom shortcuts (D5, D8), one row per action. */
function Commands() {
  return (
    <>
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
      </div>
    </>
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
