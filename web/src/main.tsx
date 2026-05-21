import React, { useState, useEffect, useCallback, useRef } from "react";
import { createRoot } from "react-dom/client";
import { Power, Sun, Palette, Wand2, RefreshCw } from "lucide-react";
import "./style.css";

interface LedState {
  power: boolean;
  color: [number, number, number];
  brightness: number;
  pattern: number;
}

const PRESET_COLORS: [number, number, number][] = [
  [255, 255, 255], // Warm White (approx)
  [200, 230, 255], // Cool White
  [255, 0, 0],     // Red
  [0, 255, 0],     // Green
  [0, 0, 255],     // Blue
  [255, 0, 255],   // Purple
  [255, 165, 0],   // Orange
  [0, 255, 255],   // Cyan
];

const PATTERNS = [
  "Solid",
  "Rainbow",
  "Pulse",
  "Chase",
  "Strobe",
  "Flow",
];

function App() {
  const [state, setState] = useState<LedState>({
    power: true,
    color: [255, 165, 0],
    brightness: 128,
    pattern: 0,
  });

  const [connected, setConnected] = useState(false);
  const ws = useRef<WebSocket | null>(null);

  const connect = useCallback(() => {
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${protocol}//${window.location.host}/ws`;
    
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";

    socket.onopen = () => {
      setConnected(true);
      console.log("Connected to ESP32");
    };

    socket.onclose = () => {
      setConnected(false);
      console.log("Disconnected, retrying...");
      setTimeout(connect, 2000);
    };

    ws.current = socket;
  }, []);

  useEffect(() => {
    connect();
    return () => ws.current?.close();
  }, [connect]);

  const sendUpdate = useCallback((newState: LedState) => {
    if (ws.current?.readyState === WebSocket.OPEN) {
      const packed = new Uint8Array([
        newState.power ? 1 : 0,
        newState.color[0],
        newState.color[1],
        newState.color[2],
        newState.brightness,
        newState.pattern
      ]);
      ws.current.send(packed);
    }
  }, []);

  const updateState = (updates: Partial<LedState>) => {
    setState((prev) => {
      const newState = { ...prev, ...updates };
      sendUpdate(newState);
      return newState;
    });
  };

  const rgbToHex = (r: number, g: number, b: number) => {
    return "#" + [r, g, b].map(x => x.toString(16).padStart(2, '0')).join('');
  };

  const hexToRgb = (hex: number): [number, number, number] => {
    const r = (hex >> 16) & 255;
    const g = (hex >> 8) & 255;
    const b = hex & 255;
    return [r, g, b];
  };

  const glowColor = state.power 
    ? `rgba(${state.color[0]}, ${state.color[1]}, ${state.color[2]}, ${state.brightness / 255})`
    : "rgba(0, 0, 0, 0.2)";

  return (
    <main className="shell">
      <div className="container">
        <header className="header">
          <div className="status-group">
            <span className={`status-indicator ${connected ? "online" : "offline"}`} />
            <p className="eyebrow">{connected ? "System Online" : "Connecting..."}</p>
          </div>
          <h1>Ceiling LED</h1>
        </header>

        <section className="visualizer-section">
          <div className="led-square-wrapper">
            <div 
              className="led-square" 
              style={{ 
                borderColor: rgbToHex(...state.color),
                boxShadow: `0 0 ${state.brightness / 4}px ${glowColor}, inset 0 0 ${state.brightness / 8}px ${glowColor}`,
                opacity: state.power ? 1 : 0.3
              }}
            >
              <div className="square-content">
                <span className="meters">14 Meters</span>
                <span className="strip-type">SK6812 RGBW</span>
              </div>
            </div>
          </div>
        </section>

        <section className="controls-grid">
          <div className="control-card power-card">
            <button 
              className={`power-btn ${state.power ? "on" : "off"}`}
              onClick={() => updateState({ power: !state.power })}
            >
              <Power size={32} />
              <span>{state.power ? "Power On" : "Power Off"}</span>
            </button>
          </div>

          <div className="control-card slider-card">
            <div className="card-header">
              <Sun size={18} />
              <h3>Brightness</h3>
              <span className="value-label">{Math.round((state.brightness / 255) * 100)}%</span>
            </div>
            <input 
              type="range" 
              min="0" 
              max="255" 
              value={state.brightness}
              onChange={(e) => updateState({ brightness: parseInt(e.target.value) })}
            />
          </div>

          <div className="control-card color-card">
            <div className="card-header">
              <Palette size={18} />
              <h3>Color Presets</h3>
            </div>
            <div className="color-grid">
              {PRESET_COLORS.map((color, i) => (
                <button
                  key={i}
                  className="color-preset"
                  style={{ backgroundColor: rgbToHex(...color) }}
                  onClick={() => updateState({ color, power: true })}
                />
              ))}
              <div className="custom-color-wrapper">
                <input 
                  type="color" 
                  value={rgbToHex(...state.color)}
                  onChange={(e) => {
                    const hex = parseInt(e.target.value.substring(1), 16);
                    updateState({ color: hexToRgb(hex), power: true });
                  }}
                />
              </div>
            </div>
          </div>

          <div className="control-card pattern-card">
            <div className="card-header">
              <Wand2 size={18} />
              <h3>Animation</h3>
            </div>
            <div className="pattern-list">
              {PATTERNS.map((name, i) => (
                <button
                  key={i}
                  className={`pattern-btn ${state.pattern === i ? "active" : ""}`}
                  onClick={() => updateState({ pattern: i, power: true })}
                >
                  {name}
                </button>
              ))}
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
