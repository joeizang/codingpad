import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { CrashBoundary } from "./CrashBoundary";
import "@fontsource-variable/hanken-grotesk";
import "@fontsource-variable/jetbrains-mono";
import "./styles.css";

performance.mark("frontend-evaluated");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <CrashBoundary>
      <App />
    </CrashBoundary>
  </StrictMode>,
);
