import { useGpu } from "../GpuContext.tsx";
import { showGpuSelector } from "../gpu.ts";
import { TABS, type TabId } from "../tabs.ts";
import { TabList } from "./TabList.tsx";

type Props = {
  active: TabId;
  onSelect: (tab: TabId) => void;
};

/** The Redline mark, the four tabs (D8) and, with more than one GPU, the GPU selector. */
export function TopBar({ active, onSelect }: Props) {
  const { gpus, gpu, selectGpu } = useGpu();
  return (
    <header className="topbar">
      <Mark />
      <TabList className="tabs" label="Sections" tabs={TABS} active={active} onSelect={onSelect} />
      {showGpuSelector(gpus) && (
        <label className="gpu-select">
          <span className="visually-hidden">GPU</span>
          <select value={gpu.id} onChange={(e) => selectGpu(e.target.value)}>
            {gpus.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name} ({g.pci_slot})
              </option>
            ))}
          </select>
        </label>
      )}
    </header>
  );
}

export function Mark() {
  return (
    <span className="mark">
      <span className="mark-dot" aria-hidden="true" />
      Redline
    </span>
  );
}
