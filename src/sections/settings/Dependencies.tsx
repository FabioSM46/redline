import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import {
  applyInstallEvent,
  type Dependency,
  dependencyAdvice,
  DEPENDENCY_INSTALL_EVENT,
  dependencyState,
  INSTALL_IDLE,
  INSTALL_RUNNING_NOTE,
  installLine,
  installRefused,
  installSettled,
  installStarted,
  type InstallState,
  NO_GUIDED_INSTALL,
  parseDependencies,
  versionLine,
} from "../../dependencies.ts";
import { IDLE, type Loaded, Setting, Value } from "./Setting.tsx";

/**
 * Settings → Dependencies: LACT, GPU Screen Recorder, ffmpeg and MangoHud, whether they are
 * installed, their version against the verified one, what to do when one is missing, and the
 * guided install (`install_dependency`, one password prompt through polkit). One install at a time.
 * There is no Cancel: the install runs as root and Redline cannot stop it, so the page shows it as
 * busy.
 */
export function Dependencies() {
  const [deps, setDeps] = useState<Loaded<Dependency[]>>({ status: "loading" });
  const [checking, setChecking] = useState(false);
  const [install, setInstall] = useState<InstallState>(INSTALL_IDLE);

  const refresh = useCallback(() => {
    setChecking(true);
    invoke<unknown>("dependency_status")
      .then(
        (raw) => setDeps({ status: "ready", value: parseDependencies(raw) }),
        (e) => setDeps({ status: "error", message: String(e) }),
      )
      .finally(() => setChecking(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Every `dependency-install` event, also those of an install started before the page opened.
  useEffect(() => {
    let current = true;
    const unlisten = listen<unknown>(DEPENDENCY_INSTALL_EVENT, (event) => {
      if (current) setInstall((state) => applyInstallEvent(state, event.payload));
    });
    return () => {
      current = false;
      unlisten.then((stop) => stop()).catch(() => {});
    };
  }, []);

  const busy = install.status === "running";

  function onInstall(dep: Dependency) {
    if (busy) return;
    setInstall(installStarted(dep.id));
    invoke<unknown>("install_dependency", { name: dep.id }).then(
      (final) => {
        setInstall((state) => installSettled(state, dep.id, final));
        // A finished install changes what is installed, whatever its outcome.
        refresh();
      },
      (e) => setInstall(installRefused(dep.id, e)),
    );
  }

  return (
    <div className="settings">
      <p className="settings-notice">
        Redline uses these tools. A missing one only turns its feature off: tuning needs LACT, replays and recordings
        need GPU Screen Recorder, clip previews need ffmpeg.
      </p>

      {busy && (
        <p className="settings-notice" role="status">
          {INSTALL_RUNNING_NOTE}
        </p>
      )}

      <Value state={deps} what="the installed tools">
        {(list) =>
          list.length === 0 ? (
            <p className="setting-error" role="alert">
              The backend reported no tools. Check again.
            </p>
          ) : (
            <>
              {list.map((dep) => (
                <DependencyRow key={dep.id} dep={dep} install={install} busy={busy} onInstall={onInstall} />
              ))}
            </>
          )
        }
      </Value>

      <div className="command">
        <button
          type="button"
          className="button"
          onClick={() => {
            if (!checking && !busy) refresh();
          }}
          aria-disabled={checking || busy}
        >
          {checking ? "Checking…" : "Check again"}
        </button>
      </div>
    </div>
  );
}

function DependencyRow({
  dep,
  install,
  busy,
  onInstall,
}: {
  dep: Dependency;
  install: InstallState;
  busy: boolean;
  onInstall: (dep: Dependency) => void;
}) {
  const state = dependencyState(dep);
  const advice = dependencyAdvice(dep);
  const outcome = installLine(install, dep.id);
  const installing = install.status === "running" && install.id === dep.id;

  return (
    <Setting title={dep.name} description={versionLine(dep)} save={IDLE} wide>
      <div className="dependency">
        <span className={`dependency-state dependency-${state.kind}`}>{state.label}</span>
        {!dep.installed && dep.can_install && (
          <button
            type="button"
            className="button"
            onClick={() => onInstall(dep)}
            aria-disabled={busy}
            aria-busy={installing}
          >
            {installing ? "Installing…" : "Install"}
          </button>
        )}
      </div>
      {dep.warning && <span className="setting-description dependency-text">{dep.warning}</span>}
      {advice && (
        <span className="setting-note dependency-text" role={state.kind === "installed" ? undefined : "status"}>
          {advice}
        </span>
      )}
      {!dep.installed && !dep.can_install && (
        <span className="setting-description dependency-text">{NO_GUIDED_INSTALL}</span>
      )}
      {outcome && (
        <span
          className={outcome.error ? "setting-error dependency-text" : "setting-note dependency-text"}
          role={outcome.error ? "alert" : "status"}
        >
          {outcome.text}
        </span>
      )}
    </Setting>
  );
}
