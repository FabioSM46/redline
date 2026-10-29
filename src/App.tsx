import { type ComponentType, useState } from "react";
import "./App.css";
import { GpuProvider, type GpuListState, useGpuList } from "./GpuContext.tsx";
import { RecorderProvider } from "./RecorderContext.tsx";
import { Home } from "./sections/Home.tsx";
import { Performance } from "./sections/Performance.tsx";
import { Record } from "./sections/Record.tsx";
import { Settings } from "./sections/Settings.tsx";
import { Mark, TopBar } from "./shell/TopBar.tsx";
import { StatusBar } from "./shell/StatusBar.tsx";
import type { SectionProps, TabId } from "./tabs.ts";

/** One component per tab: each section fills its own file without touching the shell. */
const SECTIONS: { [Tab in TabId]: ComponentType<SectionProps> } = {
  home: Home,
  performance: Performance,
  record: Record,
  settings: Settings,
};

/** The shell (D8): top bar with the tabs, scrolling section content, fixed status bar. */
function App() {
  const gpus = useGpuList();
  const [tab, setTab] = useState<TabId>("home");

  if (gpus.status !== "ready") {
    return (
      <div className="shell">
        <header className="topbar">
          <Mark />
        </header>
        <main className="content">
          <p className="state">{shellMessage(gpus)}</p>
        </main>
      </div>
    );
  }

  const Section = SECTIONS[tab];
  return (
    <GpuProvider value={gpus.value}>
      <RecorderProvider>
        <div className="shell">
          <TopBar active={tab} onSelect={setTab} />
          <main className="content">
            <Section navigate={setTab} />
          </main>
          <StatusBar />
        </div>
      </RecorderProvider>
    </GpuProvider>
  );
}

function shellMessage(gpus: Exclude<GpuListState, { status: "ready" }>): string {
  switch (gpus.status) {
    case "loading":
      return "Looking for GPUs…";
    case "empty":
      return "No AMD GPU found. Redline needs the amdgpu driver.";
    case "error":
      return `Cannot list the GPUs: ${gpus.message}`;
  }
}

export default App;
