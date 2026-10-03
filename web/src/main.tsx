import React, { useState, useEffect, useCallback, useRef } from "react";
import { createRoot } from "react-dom/client";
import { Power, Sun, Palette, Wand2, Plus, Trash2 } from "lucide-react";
import "./style.css";

interface LedState {
  power: boolean;
  color: [number, number, number];
  brightness: number;
  pattern: number;
  ledMask: number[];
}

const LEDS_PER_SIDE = 120;
const LED_COUNT = LEDS_PER_SIDE * 4;
const LED_MASK_BYTES = LED_COUNT / 8;
const FULL_LED_MASK = Array<number>(LED_MASK_BYTES).fill(0xff);

const DEFAULT_PRESETS: [number, number, number][] = [
  [255, 255, 255], [200, 230, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 0, 255], [255, 165, 0], [0, 255, 255],
];

const PATTERNS = ["Solid", "Rainbow", "Pulse", "Chase", "Strobe", "Flow"];

function App() {
  const [state, setState] = useState<LedState>({
    power: true,
    color: [255, 165, 0],
    brightness: 128,
    pattern: 0,
    ledMask: FULL_LED_MASK.slice(),
  });

  const [customColors, setCustomColors] = useState<[number, number, number][]>([]);
  const [connectionStatus, setConnectionStatus] = useState<"connecting" | "reconnecting" | "connected">("connecting");
  const ws = useRef<WebSocket | null>(null);
  const stateRef = useRef(state);

  useEffect(() => {
    const saved = localStorage.getItem("customColors");
    if (saved) setCustomColors(JSON.parse(saved));
  }, []);

  const addCustomColor = (color: [number, number, number]) => {
    const newCustom = [...customColors, color].slice(-5);
    setCustomColors(newCustom);
    localStorage.setItem("customColors", JSON.stringify(newCustom));
  };

  const removeCustomColor = (index: number) => {
    const newCustom = customColors.filter((_, i) => i !== index);
    setCustomColors(newCustom);
    localStorage.setItem("customColors", JSON.stringify(newCustom));
  };

  const connect = useCallback(() => {
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${protocol}//${window.location.host}/ws`;
    let stopped = false;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let retryDelay = 1000;

    const openSocket = () => {
      if (stopped) return;
      setConnectionStatus((status) => status === "connected" ? "reconnecting" : status);
      let socket: WebSocket;
      try {
        socket = new WebSocket(url);
      } catch {
        scheduleRetry();
        return;
      }
    socket.binaryType = "arraybuffer";

      socket.onopen = () => {
        if (stopped) {
          socket.close();
          return;
        }
        retryDelay = 1000;
        ws.current = socket;
        setConnectionStatus("connected");
        const current = stateRef.current;
        socket.send(new Uint8Array([current.power ? 1 : 0, ...current.color, current.brightness, current.pattern, ...current.ledMask]));
      };
    socket.onclose = () => {
        if (ws.current === socket) ws.current = null;
        if (stopped) return;
        setConnectionStatus("reconnecting");
        scheduleRetry();
    };
      socket.onerror = () => socket.close();

    ws.current = socket;
    };

    const scheduleRetry = () => {
      if (stopped || retryTimer) return;
      setConnectionStatus("reconnecting");
      retryTimer = setTimeout(() => {
        retryTimer = undefined;
        openSocket();
      }, retryDelay);
      retryDelay = Math.min(retryDelay * 2, 10000);
    };

    openSocket();
    return () => {
      stopped = true;
      if (retryTimer) clearTimeout(retryTimer);
      const socket = ws.current;
      ws.current = null;
      socket?.close();
    };
  }, []);

  useEffect(() => {
    return connect();
  }, [connect]);

  const sendUpdate = useCallback((newState: LedState) => {
    if (ws.current?.readyState === WebSocket.OPEN) {
      const packed = new Uint8Array([
        newState.power ? 1 : 0,
        newState.color[0],
        newState.color[1],
        newState.color[2],
        newState.brightness,
        newState.pattern,
        ...newState.ledMask,
      ]);
      ws.current.send(packed);
    }
  }, []);

  const updateState = (updates: Partial<LedState>) => {
    const newState = { ...stateRef.current, ...updates };
    stateRef.current = newState;
    setState(newState);
    sendUpdate(newState);
  };

  const rgbToHex = (r: number, g: number, b: number) => "#" + [r, g, b].map(x => x.toString(16).padStart(2, '0')).join('');
  const hexToRgb = (hex: string): [number, number, number] => {
    const r = parseInt(hex.substring(1, 3), 16);
    const g = parseInt(hex.substring(3, 5), 16);
    const b = parseInt(hex.substring(5, 7), 16);
    return [r, g, b];
  };

  const [traceValue, setTraceValue] = useState<boolean | null>(null);
  const setLed = (index: number, value?: boolean) => {
    const mask = stateRef.current.ledMask.slice();
    const byteIndex = Math.floor(index / 8);
    const bit = 1 << (index % 8);
    const currentlyOn = (mask[byteIndex] & bit) !== 0;
    const turnOn = value ?? !currentlyOn;
    mask[byteIndex] = turnOn ? mask[byteIndex] | bit : mask[byteIndex] & ~bit;
    updateState({ ledMask: mask });
  };
  const ledPoints = Array.from({ length: LED_COUNT }, (_, index) => {
    const side = Math.floor(index / LEDS_PER_SIDE);
    const t = (index % LEDS_PER_SIDE) / LEDS_PER_SIDE;
    if (side === 0) return [20 + t * 260, 20];
    if (side === 1) return [280, 20 + t * 260];
    if (side === 2) return [280 - t * 260, 280];
    return [20, 280 - t * 260];
  });
  const isLedOn = (index: number) => (state.ledMask[Math.floor(index / 8)] & (1 << (index % 8))) !== 0;
  const litConnections = ledPoints.slice(0, -1).flatMap(([x1, y1], index) => {
    if (Math.floor(index / LEDS_PER_SIDE) !== Math.floor((index + 1) / LEDS_PER_SIDE) || !isLedOn(index) || !isLedOn(index + 1)) return [];
    const [x2, y2] = ledPoints[index + 1];
    return [`M${x1},${y1} L${x2},${y2}`];
  }).join(" ");

  return (
    <main className="shell">
      <div className="container">
        <header className="header">
          <div className="status-group">
            <span className={`status-indicator ${connectionStatus}`} />
            <p className="eyebrow" aria-live="polite">
              {connectionStatus === "connected" ? "System Online" : connectionStatus === "reconnecting" ? "Reconnecting…" : "Connecting…"}
            </p>
          </div>
          <h1>Ceiling LED</h1>
        </header>

        <section className="visualizer-section">
          <div className="led-square-wrapper">
            <div className="led-square" role="group" aria-label="480 individually controlled LEDs, 120 per side" onPointerUp={() => setTraceValue(null)} onPointerLeave={() => setTraceValue(null)}>
              <svg className="led-map" viewBox="0 0 300 300" aria-label="Click or trace individual LED pixels to toggle them">
                {state.power && litConnections && <path className="led-connections" d={litConnections} style={{ stroke: rgbToHex(...state.color), filter: `drop-shadow(0 0 2px ${rgbToHex(...state.color)})`, opacity: Math.max(0.2, state.brightness / 255) }} />}
                {ledPoints.map(([x, y], index) => {
                  const on = isLedOn(index);
                  const paintLed = (value?: boolean) => setLed(index, value);
                  return <g key={index} role="button" tabIndex={0} aria-label={`LED ${index + 1}, ${on ? "on" : "off"}`}
                    onPointerDown={(event) => { event.preventDefault(); const value = !on; setTraceValue(value); paintLed(value); }}
                    onPointerEnter={() => { if (traceValue !== null) paintLed(traceValue); }}
                    onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") paintLed(); }}>
                    <circle cx={x} cy={y} r="2" className="led-hit" />
                    <circle cx={x} cy={y} r="1.25" className={`digital-led ${on && state.power ? "lit" : ""}`}
                      style={on && state.power ? { fill: rgbToHex(...state.color), opacity: Math.max(0.2, state.brightness / 255) } : undefined} />
                  </g>;
                })}
                <text x="150" y="142" className="square-label">4 SIDES</text>
                <text x="150" y="159" className="square-sub-label">120 LEDs EACH</text>
              </svg>
            </div>
          </div>
        </section>

        <section className="controls-grid">
          <div className="control-card power-card">
            <button className={`power-btn ${state.power ? "on" : "off"}`} onClick={() => updateState({ power: !state.power })}>
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
            <input type="range" min="0" max="255" value={state.brightness} onChange={(e) => updateState({ brightness: parseInt(e.target.value) })} />
          </div>

          <div className="control-card color-card">
            <div className="card-header">
              <Palette size={18} />
              <h3>Presets</h3>
            </div>
            <div className="color-grid">
              {DEFAULT_PRESETS.map((color, i) => (
                <button key={i} className="color-preset" style={{ backgroundColor: rgbToHex(...color) }} onClick={() => updateState({ color, power: true })} />
              ))}
              {customColors.map((color, i) => (
                <div key={i} className="color-preset-wrapper" style={{ position: 'relative' }}>
                  <button className="color-preset" style={{ backgroundColor: rgbToHex(...color) }} onClick={() => updateState({ color, power: true })} />
                  <button className="remove-preset" onClick={() => removeCustomColor(i)}><Trash2 size={10} /></button>
                </div>
              ))}
              <div className="custom-color-wrapper">
                <input type="color" onChange={(e) => {
                  const color = hexToRgb(e.target.value);
                  updateState({ color, power: true });
                  addCustomColor(color);
                }} />
                <Plus size={16} />
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
                <button key={i} className={`pattern-btn ${state.pattern === i ? "active" : ""}`} onClick={() => updateState({ pattern: i, power: true })}>
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

createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
