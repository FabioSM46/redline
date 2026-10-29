import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type Dispatch, useCallback, useEffect, useReducer, useRef, useState } from "react";
import {
  NO_PENDING,
  PENDING_EVENT,
  type PendingAction,
  type PendingChange,
  pendingReducer,
  type PendingState,
  SETTLED_EVENT,
  type Settled,
  type TuningStatus,
} from "../../../tuning.ts";
import type { Loaded } from "../../settings/Setting.tsx";

/** A `tuning_status` reply, tied to the GPU it was read for. */
type Stored = { gpuId: string; state: Loaded<TuningStatus> };

/**
 * `tuning_status` of `gpuId`, read when the GPU changes and on `reload()`. A reload keeps the
 * previous reply on screen until the new one arrives; a reply for another GPU, or older than the
 * latest request, is dropped.
 */
export function useTuningStatus(gpuId: string): { state: Loaded<TuningStatus>; reload: () => void } {
  const [stored, setStored] = useState<Stored | null>(null);
  const [generation, setGeneration] = useState(0);
  const reload = useCallback(() => setGeneration((g) => g + 1), []);

  useEffect(() => {
    let current = true;
    invoke<TuningStatus>("tuning_status", { gpuId }).then(
      (value) => current && setStored({ gpuId, state: { status: "ready", value } }),
      (e) => current && setStored({ gpuId, state: { status: "error", message: String(e) } }),
    );
    return () => {
      current = false;
    };
  }, [gpuId, generation]);

  const state: Loaded<TuningStatus> = stored?.gpuId === gpuId ? stored.state : { status: "loading" };
  return { state, reload };
}

/**
 * The one change lactd holds for the whole daemon, from `tuning-pending` / `tuning-settled` and
 * from the replies of the setters. `onSettled` runs once per settled change.
 */
export function usePendingChange(onSettled: (settled: Settled) => void): [PendingState, Dispatch<PendingAction>] {
  const [state, dispatch] = useReducer(pendingReducer, NO_PENDING);
  const settledRef = useRef(onSettled);
  settledRef.current = onSettled;

  useEffect(() => {
    let active = true;
    const unlisten: (() => void)[] = [];
    const subscribe = (promise: Promise<() => void>) =>
      promise.then(
        (stop) => (active ? unlisten.push(stop) : stop()),
        () => {},
      );
    subscribe(listen<PendingChange>(PENDING_EVENT, (event) => dispatch({ type: "pending", change: event.payload })));
    subscribe(listen<Settled>(SETTLED_EVENT, (event) => dispatch({ type: "settled", result: event.payload })));
    return () => {
      active = false;
      unlisten.forEach((stop) => stop());
    };
  }, []);

  // Once per settled change, whichever of the event or the reply settled it.
  const settled = state.settled;
  useEffect(() => {
    if (settled !== null) settledRef.current(settled.result);
  }, [settled]);

  return [state, dispatch];
}

/** The current time, every `intervalMs` while `active`. */
export function useNow(active: boolean, intervalMs = 250): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [active, intervalMs]);
  return now;
}
