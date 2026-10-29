import { useGpu } from "../GpuContext.tsx";
import { type Figure, homeFigures } from "../home.ts";

/** The GPU state at a glance: five large figures, then the secondary ones (D8). */
export function Home() {
  const { reading } = useGpu();

  if (reading.status === "error") {
    return <p className="state">Cannot read the GPU: {reading.message}</p>;
  }
  if (reading.status === "loading") {
    return <p className="state">Reading sensors…</p>;
  }

  const { primary, secondary } = homeFigures(reading.snapshot);
  return (
    <section className="section">
      <Figures figures={primary} className="figures figures-primary" label="Main figures" />
      <Figures figures={secondary} className="figures figures-secondary" label="Secondary figures" />
    </section>
  );
}

function Figures({ figures, className, label }: { figures: Figure[]; className: string; label: string }) {
  return (
    <dl className={className} aria-label={label}>
      {figures.map((f) => (
        <div className="figure" key={f.label}>
          <dt>{f.label}</dt>
          <dd className={f.high ? "figure-value figure-high" : "figure-value"}>
            {f.value}
            {f.high && <span className="visually-hidden"> (high)</span>}
          </dd>
          {f.detail !== undefined && <dd className="figure-detail">{f.detail}</dd>}
        </div>
      ))}
    </dl>
  );
}
