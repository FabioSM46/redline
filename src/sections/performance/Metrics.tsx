import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useGpu } from "../../GpuContext.tsx";
import { TimeChart } from "../../charts/TimeChart.tsx";
import { useChartPalette } from "../../charts/useChartPalette.ts";
import { CHARTS, DEFAULT_WINDOW, type HistorySample, toColumns, WINDOWS, type WindowS, xRange } from "../../metrics.ts";

type HistoryState = { status: "error"; message: string } | { status: "ok"; samples: HistorySample[] };

/** A history reply together with the GPU and window it was asked for. */
type StoredHistory = { gpuId: string; windowS: WindowS; state: HistoryState };

const REFRESH_MS = 1000;

/** Charts over time of the selected GPU, from `gpu_history`, refreshed every second. */
export function Metrics() {
  const { gpu } = useGpu();
  const [windowS, setWindowS] = useState<WindowS>(DEFAULT_WINDOW);
  const [stored, setStored] = useState<StoredHistory | null>(null);
  const palette = useChartPalette();

  useEffect(() => {
    const gpuId = gpu.id;
    // A reply that arrives after the GPU or the window changed is dropped.
    let current = true;
    let inFlight = false;
    const read = () => {
      if (inFlight) return;
      inFlight = true;
      invoke<HistorySample[]>("gpu_history", { gpuId, windowS })
        .then((samples) => current && setStored({ gpuId, windowS, state: { status: "ok", samples } }))
        .catch((e) => current && setStored({ gpuId, windowS, state: { status: "error", message: String(e) } }))
        .finally(() => {
          inFlight = false;
        });
    };
    read();
    const timer = setInterval(read, REFRESH_MS);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [gpu.id, windowS]);

  // Another GPU's history is never shown under this one. After a window change the previous
  // window stays on screen until the new reply arrives, so the charts are not rebuilt.
  const history = stored?.gpuId === gpu.id ? stored : null;
  const samples = history?.state.status === "ok" ? history.state.samples : null;
  const shownWindow = history?.windowS ?? windowS;

  const charts = useMemo(() => {
    const range = samples ? xRange(samples, shownWindow) : null;
    if (!samples || !range) return null;
    return CHARTS.map((chart) => ({ chart, range, data: toColumns(samples, chart) }));
  }, [samples, shownWindow]);

  let body;
  if (history === null) {
    body = <p className="state">Reading the history…</p>;
  } else if (history.state.status === "error") {
    body = <p className="state">Cannot read the history: {history.state.message}</p>;
  } else if (charts === null) {
    body = <p className="state">Collecting the first readings…</p>;
  } else {
    body = (
      <div className="charts">
        {charts.map(({ chart, range, data }) => (
          <figure className="chart" key={chart.id}>
            <figcaption className="chart-title">
              {chart.title} ({chart.unit})
            </figcaption>
            <TimeChart chart={chart} data={data} range={range} showSeconds={shownWindow === 60} palette={palette} />
          </figure>
        ))}
      </div>
    );
  }

  return (
    <div className="metrics-view">
      <div className="window-select" role="group" aria-label="Time window">
        {WINDOWS.map((w) => (
          <button
            key={w.seconds}
            type="button"
            className="window-option"
            aria-pressed={w.seconds === windowS}
            onClick={() => setWindowS(w.seconds)}
          >
            {w.label}
          </button>
        ))}
      </div>
      {body}
    </div>
  );
}
