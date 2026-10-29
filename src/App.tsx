import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import "./App.css";
import { formatValue, percentOf } from "./format.ts";

type GpuSnapshot = {
  temp_edge_c: number | null;
  temp_junction_c: number | null;
  temp_mem_c: number | null;
  power_w: number | null;
  power_cap_w: number | null;
  fan_rpm: number | null;
  sclk_mhz: number | null;
  mclk_mhz: number | null;
  vddgfx_mv: number | null;
  gpu_busy_pct: number | null;
  mem_busy_pct: number | null;
  vram_used_mib: number | null;
  vram_total_mib: number | null;
};

function App() {
  const [snapshot, setSnapshot] = useState<GpuSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const read = () =>
      invoke<GpuSnapshot>("gpu_snapshot")
        .then((s) => {
          setSnapshot(s);
          setError(null);
        })
        .catch((e) => setError(String(e)));
    read();
    const timer = setInterval(read, 1000);
    return () => clearInterval(timer);
  }, []);

  if (error) {
    return <main className="state">Impossibile leggere la GPU: {error}</main>;
  }
  if (!snapshot) {
    return <main className="state">Lettura dei sensori…</main>;
  }

  const s = snapshot;
  const vram = percentOf(s.vram_used_mib, s.vram_total_mib);
  const metrics: [string, string][] = [
    ["Temperatura", formatValue(s.temp_edge_c, "°C")],
    ["Hotspot", formatValue(s.temp_junction_c, "°C")],
    ["Memoria", formatValue(s.temp_mem_c, "°C")],
    ["Potenza", `${formatValue(s.power_w, "W")} / ${formatValue(s.power_cap_w, "W")}`],
    ["Ventola", formatValue(s.fan_rpm, "rpm")],
    ["Clock GPU", formatValue(s.sclk_mhz, "MHz")],
    ["Clock VRAM", formatValue(s.mclk_mhz, "MHz")],
    ["Tensione", formatValue(s.vddgfx_mv, "mV")],
    ["Utilizzo GPU", formatValue(s.gpu_busy_pct, "%")],
    ["VRAM", `${formatValue(s.vram_used_mib, "MiB")} (${formatValue(vram, "%")})`],
  ];

  return (
    <main className="dashboard">
      <h1>Redline</h1>
      <dl className="metrics">
        {metrics.map(([label, value]) => (
          <div className="metric" key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </main>
  );
}

export default App;
