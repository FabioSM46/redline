import { useState } from "react";
import { TabList } from "../shell/TabList.tsx";
import { SETTINGS_TABS, type SettingsTabId } from "../tabs.ts";
import { General } from "./settings/General.tsx";

/** Settings (D8). General only in M1; Capture and Shortcuts join SETTINGS_TABS later. */
export function Settings() {
  const [tab, setTab] = useState<SettingsTabId>("general");
  return (
    <section className="section">
      <TabList className="subtabs" label="Settings" tabs={SETTINGS_TABS} active={tab} onSelect={setTab} />
      {tab === "general" && <General />}
    </section>
  );
}
