import { useGpu } from "../GpuContext.tsx";
import { formatValue, percentOf } from "../format.ts";

/** The GPU state at a glance. The Home issue reshapes these figures (D8). */
export function Home() {
  const { reading } = useGpu();

  if (reading.status === "error") {
    return <p className="state">Cannot read the GPU: {reading.message}</p>;
  }
  if (reading.status === "loading") {
    return <p className="state">Reading sensors…</p>;
  }

  const s = reading.snapshot;
  const vram = percentOf(s.vram_used_mib, s.vram_total_mib);
  const metrics: [string, string][] = [
    ["Temperature", formatValue(s.temp_edge_c, "°C")],
    ["Hotspot", formatValue(s.temp_junction_c, "°C")],
    ["Memory", formatValue(s.temp_mem_c, "°C")],
    ["Power", `${formatValue(s.power_w, "W")} / ${formatValue(s.power_cap_w, "W")}`],
    ["Fan", formatValue(s.fan_rpm, "rpm")],
    ["GPU clock", formatValue(s.sclk_mhz, "MHz")],
    ["VRAM clock", formatValue(s.mclk_mhz, "MHz")],
    ["Voltage", formatValue(s.vddgfx_mv, "mV")],
    ["GPU usage", formatValue(s.gpu_busy_pct, "%")],
    ["VRAM", `${formatValue(s.vram_used_mib, "MiB")} (${formatValue(vram, "%")})`],
  ];

  return (
    <section className="section">
      <dl className="metrics">
        {metrics.map(([label, value]) => (
          <div className="metric" key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
