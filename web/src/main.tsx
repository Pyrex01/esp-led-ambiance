import React, { useState, useEffect, useCallback, useRef } from "react";
import { createRoot } from "react-dom/client";
import { Power, Sun, Palette, Wand2, Plus, Trash2, Play } from "lucide-react";
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
const initialFrames = ["#ff6b6b", "#ffd166", "#b9f18f", "#8fd3ff"];

function App() {
  const [state, setState] = useState<LedState>({
    power: true,
    color: [255, 165, 0],
    brightness: 128,
    pattern: 0,
    ledMask: FULL_LED_MASK.slice(),
  });

  const [customColors, setCustomColors] = useState<[number, number, number][]>([]);
  const [keyframes, setKeyframes] = useState(initialFrames);
  const [duration, setDuration] = useState(500);
  const [phase, setPhase] = useState(0);
  const keyframesRef = useRef(keyframes);
  const durationRef = useRef(duration);
  keyframesRef.current = keyframes;
  durationRef.current = duration;
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
        if (current.pattern === 6) sendAnimation(socket);
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

  const sendAnimation = (socket = ws.current) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) return;
    const colors = keyframesRef.current.slice(0, 8).map(hexToRgb);
    const ms = durationRef.current;
    const packet = new Uint8Array([0xA1, colors.length, ms & 255, ms >> 8, ...colors.flat()]);
    socket.send(packet);
  };

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

  useEffect(() => {
    let frame = 0;
    let lastStep = -1;
    const tick = (time: number) => {
      // The ESP32 emits a frame every 30 ms. Keep the preview in step with it
      // and avoid rerendering the complete dashboard at the browser's 60 Hz.
      const step = Math.floor(time / 30);
      if (step !== lastStep) {
        lastStep = step;
        setPhase(step);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, []);

  const rgbToHex = (r: number, g: number, b: number) => "#" + [r, g, b].map(x => x.toString(16).padStart(2, '0')).join('');
  const hexToRgb = (hex: string): [number, number, number] => {
    const r = parseInt(hex.substring(1, 3), 16);
    const g = parseInt(hex.substring(3, 5), 16);
    const b = parseInt(hex.substring(5, 7), 16);
    return [r, g, b];
  };

  const traceValueRef = useRef<boolean | null>(null);
  const previousTraceIndexRef = useRef<number | null>(null);
  const paintTraceIndex = (index: number) => {
    const value = traceValueRef.current;
    const previousIndex = previousTraceIndexRef.current;
    if (value === null) return;

    const mask = stateRef.current.ledMask.slice();
    const indices = [index];
    if (previousIndex !== null) {
      const forward = (index - previousIndex + LED_COUNT) % LED_COUNT;
      const step = forward <= LED_COUNT / 2 ? 1 : -1;
      const distance = step === 1 ? forward : LED_COUNT - forward;
      for (let offset = 1; offset < distance; offset++) {
        indices.push((previousIndex + step * offset + LED_COUNT) % LED_COUNT);
      }
    }
    let changed = false;
    for (const ledIndex of indices) {
      const byteIndex = Math.floor(ledIndex / 8);
      const bit = 1 << (ledIndex % 8);
      const nextByte = value ? mask[byteIndex] | bit : mask[byteIndex] & ~bit;
      if (nextByte !== mask[byteIndex]) {
        mask[byteIndex] = nextByte;
        changed = true;
      }
    }
    previousTraceIndexRef.current = index;
    if (changed) updateState({ ledMask: mask });
  };
  const ledAtPointer = (event: React.PointerEvent<SVGSVGElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const x = (event.clientX - bounds.left) / bounds.width * 300;
    const y = (event.clientY - bounds.top) / bounds.height * 300;
    let nearest = -1;
    let nearestDistance = Infinity;
    ledPoints.forEach(([ledX, ledY], index) => {
      const distance = (ledX - x) ** 2 + (ledY - y) ** 2;
      if (distance < nearestDistance) {
        nearest = index;
        nearestDistance = distance;
      }
    });
    if (nearestDistance <= 24 ** 2) paintTraceIndex(nearest);
  };
  const startTrace = (event: React.PointerEvent<SVGSVGElement>) => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    event.preventDefault();
    const target = event.target as Element;
    const ledGroup = target.closest("[data-led-index]");
    const index = ledGroup ? Number(ledGroup.getAttribute("data-led-index")) : -1;
    if (index < 0) return;
    const value = !isLedOn(index);
    traceValueRef.current = value;
    previousTraceIndexRef.current = index;
    event.currentTarget.setPointerCapture(event.pointerId);
    paintTraceIndex(index);
  };
  const endTrace = (event: React.PointerEvent<SVGSVGElement>) => {
    traceValueRef.current = null;
    previousTraceIndexRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
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
  const colorAt = (index: number): [number, number, number] => {
    if (state.pattern === 1 || state.pattern === 5) {
      const hue = ((index / LED_COUNT * 360 + phase * (state.pattern === 5 ? 2.4 : 1)) % 360);
      const chroma = 1 - Math.abs(2 * .55 - 1);
      const x = chroma * (1 - Math.abs((hue / 60) % 2 - 1));
      const m = .55 - chroma / 2;
      const rgb = hue < 60 ? [chroma, x, 0] : hue < 120 ? [x, chroma, 0] : hue < 180 ? [0, chroma, x] : hue < 240 ? [0, x, chroma] : hue < 300 ? [x, 0, chroma] : [chroma, 0, x];
      return rgb.map((v) => Math.round((v + m) * 255)) as [number, number, number];
    }
    if (state.pattern === 3 && (index + Math.floor(phase * 3)) % 24 >= 8) return [12, 14, 12];
    if (state.pattern === 4 && Math.floor(phase) % 16 < 8) return [12, 14, 12];
    if (state.pattern === 6 && keyframes.length) {
      const idx = Math.floor(phase * 30 / duration) % keyframes.length;
      return hexToRgb(keyframes[idx]);
    }
    return state.color;
  };
  const stripSides = [
    { from: [20, 20], to: [280, 20], path: "M20 20 H280" },
    { from: [280, 20], to: [280, 280], path: "M280 20 V280" },
    { from: [280, 280], to: [20, 280], path: "M280 280 H20" },
    { from: [20, 280], to: [20, 20], path: "M20 280 V20" },
  ];

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
            <div className="led-square" role="group" aria-label="480 individually controlled LEDs, 120 per side">
              <svg className="led-map" viewBox="0 0 300 300" aria-label="Click or trace individual LED pixels to toggle them"
                onPointerDown={startTrace} onPointerMove={(event) => { if (traceValueRef.current !== null) ledAtPointer(event); }}
                onPointerUp={endTrace} onPointerCancel={endTrace}>
                <path className="strip-track" d="M20 20 H280 V280 H20 Z" />
                <defs>{stripSides.map((side, sideIndex) => <linearGradient key={sideIndex} id={`strip-gradient-${sideIndex}`} x1={side.from[0]} y1={side.from[1]} x2={side.to[0]} y2={side.to[1]} gradientUnits="userSpaceOnUse">
                  {Array.from({ length: LEDS_PER_SIDE }, (_, offset) => {
                    const index = sideIndex * LEDS_PER_SIDE + offset;
                    const on = state.power && isLedOn(index);
                    const color = colorAt(index);
                    const pulse = state.pattern === 2 ? .15 + .85 * ((Math.sin(phase / 4) + 1) / 2) : 1;
                    const opacity = on ? Math.max(.08, state.brightness / 255) * pulse : .035;
                    return <stop key={offset} offset={`${offset / (LEDS_PER_SIDE - 1) * 100}%`} stopColor={rgbToHex(...color)} stopOpacity={opacity} />;
                  })}
                </linearGradient>)}</defs>
                {stripSides.map((side, index) => <path key={index} className="led-connections" d={side.path} stroke={`url(#strip-gradient-${index})`} style={{ filter: state.power ? `drop-shadow(0 0 5px ${rgbToHex(...state.color)})` : undefined }} />)}
                {ledPoints.map(([x, y], index) => {
                  const on = isLedOn(index);
                  const paintLed = (value?: boolean) => setLed(index, value);
                  return <g key={index} data-led-index={index} role="button" tabIndex={0} aria-label={`LED ${index + 1}, ${on ? "on" : "off"}`}
                    onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") paintLed(); }}>
                    <circle cx={x} cy={y} r="7" className="led-hit" />
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
              <button className={`pattern-btn ${state.pattern === 6 ? "active" : ""}`} onClick={() => updateState({ pattern: 6, power: true })}>My animation</button>
            </div>
            <div className="animation-editor">
              <div className="editor-heading"><span>Build a loop</span><button onClick={() => setKeyframes([...keyframes, "#8f8cff"].slice(0, 8))} disabled={keyframes.length >= 8}><Plus size={14}/> Add color</button></div>
              <div className="keyframe-row">{keyframes.map((color, i) => <div className="keyframe" key={i}><input aria-label={`Animation color ${i + 1}`} type="color" value={color} onChange={(e) => setKeyframes(keyframes.map((item, index) => index === i ? e.target.value : item))}/>{keyframes.length > 2 && <button aria-label="Remove color" onClick={() => setKeyframes(keyframes.filter((_, index) => index !== i))}><Trash2 size={12}/></button>}</div>)}</div>
              <label className="duration-label">Transition time <span>{(duration / 1000).toFixed(1)} sec</span></label>
              <input type="range" min="200" max="3000" step="100" value={duration} onChange={(e) => setDuration(Number(e.target.value))}/>
              <button className="apply-animation" onClick={() => { updateState({ pattern: 6, power: true }); sendAnimation(); }}><Play size={14} fill="currentColor"/> Play on LEDs</button>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
