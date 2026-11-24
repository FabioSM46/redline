import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { REDLINE_LICENSE, THIRD_PARTY_LICENSES } from "../../dependencies.ts";
import { copyError, copyText } from "../../shortcuts.ts";
import { IDLE, type Loaded, Setting, Value } from "./Setting.tsx";
import { copyWithSelection } from "./Shortcuts.tsx";

/** What a Copy button did last: nothing yet, copied, or why it could not. */
type CopyState = { status: "idle" } | { status: "copied" } | { status: "error"; message: string };

/** How long "Copied" stays next to an address. */
const COPIED_MS = 2000;

/** `SupportPage` in src-tauri/src/support.rs: the backend owns the addresses. */
type SupportPage = "github_sponsors" | "ko_fi";

/**
 * Settings → About: Redline's version and license, a way to support it, and the licenses of the
 * tools it works with. The license addresses are shown with a Copy button and the webview never
 * navigates away from Redline; the support buttons name a page and the backend opens it.
 */
export function About() {
  const [version, setVersion] = useState<Loaded<string>>({ status: "loading" });

  useEffect(() => {
    getVersion().then(
      (value) => setVersion({ status: "ready", value }),
      (e) => setVersion({ status: "error", message: String(e) }),
    );
  }, []);

  return (
    <div className="settings">
      <Setting title="Redline" description="Control panel for GPUs on Linux." save={IDLE}>
        <Value state={version} what="the version">
          {(value) => <span className="about-value">Version {value}</span>}
        </Value>
      </Setting>
      <Setting title="License" description="Redline is free software." save={IDLE}>
        <span className="about-value">{REDLINE_LICENSE}</span>
      </Setting>
      <SupportRow />

      <p className="settings-notice">
        Redline works with these separate projects. They have their own licenses and are installed on their own.
      </p>
      {THIRD_PARTY_LICENSES.map((tool) => (
        <LicenseRow key={tool.name} title={`${tool.name} — ${tool.license}`} description={tool.use} url={tool.url} />
      ))}
    </div>
  );
}

/** Opens GitHub Sponsors or Ko-fi in the browser; nothing is asked for anywhere else in the app. */
function SupportRow() {
  const [error, setError] = useState<string | null>(null);

  function open(page: SupportPage) {
    setError(null);
    invoke<void>("open_support_page", { page }).catch((e) => setError(`Cannot open the page: ${String(e)}`));
  }

  return (
    <Setting
      title="Support Redline"
      description="Redline is free. If it is useful to you, you can support its development."
      save={IDLE}
    >
      <div className="command">
        <button type="button" className="button" onClick={() => open("github_sponsors")}>
          GitHub Sponsors
        </button>
        <button type="button" className="button" onClick={() => open("ko_fi")}>
          Ko-fi
        </button>
      </div>
      {error !== null && (
        <span className="setting-error" role="alert">
          {error}
        </span>
      )}
    </Setting>
  );
}

function LicenseRow({ title, description, url }: { title: string; description: string; url: string }) {
  const [copy, setCopy] = useState<CopyState>({ status: "idle" });

  useEffect(() => {
    if (copy.status !== "copied") return;
    const timer = window.setTimeout(() => setCopy({ status: "idle" }), COPIED_MS);
    return () => window.clearTimeout(timer);
  }, [copy]);

  function onCopy() {
    copyText(navigator.clipboard, url, copyWithSelection).then(
      () => setCopy({ status: "copied" }),
      (e) => setCopy({ status: "error", message: copyError(e).replace("the command", "the address") }),
    );
  }

  return (
    <Setting title={title} description={description} save={IDLE} wide>
      <div className="command">
        <code className="command-text">{url}</code>
        <button type="button" className="button" onClick={onCopy} aria-label={`Copy ${url}`}>
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
