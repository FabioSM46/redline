// Settings → Dependencies and About: the tools Redline uses, their state, the guided install, and
// the licenses. The types mirror src-tauri/src/dependencies.rs and src-tauri/src/installer.rs;
// `dependencies.test.ts` reads both files and fails if they drift.

/** `DependencyId` in dependencies.rs (serde snake_case), in the order `dependency_status` lists them. */
export const DEPENDENCY_IDS = ["lact", "gpu_screen_recorder", "ffmpeg"] as const;

export type DependencyId = (typeof DEPENDENCY_IDS)[number];

/** `DaemonStatus` in dependencies.rs: lactd, as Redline sees it. */
export type DaemonStatus = { running: boolean; socket_accessible: boolean };

/** `Dependency` in dependencies.rs, as `dependency_status` returns it. */
export type Dependency = {
  id: DependencyId;
  name: string;
  installed: boolean;
  version: string | null;
  verified_version: string | null;
  warning: string | null;
  hint: string | null;
  daemon: DaemonStatus | null;
  can_install: boolean;
};

/** `INSTALL_EVENT` in installer.rs. */
export const DEPENDENCY_INSTALL_EVENT = "dependency-install";

/** `InstallStage` in installer.rs. */
export type InstallStage = "progress" | "succeeded" | "failed" | "cancelled";

/** `InstallEvent` in installer.rs: the event payload, and what `install_dependency` resolves with. */
export type InstallEvent = { id: DependencyId; stage: InstallStage; message: string | null };

function isDependencyId(value: unknown): value is DependencyId {
  return typeof value === "string" && (DEPENDENCY_IDS as readonly string[]).includes(value);
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/**
 * The `dependency_status` answer, checked: entries with an unknown id are dropped, a missing field
 * reads as absent (null, false), never as installed. Not a list: an empty one.
 */
export function parseDependencies(raw: unknown): Dependency[] {
  if (!Array.isArray(raw)) return [];
  const result: Dependency[] = [];
  for (const entry of raw) {
    if (typeof entry !== "object" || entry === null) continue;
    const e = entry as Record<string, unknown>;
    if (!isDependencyId(e.id)) continue;
    const daemon =
      typeof e.daemon === "object" && e.daemon !== null
        ? {
            running: (e.daemon as Record<string, unknown>).running === true,
            socket_accessible: (e.daemon as Record<string, unknown>).socket_accessible === true,
          }
        : null;
    result.push({
      id: e.id,
      name: text(e.name) ?? e.id,
      installed: e.installed === true,
      version: text(e.version),
      verified_version: text(e.verified_version),
      warning: text(e.warning),
      hint: text(e.hint),
      daemon,
      can_install: e.can_install === true,
    });
  }
  return result;
}

/** What a row's state label says: ready, missing, or installed but not usable as is. */
export type DependencyState = { kind: "installed" | "missing" | "attention"; label: string };

export function dependencyState(dep: Dependency): DependencyState {
  if (!dep.installed) return { kind: "missing", label: "Not installed" };
  if (dep.daemon && !(dep.daemon.running && dep.daemon.socket_accessible)) {
    return { kind: "attention", label: "Not reachable" };
  }
  if (dep.warning) return { kind: "attention", label: "Installed" };
  return { kind: "installed", label: "Installed" };
}

/** The version line: the installed version against the one Redline is verified with. */
export function versionLine(dep: Dependency): string {
  const installed = dep.installed ? (dep.version ?? "unknown") : "none";
  const verified = dep.verified_version ? `, verified with ${dep.verified_version}` : "";
  return `Version ${installed}${verified}`;
}

/** Said when lactd runs but refuses this user and the backend gave no sentence of its own. */
export const LACT_SOCKET_FALLBACK =
  "lactd is running but Redline cannot use it: your user is not in its admin group. Set admin_group in " +
  "/etc/lact/config.yaml to a group you belong to, restart lactd, then log out and back in.";

/**
 * What to do about a tool, if anything: the backend's hint, or for lactd running behind a socket
 * this user may not open, a sentence about the admin group.
 */
export function dependencyAdvice(dep: Dependency): string | null {
  if (dep.hint) return dep.hint;
  if (dep.daemon?.running && !dep.daemon.socket_accessible) return LACT_SOCKET_FALLBACK;
  return null;
}

/** Shown instead of the Install button when the guided install cannot run here. */
export const NO_GUIDED_INSTALL =
  "The guided install is not available here: it comes with the Redline package and needs polkit " +
  "(pkexec). Install the tool with your package manager instead.";

/** Said while an install runs: why there is no Cancel button. */
export const INSTALL_RUNNING_NOTE =
  "Installing as administrator can take several minutes. It cannot be stopped from Redline; " +
  "every step stops by itself after 30 minutes.";

/** The install shown on the page: none, one running, or the outcome of the last one. */
export type InstallState =
  | { status: "idle" }
  | { status: "running"; id: DependencyId; step: string | null }
  | { status: "succeeded"; id: DependencyId }
  | { status: "cancelled"; id: DependencyId }
  | { status: "failed"; id: DependencyId; message: string };

export const INSTALL_IDLE: InstallState = { status: "idle" };

/** Starting an install from the page, before any event arrives. */
export function installStarted(id: DependencyId): InstallState {
  return { status: "running", id, step: null };
}

/**
 * The page's install after an event (or the command's final result, which repeats the last event:
 * applying it twice changes nothing). A payload that is not an `InstallEvent` is ignored.
 */
export function applyInstallEvent(state: InstallState, raw: unknown): InstallState {
  if (typeof raw !== "object" || raw === null) return state;
  const event = raw as Record<string, unknown>;
  if (!isDependencyId(event.id)) return state;
  const id = event.id;
  const message = text(event.message);
  switch (event.stage) {
    case "progress":
      return { status: "running", id, step: message ?? (state.status === "running" ? state.step : null) };
    case "succeeded":
      return { status: "succeeded", id };
    case "cancelled":
      return { status: "cancelled", id };
    case "failed":
      return { status: "failed", id, message: message ?? "the install helper gave no reason." };
    default:
      return state;
  }
}

/** The command was refused before anything ran (no helper, no pkexec, another install). */
export function installRefused(id: DependencyId, reason: unknown): InstallState {
  const message = (reason instanceof Error ? reason.message : String(reason)).trim();
  return { status: "failed", id, message: message || "the install could not start." };
}

/** One line for a row's install outcome, and whether it is an error. */
export function installLine(state: InstallState, id: DependencyId): { text: string; error: boolean } | null {
  if (state.status === "idle" || state.id !== id) return null;
  switch (state.status) {
    case "running":
      return { text: state.step ? `Installing: ${state.step}` : "Installing… waiting for the password", error: false };
    case "succeeded":
      return { text: "Installed.", error: false };
    case "cancelled":
      return { text: "Install cancelled: the password prompt was closed. Nothing was changed.", error: false };
    case "failed":
      return { text: `Install failed: ${state.message}`, error: true };
  }
}

/** Redline's own license (package.json, LICENSE). */
export const REDLINE_LICENSE = "MIT";

/** The tools Redline starts, and their licenses. None of their code is part of Redline. */
export const THIRD_PARTY_LICENSES = [
  {
    name: "LACT",
    license: "MIT",
    use: "Talks to its daemon, lactd, for every tuning change.",
    url: "https://github.com/ilya-zlobintsev/LACT/blob/master/LICENSE",
  },
  {
    name: "GPU Screen Recorder",
    license: "GPL-3.0",
    use: "Started as an external program for replays and recordings; none of its code is in Redline.",
    url: "https://git.dec05eba.com/gpu-screen-recorder/tree/LICENSE",
  },
] as const;
