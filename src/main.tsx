import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { initializeTheme } from "./lib/theme";
import "./theme.css";
import "./styles.css";
import "./settings.css";

// Apply the last known theme before the first React render so startup never
// flashes the wrong palette. The backend remains the source of truth and
// re-applies the persisted choice once settings load.
initializeTheme();

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
