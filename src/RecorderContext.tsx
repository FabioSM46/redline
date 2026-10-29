// The recorder state, its actions and the saved clips, shared by the status bar, Home and the
// Record tab. The provider subscribes once to `recorder-state` and `file-saved`; components read
// it with `useRecorder()` and `useThumbnail(id)` instead of calling the backend on their own.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { createThumbnailCache, type Recording, type RecorderStatus, replayIsOn } from "./recorder.ts";

export type Loaded<T> = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; value: T };

export type RecorderContextValue = {
  /** `recorder_status`, then every `recorder-state` event. */
  recorder: Loaded<RecorderStatus>;
  /** `list_recordings` (newest first), read again on every `file-saved`. */
  clips: Loaded<Recording[]>;
  /** A replay action is running: switches stay focusable but ignore input. */
  busy: boolean;
  /** Why the last action failed, until the next one; shown inline next to the switch. */
  actionError: string | null;
  /** Starts the replay when it is off or failed, stops it when it is on or starting. */
  toggleReplay: () => void;
  startReplay: () => void;
  stopReplay: () => void;
  saveReplay: () => void;
  /** Reads the clip list again (the Record tab after opening, for instance). */
  refreshClips: () => void;
  /** The cached preview of a clip; prefer `useThumbnail`. */
  thumbnail: (id: string) => Promise<string | null>;
  peekThumbnail: (id: string) => string | null | undefined;
};

const RecorderContext = createContext<RecorderContextValue | null>(null);

export function useRecorder(): RecorderContextValue {
  const value = useContext(RecorderContext);
  if (value === null) {
    throw new Error("useRecorder() used outside the recorder provider");
  }
  return value;
}

/**
 * The preview of a clip: undefined while it is asked, null for "no preview" (show a neutral
 * placeholder), otherwise a data URL for `<img src>`. Asked once per clip and cached by id.
 */
export function useThumbnail(id: string): string | null | undefined {
  const { thumbnail, peekThumbnail } = useRecorder();
  const [loaded, setLoaded] = useState<{ id: string; url: string | null } | null>(null);

  useEffect(() => {
    let current = true;
    thumbnail(id).then((url) => current && setLoaded({ id, url }));
    return () => {
      current = false;
    };
  }, [id, thumbnail]);

  return loaded?.id === id ? loaded.url : peekThumbnail(id);
}

export function RecorderProvider({ children }: { children: ReactNode }) {
  const [recorder, setRecorder] = useState<Loaded<RecorderStatus>>({ status: "loading" });
  const [clips, setClips] = useState<Loaded<Recording[]>>({ status: "loading" });
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  // Counts `recorder-state` events. A command reply is applied only if no event arrived while it
  // was in flight: replies and events travel separately, and the event is always the newer one.
  const events = useRef(0);
  const busyRef = useRef(false);
  const clipRequest = useRef(0);
  const mounted = useRef(true);
  const cache = useMemo(
    () => createThumbnailCache((id) => invoke<string | null>("recording_thumbnail", { id })),
    [],
  );

  const refreshClips = useCallback(() => {
    const request = ++clipRequest.current;
    invoke<Recording[]>("list_recordings").then(
      (value) => {
        if (!mounted.current || request !== clipRequest.current) return;
        cache.retain(value.map((clip) => clip.id));
        setClips({ status: "ready", value });
      },
      (e) => {
        if (mounted.current && request === clipRequest.current) setClips({ status: "error", message: String(e) });
      },
    );
  }, [cache]);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    const unlisten: (() => void)[] = [];
    const subscribe = (promise: Promise<() => void>) =>
      promise.then(
        (stop) => (active ? unlisten.push(stop) : stop()),
        () => {},
      );

    subscribe(
      listen<RecorderStatus>("recorder-state", (event) => {
        events.current++;
        setRecorder({ status: "ready", value: event.payload });
      }),
    );
    subscribe(listen("file-saved", () => refreshClips()));

    const seen = events.current;
    invoke<RecorderStatus>("recorder_status").then(
      (value) => {
        if (active && events.current === seen) setRecorder({ status: "ready", value });
      },
      (e) => {
        if (active && events.current === seen) setRecorder({ status: "error", message: String(e) });
      },
    );
    refreshClips();

    return () => {
      active = false;
      mounted.current = false;
      unlisten.forEach((stop) => stop());
    };
  }, [refreshClips]);

  const run = useCallback((command: "replay_start" | "replay_stop" | "replay_save") => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError(null);
    const seen = events.current;
    invoke<RecorderStatus | null>(command)
      .then(
        (value) => {
          if (mounted.current && value && events.current === seen) setRecorder({ status: "ready", value });
        },
        (e) => mounted.current && setActionError(String(e)),
      )
      .finally(() => {
        busyRef.current = false;
        if (mounted.current) setBusy(false);
      });
  }, []);

  const startReplay = useCallback(() => run("replay_start"), [run]);
  const stopReplay = useCallback(() => run("replay_stop"), [run]);
  const saveReplay = useCallback(() => run("replay_save"), [run]);
  const toggleReplay = useCallback(() => {
    if (recorder.status !== "ready" || !recorder.value.available) return;
    run(replayIsOn(recorder.value) ? "replay_stop" : "replay_start");
  }, [recorder, run]);
  const thumbnail = useCallback((id: string) => cache.get(id), [cache]);
  const peekThumbnail = useCallback((id: string) => cache.peek(id), [cache]);

  const value = useMemo<RecorderContextValue>(
    () => ({
      recorder,
      clips,
      busy,
      actionError,
      toggleReplay,
      startReplay,
      stopReplay,
      saveReplay,
      refreshClips,
      thumbnail,
      peekThumbnail,
    }),
    [recorder, clips, busy, actionError, toggleReplay, startReplay, stopReplay, saveReplay, refreshClips, thumbnail, peekThumbnail],
  );

  return <RecorderContext.Provider value={value}>{children}</RecorderContext.Provider>;
}
