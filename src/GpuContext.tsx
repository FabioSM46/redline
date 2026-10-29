// The selected GPU and its latest reading, shared by the status bar and every section.
// The shell polls `gpu_snapshot` once per second; sections read it with `useGpu()` instead of
// polling on their own.

import { invoke } from "@tauri-apps/api/core";
import { createContext, type ReactNode, useContext, useEffect, useState } from "react";
import { type GpuInfo, type GpuSnapshot, pickGpuId, type SnapshotState } from "./gpu.ts";

export type GpuContextValue = {
  /** Every AMD GPU listed by the backend, sorted by PCI slot. */
  gpus: readonly GpuInfo[];
  /** The GPU every section and the status bar show. */
  gpu: GpuInfo;
  selectGpu: (id: string) => void;
  /** Latest `gpu_snapshot` of `gpu`; back to "loading" when the selection changes. */
  reading: SnapshotState;
};

const GpuContext = createContext<GpuContextValue | null>(null);

/** Only valid below the shell, which renders sections only once a GPU is selected. */
export function useGpu(): GpuContextValue {
  const value = useContext(GpuContext);
  if (value === null) {
    throw new Error("useGpu() used outside the GPU provider");
  }
  return value;
}

/** What the shell shows before a GPU is available. */
export type GpuListState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "empty" }
  | { status: "ready"; value: GpuContextValue };

/** Lists the GPUs once, keeps the selection and polls the selected GPU every second. */
export function useGpuList(): GpuListState {
  const [gpus, setGpus] = useState<GpuInfo[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [reading, setReading] = useState<SnapshotState>({ status: "loading" });

  useEffect(() => {
    invoke<GpuInfo[]>("list_gpus")
      .then(setGpus)
      .catch((e) => setListError(String(e)));
  }, []);

  const gpuId = gpus ? pickGpuId(gpus, chosen) : null;

  useEffect(() => {
    if (!gpuId) return;
    // A reply that arrives after the selection changed belongs to the previous GPU: drop it.
    let current = true;
    setReading({ status: "loading" });
    const read = () =>
      invoke<GpuSnapshot>("gpu_snapshot", { gpuId })
        .then((snapshot) => current && setReading({ status: "ok", snapshot }))
        .catch((e) => current && setReading({ status: "error", message: String(e) }));
    read();
    const timer = setInterval(read, 1000);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [gpuId]);

  if (listError !== null) return { status: "error", message: listError };
  if (gpus === null) return { status: "loading" };
  const gpu = gpus.find((g) => g.id === gpuId);
  if (!gpu) return { status: "empty" };
  return { status: "ready", value: { gpus, gpu, selectGpu: setChosen, reading } };
}

export function GpuProvider({ value, children }: { value: GpuContextValue; children: ReactNode }) {
  return <GpuContext.Provider value={value}>{children}</GpuContext.Provider>;
}
