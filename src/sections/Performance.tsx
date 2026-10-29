import { useState } from "react";
import { TabList } from "../shell/TabList.tsx";
import { PERFORMANCE_TABS, type PerformanceTabId, type SectionProps } from "../tabs.ts";
import { Metrics } from "./performance/Metrics.tsx";
import { Tuning } from "./performance/Tuning.tsx";

/** Performance, in two parts (D8): Metrics and Tuning. */
export function Performance({ navigate }: SectionProps) {
  const [tab, setTab] = useState<PerformanceTabId>("metrics");
  return (
    <section className="section">
      <TabList className="subtabs" label="Performance" tabs={PERFORMANCE_TABS} active={tab} onSelect={setTab} />
      {tab === "metrics" ? <Metrics /> : <Tuning navigate={navigate} />}
    </section>
  );
}
