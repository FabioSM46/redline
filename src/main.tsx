import { invoke } from "@tauri-apps/api/core";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, parseThemeSetting } from "./theme.ts";

function render() {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

// The saved theme is applied before the first render, so the interface never paints in the
// wrong theme. Until it arrives (or if it cannot be read) the CSS follows the desktop.
invoke<{ theme: unknown }>("get_settings")
  .then((settings) => applyTheme(parseThemeSetting(settings.theme)))
  .catch(() => applyTheme("system"))
  .finally(render);
