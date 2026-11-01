// The update notice: whether the status bar shows it and what Settings → General says. The check
// itself, the comparison and the release page belong to the backend (D14); nothing here sees a
// URL or the network.

import type { Loaded } from "./RecorderContext.tsx";

/** `LastCheck` in src-tauri/src/updates.rs. */
export type LastCheck = "never" | "ok" | "curl_missing" | "unreachable" | "refused" | "bad_response";

/** `UpdateStatus` in src-tauri/src/updates.rs. `update_available` ignores the dismissal. */
export type UpdateStatus = {
  current_version: string;
  latest_version: string | null;
  update_available: boolean;
  dismissed: boolean;
  last_check: LastCheck;
};

/** What the General row shows. */
export type UpdateRow = {
  /** Null until the status is read. */
  installed: string | null;
  /** The newer version, dismissed or not: the row outlives the dismissal. */
  available: string | null;
  /** The status bar still shows this version, so it can be set aside. */
  canDismiss: boolean;
  /** What happened and what to do; null when there is nothing to say yet. */
  message: string | null;
};

/**
 * The version the status bar names, or null to show nothing, whatever the last check did. It does
 * not read `check_for_updates`, unlike `updateRow`: turning the check off makes the backend forget
 * the version and send `update-status`, which clears this.
 */
export function indicatorVersion(update: Loaded<UpdateStatus>): string | null {
  if (update.status !== "ready") return null;
  const { update_available, dismissed, latest_version } = update.value;
  return update_available && !dismissed ? latest_version : null;
}

export function indicatorLabel(version: string): string {
  return `Update ${version} available`;
}

/** The next try is the next round: the checks repeat every 24 hours while the app runs. */
function failedCheck(lastCheck: Exclude<LastCheck, "never" | "ok">): string {
  switch (lastCheck) {
    case "curl_missing":
      return "The check needs curl, which is not installed. Install curl and restart Redline.";
    case "unreachable":
      return "GitHub could not be reached. Check the network; Redline tries again later while it runs.";
    case "refused":
      return "GitHub refused the check, usually because of its request limit. Redline tries again once a day while it runs.";
    case "bad_response":
      return "GitHub's answer could not be read. Redline tries again once a day while it runs.";
  }
}

/**
 * The row in General. `checkEnabled` is `check_for_updates`, null while it is unknown. The status
 * has no "disabled" state: turning the check off reads as `never`, so the setting decides first,
 * and an unknown setting says nothing rather than promise a check that may never run.
 */
export function updateRow(update: Loaded<UpdateStatus>, checkEnabled: boolean | null): UpdateRow {
  if (update.status === "loading") {
    return { installed: null, available: null, canDismiss: false, message: "Reading the update status…" };
  }
  if (update.status === "error") {
    return {
      installed: null,
      available: null,
      canDismiss: false,
      message: `Cannot read the update status: ${update.message}. Restart Redline to try again.`,
    };
  }

  const status = update.value;
  const installed = status.current_version;
  if (checkEnabled === false) {
    return { installed, available: null, canDismiss: false, message: "Update checks are off." };
  }

  const available = status.update_available ? status.latest_version : null;
  const canDismiss = available !== null && !status.dismissed;
  const notice =
    available === null
      ? null
      : `Version ${available} is available.${status.dismissed ? " Dismissed: the status bar shows only a later version." : ""}`;

  let outcome: string | null;
  switch (status.last_check) {
    case "never":
      outcome = checkEnabled === null ? null : "No check has finished yet. The first one runs shortly.";
      break;
    case "ok":
      outcome = available === null ? "Redline is up to date." : null;
      break;
    default:
      outcome = failedCheck(status.last_check);
  }
  const message = [notice, outcome].filter((part) => part !== null).join(" ");
  return { installed, available, canDismiss, message: message === "" ? null : message };
}
