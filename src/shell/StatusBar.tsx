import { useGpu } from "../GpuContext.tsx";
import { statusBarItems } from "../gpu.ts";

/** Always visible: GPU name, temperature and power of the selected GPU. */
export function StatusBar() {
  const { gpu, reading } = useGpu();
  const [name, ...values] = statusBarItems(gpu.name, reading);
  return (
    <footer className="statusbar">
      <span className="statusbar-name">{name}</span>
      {values.map((value, i) => (
        <span className="statusbar-value" key={i}>
          {value}
        </span>
      ))}
      {reading.status === "error" && (
        <span className="statusbar-error" title={reading.message}>
          Sensors unreadable
        </span>
      )}
    </footer>
  );
}
