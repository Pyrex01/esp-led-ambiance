import React from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

const stats = [
  ["Serving", "Brotli assets from flash"],
  ["Network", "Station mode over local Wi-Fi"],
  ["Runtime", "Rust + Embassy on ESP32-S3"]
];

function App() {
  return (
    <main className="shell">
      <section className="panel">
        <div>
          <p className="eyebrow">ESP32-S3 WROOM-1</p>
          <h1>React UI served by Rust firmware</h1>
          <p className="summary">
            This page was built by Vite, compressed with Brotli at firmware
            build time, embedded into flash, and returned with
            <code> Content-Encoding: br</code>.
          </p>
        </div>

        <dl className="stats">
          {stats.map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
