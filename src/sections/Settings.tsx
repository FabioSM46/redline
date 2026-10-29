import { useState } from "react";
import { TabList } from "../shell/TabList.tsx";
import { SETTINGS_TABS, type SettingsTabId } from "../tabs.ts";
import { About } from "./settings/About.tsx";
import { Capture } from "./settings/Capture.tsx";
import { Dependencies } from "./settings/Dependencies.tsx";
import { General } from "./settings/General.tsx";
import { Shortcuts } from "./settings/Shortcuts.tsx";

/** Settings (D8): General, Capture, Shortcuts, Dependencies and About. */
export function Settings() {
  const [tab, setTab] = useState<SettingsTabId>("general");
  return (
    <section className="section">
      <TabList className="subtabs" label="Settings" tabs={SETTINGS_TABS} active={tab} onSelect={setTab} />
      {tab === "general" && <General />}
      {tab === "capture" && <Capture />}
      {tab === "shortcuts" && <Shortcuts />}
      {tab === "dependencies" && <Dependencies />}
      {tab === "about" && <About />}
    </section>
  );
}
