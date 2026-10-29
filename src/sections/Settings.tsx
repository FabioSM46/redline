import { useState } from "react";
import { TabList } from "../shell/TabList.tsx";
import { SETTINGS_TABS, type SettingsTabId } from "../tabs.ts";
import { Capture } from "./settings/Capture.tsx";
import { General } from "./settings/General.tsx";

/** Settings (D8): General and Capture; Shortcuts joins SETTINGS_TABS later. */
export function Settings() {
  const [tab, setTab] = useState<SettingsTabId>("general");
  return (
    <section className="section">
      <TabList className="subtabs" label="Settings" tabs={SETTINGS_TABS} active={tab} onSelect={setTab} />
      {tab === "general" && <General />}
      {tab === "capture" && <Capture />}
    </section>
  );
}
