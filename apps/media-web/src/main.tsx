import "@assemble/display";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("MEDIA_ROOT_MISSING");
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
