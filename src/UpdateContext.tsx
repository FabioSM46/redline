// The update status, shared by the status bar and Settings → General. The provider subscribes once
// to `update-status`; components read it with `useUpdates()` instead of calling the backend.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { Loaded } from "./RecorderContext.tsx";
import type { UpdateStatus } from "./updates.ts";

export type UpdateContextValue = {
  /** `update_status`, then every `update-status` event. */
  update: Loaded<UpdateStatus>;
  /** An action is running: buttons stay focusable but ignore input. */
  busy: boolean;
  /** Why the last action failed, for ACTION_ERROR_MS or until the next action; never a failed check. */
  actionError: string | null;
  openRelease: () => void;
  dismiss: () => void;
};

/** How long a failed action stays on screen: the status bar has no way to close it. */
const ACTION_ERROR_MS = 15000;

const UpdateContext = createContext<UpdateContextValue | null>(null);

export function useUpdates(): UpdateContextValue {
  const value = useContext(UpdateContext);
  if (value === null) {
    throw new Error("useUpdates() used outside the update provider");
  }
  return value;
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const [update, setUpdate] = useState<Loaded<UpdateStatus>>({ status: "loading" });
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  // Counts `update-status` events. A command reply is applied only if no event arrived while it
  // was in flight: replies and events travel separately, and the event is always the newer one.
  const events = useRef(0);
  const busyRef = useRef(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    let unlisten: (() => void) | undefined;

    // The first read follows the subscription, so a change between the two cannot be lost: the
    // event is sent only on a change, and no other event would correct a stale reading.
    const read = () => {
      if (!active) return;
      const seen = events.current;
      invoke<UpdateStatus>("update_status").then(
        (value) => {
          if (active && events.current === seen) setUpdate({ status: "ready", value });
        },
        (e) => {
          if (active && events.current === seen) setUpdate({ status: "error", message: String(e) });
        },
      );
    };

    listen<UpdateStatus>("update-status", (event) => {
      events.current++;
      setUpdate({ status: "ready", value: event.payload });
    }).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
        read();
      },
      read,
    );

    return () => {
      active = false;
      mounted.current = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (actionError === null) return;
    const timer = window.setTimeout(() => setActionError(null), ACTION_ERROR_MS);
    return () => window.clearTimeout(timer);
  }, [actionError]);

  const run = useCallback((action: () => Promise<UpdateStatus | void>, failure: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError(null);
    const seen = events.current;
    action()
      .then(
        (value) => {
          if (mounted.current && value && events.current === seen) setUpdate({ status: "ready", value });
        },
        (e) => mounted.current && setActionError(`${failure}: ${String(e)}`),
      )
      .finally(() => {
        busyRef.current = false;
        if (mounted.current) setBusy(false);
      });
  }, []);

  const openRelease = useCallback(
    () => run(() => invoke<void>("open_latest_release"), "Cannot open the release page"),
    [run],
  );
  const dismiss = useCallback(
    () => run(() => invoke<UpdateStatus>("dismiss_update"), "Cannot dismiss the update"),
    [run],
  );

  const value = useMemo<UpdateContextValue>(
    () => ({ update, busy, actionError, openRelease, dismiss }),
    [update, busy, actionError, openRelease, dismiss],
  );

  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}
