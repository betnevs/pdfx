import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { PrototypeApp } from "./PrototypeApp";
import "./prototype.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

createRoot(root).render(
  <StrictMode>
    <PrototypeApp />
  </StrictMode>,
);
