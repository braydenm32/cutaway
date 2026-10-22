// Cutaway for Stream Deck.
//
// Keys and dials that run Cutaway: scenes and sources with live pictures
// and tally colours, take/cut, transitions, record, go live, markers,
// graphics, audio inputs (Stream Deck+ dials too), auto-switching,
// captions, media players, sound pads, Syrinscape, talkback, clips,
// timers, the rundown, guests, fade to black, instant replay, macros, PTZ
// cameras, the teleprompter, viewer comments and any Cutaway command.
//
// No dependencies: this speaks the Stream Deck plugin protocol and
// Cutaway's remote-control API (WebSocket + HTTP) itself, with the small
// WebSocket client below.

"use strict";

const net = require("net");
const http = require("http");
const crypto = require("crypto");
const { EventEmitter } = require("events");

const PREFIX = "app.cutaway.streamdeck.";
const DEFAULT_PORT = 8790;

// ---- a small WebSocket client (RFC 6455) ------------------------------------

class WebSocketClient extends EventEmitter {
  /** `path` includes any query; `headers` are extra request headers. */
  constructor(host, port, path, headers = {}) {
    super();
    this.host = host;
    this.port = port;
    this.path = path;
    this.headers = headers;
    this.open = false;
    this.closed = false;
    this.buffer = Buffer.alloc(0);
    this.fragments = [];
    this.key = crypto.randomBytes(16).toString("base64");
    this.socket = net.connect({ host, port });
    this.socket.setNoDelay(true);
    this.socket.on("connect", () => {
      const lines = [
        `GET ${path} HTTP/1.1`,
        `Host: ${host}:${port}`,
        "Upgrade: websocket",
        "Connection: Upgrade",
        `Sec-WebSocket-Key: ${this.key}`,
        "Sec-WebSocket-Version: 13",
        ...Object.entries(headers).map(([k, v]) => `${k}: ${v}`),
        "",
        "",
      ];
      this.socket.write(lines.join("\r\n"));
    });
    this.socket.on("data", (d) => this._data(d));
    this.socket.on("error", (e) => this._fail(e));
    this.socket.on("close", () => this._fail(new Error("connection closed")));
  }

  _fail(err) {
    if (this.closed) return;
    this.closed = true;
    this.open = false;
    this.socket.destroy();
    if (!this.handshook) this.emit("error", err);
    this.emit("close");
  }

  _data(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    if (!this.handshook) {
      const end = this.buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const head = this.buffer.subarray(0, end).toString("latin1");
      this.buffer = this.buffer.subarray(end + 4);
      const status = Number((head.match(/^HTTP\/1\.1 (\d+)/) || [])[1] || 0);
      const accept = (head.match(/sec-websocket-accept:\s*(\S+)/i) || [])[1];
      const want = crypto.createHash("sha1").update(this.key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
      if (status !== 101 || accept !== want) {
        const err = new Error(status ? `HTTP ${status}` : "not a WebSocket server");
        err.status = status;
        this._fail(err);
        return;
      }
      this.handshook = true;
      this.open = true;
      this.emit("open");
    }
    this._frames();
  }

  _frames() {
    for (;;) {
      const b = this.buffer;
      if (b.length < 2) return;
      const fin = (b[0] & 0x80) !== 0;
      const opcode = b[0] & 0x0f;
      const masked = (b[1] & 0x80) !== 0;
      let len = b[1] & 0x7f;
      let at = 2;
      if (len === 126) {
        if (b.length < 4) return;
        len = b.readUInt16BE(2);
        at = 4;
      } else if (len === 127) {
        if (b.length < 10) return;
        len = Number(b.readBigUInt64BE(2));
        at = 10;
      }
      const mask = masked ? b.subarray(at, at + 4) : null;
      if (masked) at += 4;
      if (b.length < at + len) return;
      let payload = Buffer.from(b.subarray(at, at + len));
      this.buffer = b.subarray(at + len);
      if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
      if (opcode === 0x8) {
        this._send(0x8, Buffer.alloc(0));
        this._fail(new Error("closed by the other side"));
        return;
      }
      if (opcode === 0x9) {
        this._send(0xa, payload);
        continue;
      }
      if (opcode === 0xa) continue;
      if (opcode === 0x1 || opcode === 0x2 || opcode === 0x0) {
        this.fragments.push(payload);
        if (fin) {
          const whole = Buffer.concat(this.fragments);
          this.fragments = [];
          this.emit("message", whole.toString("utf8"));
        }
      }
    }
  }

  _send(opcode, payload) {
    if (this.closed) return;
    const mask = crypto.randomBytes(4);
    const len = payload.length;
    let head;
    if (len < 126) {
      head = Buffer.from([0x80 | opcode, 0x80 | len]);
    } else if (len < 65536) {
      head = Buffer.alloc(4);
      head[0] = 0x80 | opcode;
      head[1] = 0x80 | 126;
      head.writeUInt16BE(len, 2);
    } else {
      head = Buffer.alloc(10);
      head[0] = 0x80 | opcode;
      head[1] = 0x80 | 127;
      head.writeBigUInt64BE(BigInt(len), 2);
    }
    const body = Buffer.alloc(len);
    for (let i = 0; i < len; i++) body[i] = payload[i] ^ mask[i & 3];
    this.socket.write(Buffer.concat([head, mask, body]));
  }

  send(text) {
    if (this.open) this._send(0x1, Buffer.from(text, "utf8"));
  }

  close() {
    if (this.open) this._send(0x8, Buffer.alloc(0));
    this._fail(new Error("closed"));
  }
}

// ---- Stream Deck --------------------------------------------------------------

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const sdPort = Number(arg("-port"));
const pluginUUID = arg("-pluginUUID");
const registerEvent = arg("-registerEvent");
if (!sdPort || !pluginUUID || !registerEvent) {
  console.error("Cutaway's Stream Deck plugin is started by the Stream Deck app.");
  process.exit(1);
}

const sd = new WebSocketClient("127.0.0.1", sdPort, "/");
function sdSend(obj) {
  sd.send(JSON.stringify(obj));
}
function log(message) {
  sdSend({ event: "logMessage", payload: { message: `[Cutaway] ${message}` } });
}
sd.on("open", () => {
  sdSend({ event: registerEvent, uuid: pluginUUID });
  sdSend({ event: "getGlobalSettings", context: pluginUUID });
});
sd.on("message", (text) => {
  let m;
  try {
    m = JSON.parse(text);
  } catch {
    return;
  }
  try {
    onStreamDeck(m);
  } catch (e) {
    log(`error handling ${m.event}: ${e && e.stack}`);
  }
});
// The Stream Deck app went away: so do we.
sd.on("error", () => process.exit(1));
sd.on("close", () => process.exit(0));
process.on("uncaughtException", (e) => log(`unexpected: ${e && e.stack}`));

// ---- the keys and dials on screen ---------------------------------------------

/** context → { kind, settings, controller, image, feedback, hold, hint, fetching, fetched, png } */
const keys = new Map();

function kindOf(actionUUID) {
  return actionUUID && actionUUID.startsWith(PREFIX) ? actionUUID.slice(PREFIX.length) : "";
}

function onStreamDeck(m) {
  const ctx = m.context;
  switch (m.event) {
    case "didReceiveGlobalSettings":
      applyGlobal((m.payload && m.payload.settings) || {});
      break;
    case "willAppear": {
      const p = m.payload || {};
      keys.set(ctx, { kind: kindOf(m.action), action: m.action, settings: p.settings || {}, controller: p.controller || "Keypad" });
      const k = keys.get(ctx);
      if (k.controller === "Encoder") sdSend({ event: "setFeedbackLayout", context: ctx, payload: { layout: "$B1" } });
      render(ctx, true);
      break;
    }
    case "willDisappear":
      keys.delete(ctx);
      break;
    case "didReceiveSettings": {
      const k = keys.get(ctx);
      if (k) {
        k.settings = (m.payload && m.payload.settings) || {};
        k.png = null;
        k.fetched = 0;
        render(ctx, true);
      }
      break;
    }
    case "keyDown":
      press(ctx, m);
      break;
    case "keyUp":
      release(ctx);
      break;
    case "dialRotate":
      rotate(ctx, (m.payload && m.payload.ticks) || 0);
      break;
    case "dialDown":
    case "touchTap": {
      const k = keys.get(ctx);
      if (k && k.kind === "audio") run(ctx, `audio:${target(k.settings.input)}`);
      break;
    }
    case "propertyInspectorDidAppear":
      sendLists(ctx, m.action);
      break;
    case "sendToPlugin": {
      const p = m.payload || {};
      if (p.kind === "global") applyGlobal(p.settings || {}, true);
      if (p.kind === "lists") sendLists(ctx, m.action);
      break;
    }
    case "systemDidWakeUp":
      reconnectSoon(0);
      break;
    default:
      break;
  }
}

// ---- Cutaway ------------------------------------------------------------------------

const cutaway = {
  global: { address: "127.0.0.1:8790", pin: "", thumbnails: "1000" },
  ws: null,
  state: null,
  connected: false,
  problem: "connecting…",
  nextId: 1,
  pending: new Map(),
  timer: null,
};

function hostPort() {
  let a = String(cutaway.global.address || "").trim().replace(/^[a-z]+:\/\//i, "").replace(/\/.*$/, "");
  if (!a) a = "127.0.0.1";
  const m = a.match(/^\[?([^\]]*?)\]?(?::(\d+))?$/);
  return { host: (m && m[1]) || "127.0.0.1", port: Number((m && m[2]) || DEFAULT_PORT) };
}

function applyGlobal(settings, save) {
  const before = JSON.stringify(cutaway.global);
  cutaway.global = { address: "127.0.0.1:8790", pin: "", thumbnails: "1000", ...settings };
  if (save) sdSend({ event: "setGlobalSettings", context: pluginUUID, payload: cutaway.global });
  if (JSON.stringify(cutaway.global) !== before || !cutaway.ws) {
    for (const k of keys.values()) {
      k.png = null;
      k.fetched = 0;
    }
    connect();
  }
}

function connect() {
  clearTimeout(cutaway.timer);
  if (cutaway.ws) {
    const old = cutaway.ws;
    cutaway.ws = null;
    old.removeAllListeners("close");
    old.on("close", () => {});
    old.close();
  }
  const { host, port } = hostPort();
  const pin = String(cutaway.global.pin || "").trim();
  const ws = new WebSocketClient(host, port, `/api/ws${pin ? `?pin=${encodeURIComponent(pin)}` : ""}`);
  cutaway.ws = ws;
  ws.on("open", () => {
    cutaway.connected = true;
    cutaway.problem = "";
    ws.send(JSON.stringify({ get: "state" }));
    log(`connected to ${host}:${port}`);
  });
  ws.on("message", (text) => {
    let m;
    try {
      m = JSON.parse(text);
    } catch {
      return;
    }
    if (m.type === "state") {
      const first = !cutaway.state;
      cutaway.state = m.state;
      renderAll(first);
    } else if (m.type === "reply") {
      const ctx = cutaway.pending.get(m.id);
      cutaway.pending.delete(m.id);
      if (!m.ok) {
        log(`"${m.command}": ${m.message}`);
        if (ctx) sdSend({ event: "showAlert", context: ctx });
      }
    }
  });
  ws.on("error", (e) => {
    if (cutaway.ws !== ws) return;
    cutaway.problem =
      e.status === 401 ? "wrong or missing PIN" : e.code === "ECONNREFUSED" ? `nothing at ${host}:${port} (is remote control on in Cutaway?)` : e.status ? `${host}:${port} isn't Cutaway (${e.message})` : e.message;
  });
  ws.on("close", () => {
    if (cutaway.ws !== ws) return;
    if (cutaway.connected) log("disconnected");
    cutaway.connected = false;
    cutaway.state = null;
    if (!cutaway.problem) cutaway.problem = "disconnected";
    renderAll(true);
    reconnectSoon(2000);
  });
}

function reconnectSoon(ms) {
  clearTimeout(cutaway.timer);
  cutaway.timer = setTimeout(connect, ms);
}

/** Run a Cutaway command for the key `ctx`. */
function run(ctx, command) {
  if (!cutaway.connected || !cutaway.ws) {
    if (ctx) sdSend({ event: "showAlert", context: ctx });
    return;
  }
  const id = cutaway.nextId++;
  if (ctx) cutaway.pending.set(id, ctx);
  cutaway.ws.send(JSON.stringify({ do: command, id }));
}

/** A setting naming a scene, source, graphic, input or player: "2" or "Host". */
function target(v) {
  const s = String(v == null || v === "" ? "1" : v).trim();
  return s;
}

/** Index (from 0) of a target in a list of {name}. */
function indexIn(list, v) {
  const s = target(v);
  if (/^\d+$/.test(s)) return Number(s) - 1;
  return (list || []).findIndex((x) => String(x.name).toLowerCase() === s.toLowerCase());
}

// ---- pressing ---------------------------------------------------------------------------

const HOLD_MS = 1000;

function holdNeeded(k, st) {
  if (k.kind === "record") return st && st.recording && k.settings.holdToStop !== false;
  if (k.kind === "live") return st && (st.live ? k.settings.holdToEnd !== false : k.settings.holdToStart !== false);
  return false;
}

function press(ctx) {
  const k = keys.get(ctx);
  if (!k) return;
  const st = cutaway.state;
  const s = k.settings;
  if (holdNeeded(k, st)) {
    // Long press: guards against ending a recording or stream by accident.
    k.hold = { at: Date.now(), timer: setTimeout(() => finishHold(ctx), HOLD_MS) };
    render(ctx);
    return;
  }
  switch (k.kind) {
    case "scene": {
      const how = s.press || "take";
      run(ctx, `${how === "preview" ? "preview" : how === "cut" ? "cut" : "scene"}:${target(s.scene)}`);
      break;
    }
    case "source":
      run(ctx, `source:${target(s.source)}`);
      break;
    case "take":
      run(ctx, s.how === "cut" ? "cut" : "take");
      break;
    case "transition":
      run(ctx, `transition:${s.style || "mix"}`);
      break;
    case "record":
      run(ctx, st && st.recording ? "record:off" : "record:on");
      break;
    case "live":
      run(ctx, st && st.live ? "live:off" : "live:on");
      break;
    case "marker":
      run(ctx, s.label && String(s.label).trim() ? `marker:${String(s.label).trim()}` : "marker");
      sdSend({ event: "showOk", context: ctx });
      break;
    case "graphic":
      run(ctx, `graphic:${target(s.graphic)}`);
      break;
    case "audio":
      run(ctx, `audio:${target(s.input)}`);
      break;
    case "auto":
      run(ctx, "auto");
      break;
    case "captions":
      run(ctx, "captions");
      break;
    case "media":
      run(ctx, `media:${target(s.player)}:${s.button || "toggle"}`);
      break;
    case "command":
      if (s.command && String(s.command).trim()) run(ctx, String(s.command).trim());
      else sdSend({ event: "showAlert", context: ctx });
      break;
    case "pad":
      // Sound starts on the press; "while held" pads stop on the release.
      run(ctx, `pad:${target(s.pad)}:press`);
      k.down = `pad:${target(s.pad)}:release`;
      break;
    case "syrinscape":
      run(ctx, `syrinscape:${target(s.button)}`);
      break;
    case "talkback":
      if ((s.mode || "hold") === "hold") {
        run(ctx, "talkback:on");
        k.down = "talkback:off";
      } else {
        run(ctx, "talkback");
      }
      break;
    case "clip":
      if (!st || !st.recording) {
        sdSend({ event: "showAlert", context: ctx });
        break;
      }
      run(ctx, s.seconds && Number(s.seconds) > 0 ? `clip:${Math.round(Number(s.seconds))}` : "clip");
      sdSend({ event: "showOk", context: ctx });
      break;
    case "timer":
      run(ctx, `timer:${clockName(s.clock)}:${s.button || "toggle"}`);
      break;
    case "rundown":
      run(ctx, `rundown:${s.button || "next"}`);
      break;
    case "guest": {
      const g = (st && st.guests || [])[indexIn(st && st.guests, s.guest)];
      if (!g || !g.source) sdSend({ event: "showAlert", context: ctx });
      else run(ctx, `${(s.press || "source") === "preview" ? "preview" : "source"}:${g.source}`);
      break;
    }
    case "ftb":
      run(ctx, "ftb");
      break;
    case "replay":
      if (st && st.replaying) run(ctx, "replay:stop");
      else if (!st || !st.recording) sdSend({ event: "showAlert", context: ctx });
      else {
        // (A camera's name, "quad" or "program"; a number would be read as seconds.)
        const angle = String(s.angle || "").trim().replace(/:/g, " ");
        run(ctx, "replay" + (s.slow ? ":slow" : "") + (angle && !/^\d+(\.\d+)?s?$/.test(angle) ? ":" + angle : ""));
      }
      break;
    case "macro":
      run(ctx, `macro:${target(s.macro)}`);
      break;
    case "ptz": {
      const cam = ptzOf(s.camera);
      if (!cam) {
        sdSend({ event: "showAlert", context: ctx });
        break;
      }
      const button = s.button || "preset";
      if (button === "preset") run(ctx, `ptz:${cam.source}:preset:${Math.max(1, Math.round(Number(s.preset) || 1))}`);
      else if (button === "home") run(ctx, `ptz:${cam.source}:home`);
      else {
        // Moves while held.
        run(ctx, `ptz:${cam.source}:${button}:${s.speed || "0.5"}`);
        k.down = `ptz:${cam.source}:stop`;
      }
      break;
    }
    case "prompter":
      if (!st || !st.prompter) sdSend({ event: "showAlert", context: ctx });
      else run(ctx, `prompter:${s.button || "toggle"}`);
      break;
    case "comment":
      if (!st || !st.comments) sdSend({ event: "showAlert", context: ctx });
      else run(ctx, `comment:${s.button || "toggle"}`);
      break;
    default:
      break;
  }
}

/** A PTZ camera setting ("1" = the first PTZ camera, or a name). */
function ptzOf(v) {
  const cams = (cutaway.state && cutaway.state.ptz) || [];
  return cams[indexIn(cams, v)];
}

/** A clock graphic setting ("2" = the second timer, or a name) as its name. */
function clockName(v) {
  const timers = ((cutaway.state && cutaway.state.clocks) || []).filter((c) => c.timer);
  const i = indexIn(timers, v);
  return timers[i] ? timers[i].name : target(v);
}

function timerOf(v) {
  const timers = ((cutaway.state && cutaway.state.clocks) || []).filter((c) => c.timer);
  return timers[indexIn(timers, v)];
}

function finishHold(ctx) {
  const k = keys.get(ctx);
  if (!k || !k.hold) return;
  k.hold = null;
  const st = cutaway.state;
  if (k.kind === "record") run(ctx, st && st.recording ? "record:off" : "record:on");
  if (k.kind === "live") run(ctx, st && st.live ? "live:off" : "live:on");
  render(ctx);
}

function release(ctx) {
  const k = keys.get(ctx);
  if (!k) return;
  if (k.down) {
    run(null, k.down);
    k.down = null;
  }
  if (!k.hold) return;
  // Let go too soon: say so on the key for a moment.
  clearTimeout(k.hold.timer);
  k.hold = null;
  k.hint = Date.now() + 1500;
  render(ctx);
  setTimeout(() => render(ctx), 1600);
}

/** Stream Deck+ dial: the input's level. */
function rotate(ctx, ticks) {
  const k = keys.get(ctx);
  if (!k || k.kind !== "audio" || !cutaway.state) return;
  const i = indexIn(cutaway.state.inputs, k.settings.input);
  const input = cutaway.state.inputs[i];
  if (!input) return;
  const step = Number(k.settings.step || 1);
  const base = k.gain != null && Date.now() - (k.gainAt || 0) < 1500 ? k.gain : input.gain_db;
  const db = Math.max(-60, Math.min(12, Math.round((base + ticks * step) * 2) / 2));
  k.gain = db;
  k.gainAt = Date.now();
  run(ctx, `gain:${target(k.settings.input)}:${db}`);
  render(ctx);
}

// ---- drawing ----------------------------------------------------------------------------

const C = {
  bg: "#15171c",
  raised: "#262a33",
  text: "#ffffff",
  dim: "#8a90a0",
  red: "#e5303e",
  green: "#2fbf5b",
  accent: "#3d7eff",
  amber: "#f2a516",
};

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function fit(text, max) {
  const s = String(text || "");
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** A 144×144 key picture. */
function svg(body, { bg = C.bg, border = null } = {}) {
  const frame = border ? `<rect x="4" y="4" width="136" height="136" rx="14" fill="none" stroke="${border}" stroke-width="8"/>` : "";
  const doc = `<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><rect width="144" height="144" rx="18" fill="${bg}"/>${body}${frame}</svg>`;
  return `data:image/svg+xml;charset=utf8,${encodeURIComponent(doc)}`;
}

function label(text, y, size = 22, color = C.text, weight = 700) {
  return `<text x="72" y="${y}" text-anchor="middle" font-family="Helvetica Neue, Helvetica, Arial, sans-serif" font-size="${size}" font-weight="${weight}" fill="${color}">${esc(text)}</text>`;
}

function clock(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  const p = (n) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`;
}

const ICONS = {
  record: (c) => `<circle cx="72" cy="58" r="26" fill="${c}"/>`,
  marker: (c) => `<polygon points="72,30 98,58 72,86 46,58" fill="${c}"/>`,
  play: (c) => `<polygon points="56,34 98,58 56,82" fill="${c}"/>`,
  pause: (c) => `<rect x="52" y="36" width="14" height="44" rx="3" fill="${c}"/><rect x="78" y="36" width="14" height="44" rx="3" fill="${c}"/>`,
  next: (c) => `<polygon points="48,36 84,58 48,80" fill="${c}"/><rect x="88" y="36" width="10" height="44" rx="2" fill="${c}"/>`,
  previous: (c) => `<polygon points="96,36 60,58 96,80" fill="${c}"/><rect x="46" y="36" width="10" height="44" rx="2" fill="${c}"/>`,
  restart: (c) =>
    `<path d="M72 34 a24 24 0 1 1 -22 14" fill="none" stroke="${c}" stroke-width="9" stroke-linecap="round"/><polygon points="40,30 62,40 44,56" fill="${c}"/>`,
  graphic: (c) => `<rect x="34" y="38" width="76" height="40" rx="5" fill="none" stroke="${c}" stroke-width="5"/><rect x="40" y="62" width="48" height="10" rx="2" fill="${c}"/>`,
  camera: (c) => `<rect x="36" y="42" width="54" height="36" rx="6" fill="${c}"/><polygon points="92,52 110,42 110,78 92,68" fill="${c}"/>`,
};

function offline(k) {
  const names = {
    scene: "Scene",
    source: "Source",
    take: "Take",
    transition: "Transition",
    record: "Record",
    live: "Go live",
    marker: "Marker",
    graphic: "Graphic",
    audio: "Audio",
    auto: "Auto",
    captions: "CC",
    media: "Media",
    command: "Command",
    pad: "Pad",
    syrinscape: "Syrinscape",
    talkback: "Talk",
    clip: "Clip",
    timer: "Timer",
    rundown: "Rundown",
    guest: "Guest",
    ftb: "FTB",
    replay: "Replay",
    macro: "Macro",
    ptz: "PTZ",
    prompter: "Prompter",
    comment: "Comment",
  };
  return svg(label(names[k.kind] || "Cutaway", 66, 22, C.dim) + label("not connected", 96, 15, C.amber, 400));
}

/** The picture (and, for dials, the feedback) a key should show now. */
function draw(k) {
  const st = cutaway.state;
  const s = k.settings;
  if (!st) return { image: offline(k) };
  if (k.hint && Date.now() < k.hint) return { image: svg(label("Hold", 64, 30) + label("to confirm", 94, 18, C.dim, 400)) };
  switch (k.kind) {
    case "scene":
    case "source": {
      const list = k.kind === "scene" ? st.scenes : st.sources;
      const i = indexIn(list, k.kind === "scene" ? s.scene : s.source);
      const item = list[i];
      if (!item) return { image: svg(label(k.kind === "scene" ? "No scene" : "No source", 66, 20, C.dim) + label(target(k.kind === "scene" ? s.scene : s.source), 96, 18, C.dim, 400)) };
      const tally = k.kind === "scene" ? (item.program ? "program" : item.preview ? "preview" : "off") : item.tally;
      // A live picture from Cutaway (fetched separately) when allowed.
      if (thumbsOn() && k.png) return { image: k.png, tally };
      const border = tally === "program" ? C.red : tally === "preview" ? C.green : null;
      const name = fit(item.name, 12);
      return { image: svg(ICONS.camera(C.dim) + label(name, 112, 19), { border }), tally };
    }
    case "take": {
      const ready = !!st.preview;
      const word = s.how === "cut" ? "CUT" : "TAKE";
      return { image: svg(label(word, 70, 34, ready ? C.text : C.dim) + label(ready ? fit(st.preview, 12) : st.preview_mode ? "nothing in preview" : "", 104, 15, C.green, 400), { bg: ready ? "#1d3a26" : C.bg }) };
    }
    case "transition": {
      const style = s.style || "mix";
      const on = st.transition === style;
      return { image: svg(label(style.toUpperCase(), 80, 28, on ? C.text : C.dim), { bg: on ? C.accent : C.bg }) };
    }
    case "record": {
      const holding = k.hold ? Math.min(1, (Date.now() - k.hold.at) / HOLD_MS) : 0;
      if (st.recording) {
        return { image: svg(ICONS.record(C.text) + label("REC", 108, 20) + label(clock(st.rec_seconds), 130, 16, C.text, 400) + holdBar(holding), { bg: C.red }) };
      }
      return { image: svg(ICONS.record(C.red) + label("REC", 112, 22, C.dim)) };
    }
    case "live": {
      const holding = k.hold ? Math.min(1, (Date.now() - k.hold.at) / HOLD_MS) : 0;
      if (st.live) {
        return { image: svg(label("LIVE", 66, 32) + label(clock(st.live_seconds), 98, 17, C.text, 400) + holdBar(holding), { bg: C.red }) };
      }
      return { image: svg(label("GO", 60, 26, C.dim) + label("LIVE", 92, 30, C.dim) + holdBar(holding)) };
    }
    case "marker":
      return { image: svg(ICONS.marker(st.recording ? C.amber : C.dim) + label(st.recording ? `Marker ${st.markers + 1}` : "Marker", 112, 18, st.recording ? C.text : C.dim, 400)) };
    case "graphic": {
      const g = st.graphics[indexIn(st.graphics, s.graphic)];
      if (!g) return { image: svg(label("No graphic", 72, 18, C.dim)) };
      return { image: svg(ICONS.graphic(g.on ? "#1b1b1b" : C.dim) + label(fit(g.name, 12), 112, 18, g.on ? "#1b1b1b" : C.text), { bg: g.on ? C.amber : C.bg }) };
    }
    case "audio": {
      const input = st.inputs[indexIn(st.inputs, s.input)];
      if (!input) return { image: svg(label("No input", 72, 18, C.dim)), feedback: { title: "Cutaway", value: "no input", indicator: { value: 0 } } };
      const gain = k.gain != null && Date.now() - (k.gainAt || 0) < 1500 ? k.gain : input.gain_db;
      // Level meter from -60 to 0 dBFS.
      const lvl = Math.max(0, Math.min(1, (input.level + 60) / 60));
      const lvlQ = Math.round(lvl * 20) / 20;
      const meterColor = input.level > -3 ? C.red : input.level > -12 ? C.amber : C.green;
      const meter = input.on ? `<rect x="22" y="70" width="${Math.round(100 * lvlQ)}" height="12" rx="3" fill="${meterColor}"/>` : "";
      const body =
        label(fit(input.name, 11), 40, 19) +
        `<rect x="22" y="70" width="100" height="12" rx="3" fill="${C.raised}"/>` +
        meter +
        label(input.on ? `${gain > 0 ? "+" : ""}${gain.toFixed(1)} dB` : "OFF", 116, 18, input.on ? C.text : C.red, input.on ? 400 : 700);
      return {
        image: svg(body, { bg: input.on ? C.bg : "#2a1418" }),
        feedback: {
          title: input.name,
          value: input.on ? `${gain > 0 ? "+" : ""}${gain.toFixed(1)} dB` : "OFF",
          indicator: { value: Math.round(faderPosition(gain) * 100) },
        },
      };
    }
    case "auto":
      return { image: svg(label("AUTO", 70, 28, st.auto ? C.text : C.dim) + label(st.auto ? "switching" : "off", 100, 15, st.auto ? C.text : C.dim, 400), { bg: st.auto ? C.accent : C.bg }) };
    case "captions": {
      const on = st.captions;
      const avail = st.captions_available;
      const cc = `<rect x="38" y="36" width="68" height="46" rx="8" fill="none" stroke="${on ? C.text : C.dim}" stroke-width="5"/>` + label("CC", 69, 24, on ? C.text : C.dim);
      return { image: svg(cc + label(avail ? (on ? "on" : "off") : "unavailable", 112, 16, avail ? C.text : C.amber, 400), { bg: on ? C.accent : C.bg }) };
    }
    case "media": {
      const p = (st.players || []).find((x) => String(x.source) === target(s.player) || String(x.name).toLowerCase() === target(s.player).toLowerCase());
      const button = s.button || "toggle";
      if (!p) return { image: svg(label("No player", 72, 18, C.dim)) };
      const icon = button === "toggle" ? (p.playing ? ICONS.pause : ICONS.play) : ICONS[button] || ICONS.play;
      return { image: svg(icon(p.playing ? C.text : C.dim) + label(fit(p.item || p.name, 12), 112, 16, C.text, 400), { bg: p.playing && button === "toggle" ? "#1d3a26" : C.bg }) };
    }
    case "command":
      return { image: svg(label(fit(s.label || s.command || "Command", 10), 80, 20)) };
    case "pad": {
      const p = (st.pads || [])[indexIn(st.pads, s.pad)];
      if (!p) return { image: svg(label("No pad", 72, 18, C.dim)) };
      const color = /^#[0-9a-f]{6}$/i.test(p.color || "") ? p.color : C.accent;
      const w = p.playing ? Math.round(144 * Math.max(0, Math.min(1, p.progress))) : 0;
      const body =
        (p.playing ? `<rect width="${w}" height="144" fill="${color}" opacity="0.75"/>` : `<rect x="10" y="118" width="124" height="8" rx="4" fill="${color}"/>`) +
        label(fit(p.name, 10), 70, 21) +
        label(p.playing ? `${Math.ceil(p.remaining)} s` : "", 100, 17, C.text, 400);
      return { image: svg(body, { bg: p.playing ? "#22262e" : C.bg, border: p.playing ? color : null }) };
    }
    case "syrinscape": {
      const b = (st.syrinscape || [])[indexIn(st.syrinscape, s.button)];
      if (!b) return { image: svg(label("Syrinscape", 64, 17, C.dim) + label("no button", 92, 15, C.amber, 400)) };
      const stop = b.kind === "stopall";
      const bg = stop ? "#4a1820" : b.playing ? "#9a6ee0" : C.bg;
      return { image: svg(label(fit(b.name, 10), 72, 21, b.playing ? "#111111" : C.text) + label(stop ? "stop all" : b.kind === "mood" ? "mood" : "sound", 104, 14, b.playing ? "#111111" : C.dim, 400), { bg }) };
    }
    case "talkback": {
      if (!st.talkback_available) return { image: svg(label("TALK", 66, 26, C.dim) + label("no talkback mic", 96, 13, C.amber, 400)) };
      const on = st.talkback;
      return { image: svg(label("TALK", 72, 32, on ? "#111111" : C.text) + label(on ? "talking" : (s.mode || "hold") === "hold" ? "hold" : "on / off", 104, 15, on ? "#111111" : C.dim, 400), { bg: on ? C.amber : C.bg }) };
    }
    case "clip": {
      const secs = s.seconds && Number(s.seconds) > 0 ? Math.round(Number(s.seconds)) : st.clip_seconds || 30;
      const c = st.recording ? C.text : C.dim;
      const scissors = `<circle cx="52" cy="54" r="11" fill="none" stroke="${c}" stroke-width="5"/><circle cx="92" cy="54" r="11" fill="none" stroke="${c}" stroke-width="5"/><line x1="60" y1="46" x2="94" y2="18" stroke="${c}" stroke-width="5" stroke-linecap="round"/><line x1="84" y1="46" x2="50" y2="18" stroke="${c}" stroke-width="5" stroke-linecap="round"/>`;
      return { image: svg(scissors + label("CLIP", 98, 20, c) + label(st.recording ? `last ${secs} s` : "not recording", 124, 14, st.recording ? C.text : C.dim, 400)) };
    }
    case "timer": {
      const t = timerOf(s.clock);
      if (!t) return { image: svg(label("Timer", 66, 22, C.dim) + label("no countdown", 96, 14, C.amber, 400)) };
      const btn = s.button || "toggle";
      const word = { toggle: t.running ? "pause" : "start", start: "start", pause: "pause", reset: "reset", stop: "stop" }[btn] || btn;
      return { image: svg(label(fit(t.name, 11), 38, 16, C.dim, 400) + label(t.text, 84, t.text.length > 5 ? 30 : 36, t.over ? C.red : C.text) + label(word, 122, 15, C.dim, 400), { bg: t.running ? "#1d3a26" : C.bg }) };
    }
    case "rundown": {
      const r = st.rundown;
      if (!r) return { image: svg(label("Rundown", 66, 20, C.dim) + label("no segments", 96, 14, C.amber, 400)) };
      const btn = s.button || "next";
      if (btn === "next" || btn === "start") {
        if (!r.number) return { image: svg(label("START", 60, 26) + label(fit(r.next || "", 12), 96, 16, C.dim, 400), { bg: "#1d3a26" }) };
        return {
          image: svg(label(fit(r.name, 11), 34, 16, C.dim, 400) + label(r.left, 78, r.left.length > 5 ? 30 : 36, r.over ? C.red : C.text) + label(r.next ? `▶ ${fit(r.next, 10)}` : "last", 118, 15, C.text, 400), {
            border: r.over ? C.red : null,
          }),
        };
      }
      const words = { previous: "◀ BACK", stop: "■ STOP" };
      return { image: svg(label(words[btn] || btn, 72, 22, r.number ? C.text : C.dim) + label(r.number ? `${r.number}/${r.count}` : "", 104, 15, C.dim, 400)) };
    }
    case "guest": {
      const g = (st.guests || [])[indexIn(st.guests, s.guest)];
      if (!g) return { image: svg(label("Guest", 66, 22, C.dim) + label("no guest spot", 96, 14, C.amber, 400)) };
      const src = st.sources[g.source - 1];
      const tally = src ? src.tally : "off";
      const border = tally === "program" ? C.red : tally === "preview" ? C.green : null;
      const status = g.connected ? fit(g.guest_name || "on", 12) : g.connecting ? "joining…" : "waiting";
      const dot = `<circle cx="72" cy="48" r="14" fill="${g.connected ? C.green : g.connecting ? C.amber : C.raised}"/>`;
      return { image: svg(dot + label(fit(g.name, 11), 92, 19) + label(status, 120, 15, g.connected ? C.text : C.dim, 400), { border }), tally };
    }
    case "ftb":
      return st.black
        ? { image: svg(label("BLACK", 66, 30, C.red) + label("fade back up", 98, 15, C.text, 400), { bg: "#000000", border: C.red }) }
        : { image: svg(label("FTB", 70, 34) + label("fade to black", 100, 15, C.dim, 400)) };
    case "replay": {
      if (st.replaying) return { image: svg(label("REPLAY", 66, 28) + label("press to stop", 98, 15, C.text, 400), { bg: C.red }) };
      const c = st.recording ? C.text : C.dim;
      const arrow = `<path d="M96 44 a26 26 0 1 0 6 26" fill="none" stroke="${c}" stroke-width="8" stroke-linecap="round"/><polygon points="88,28 112,40 92,56" fill="${c}"/>`;
      return { image: svg(arrow + label(s.slow ? "SLOW-MO" : "REPLAY", 112, 18, c) + label(st.recording ? "" : "not recording", 130, 13, C.dim, 400)) };
    }
    case "macro": {
      const m = (st.macros || [])[indexIn(st.macros, s.macro)];
      if (!m) return { image: svg(label("Macro", 66, 22, C.dim) + label("no macro", 96, 14, C.amber, 400)) };
      const color = /^#[0-9a-f]{6}$/i.test(m.color || "") ? m.color : C.accent;
      return { image: svg(`<rect x="10" y="112" width="124" height="10" rx="5" fill="${color}"/>` + label(fit(m.name, 10), 66, 22) + label("macro", 94, 14, C.dim, 400), { border: color }) };
    }
    case "ptz": {
      const cam = ptzOf(s.camera);
      if (!cam) return { image: svg(label("PTZ", 66, 24, C.dim) + label("no PTZ camera", 96, 14, C.amber, 400)) };
      const src = st.sources[cam.source - 1];
      const tally = src ? src.tally : "off";
      const border = tally === "program" ? C.red : tally === "preview" ? C.green : null;
      const button = s.button || "preset";
      const n = Math.max(1, Math.round(Number(s.preset) || 1));
      const words = { preset: cam.presets[n - 1] || `Preset ${n}`, home: "⌂ home", left: "◀ left", right: "right ▶", up: "▲ up", down: "▼ down", in: "zoom in", out: "zoom out" };
      return { image: svg(label(fit(cam.name, 11), 50, 16, C.dim, 400) + label(fit(words[button] || button, 10), 86, 22), { border }), tally };
    }
    case "prompter": {
      const pr = st.prompter;
      if (!pr) return { image: svg(label("Prompter", 66, 20, C.dim) + label("no script", 96, 14, C.amber, 400)) };
      const button = s.button || "toggle";
      if (button === "toggle") {
        return { image: svg(label(pr.playing ? "ROLLING" : "PAUSED", 70, 24, pr.playing ? "#111111" : C.text) + label(`${Math.round(pr.words_per_minute)} wpm`, 100, 16, pr.playing ? "#111111" : C.dim, 400), { bg: pr.playing ? C.green : C.bg }) };
      }
      const words = { faster: "FASTER", slower: "SLOWER", top: "⏮ TOP", back: "◀ BACK", forward: "AHEAD ▶" };
      return { image: svg(label(words[button] || button, 70, 22) + label(`${Math.round(pr.words_per_minute)} wpm`, 100, 15, C.dim, 400)) };
    }
    case "comment": {
      const cm = st.comments;
      if (!cm) return { image: svg(label("Comments", 66, 19, C.dim) + label("chat off", 96, 14, C.amber, 400)) };
      const button = s.button || "toggle";
      const hide = button === "hide" || (button === "toggle" && cm.showing);
      const waiting = cm.waiting ? `${cm.waiting} waiting` : "none waiting";
      return { image: svg(label(hide ? "HIDE" : "SHOW", 66, 28, cm.showing ? "#111111" : C.text) + label(hide ? "comment" : waiting, 98, 15, cm.showing ? "#111111" : C.dim, 400), { bg: cm.showing ? C.amber : C.bg }) };
    }
    default:
      return { image: svg(label("Cutaway", 80, 20, C.dim)) };
  }
}

function holdBar(f) {
  return f > 0 ? `<rect x="14" y="132" width="${Math.round(116 * f)}" height="6" rx="3" fill="#ffffff"/>` : "";
}

/** Cutaway's fader curve (see control::fader_db), as 0..1. */
function faderPosition(db) {
  let v;
  if (db <= -40) v = 0;
  else if (db <= 0) v = ((db + 40) * 100) / 40;
  else v = 100 + (db * 27) / 12;
  return Math.max(0, Math.min(1, v / 127));
}

function render(ctx, force) {
  const k = keys.get(ctx);
  if (!k) return;
  const out = draw(k);
  if (out.tally !== undefined && out.tally !== k.tally) {
    // Tally changed: fetch a fresh picture now (its border shows the colour).
    k.tally = out.tally;
    k.fetched = 0;
  }
  if (k.controller === "Encoder") {
    const fb = JSON.stringify(out.feedback || {});
    if (force || fb !== k.feedback) {
      k.feedback = fb;
      sdSend({ event: "setFeedback", context: ctx, payload: out.feedback || {} });
    }
    return;
  }
  if (force || out.image !== k.image) {
    k.image = out.image;
    sdSend({ event: "setImage", context: ctx, payload: { image: out.image, target: 0 } });
  }
}

function renderAll(force) {
  for (const ctx of keys.keys()) render(ctx, force);
}

// ---- live pictures --------------------------------------------------------------------

function thumbsOn() {
  return String(cutaway.global.thumbnails) !== "off";
}

function thumbEvery() {
  const n = Number(cutaway.global.thumbnails);
  return Number.isFinite(n) && n >= 100 ? n : 1000;
}

function fetchPng(path) {
  const { host, port } = hostPort();
  const pin = String(cutaway.global.pin || "").trim();
  return new Promise((resolve, reject) => {
    const req = http.get({ host, port, path, headers: pin ? { "x-cutaway-pin": pin } : {}, timeout: 3000 }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => (res.statusCode === 200 ? resolve(Buffer.concat(chunks)) : reject(new Error(`HTTP ${res.statusCode}`))));
    });
    req.on("timeout", () => req.destroy(new Error("timed out")));
    req.on("error", reject);
  });
}

setInterval(() => {
  if (!cutaway.connected || !thumbsOn() || !cutaway.state) return;
  const now = Date.now();
  for (const [ctx, k] of keys) {
    if ((k.kind !== "scene" && k.kind !== "source") || k.controller !== "Keypad" || k.fetching) continue;
    if (now - (k.fetched || 0) < thumbEvery()) continue;
    const list = k.kind === "scene" ? cutaway.state.scenes : cutaway.state.sources;
    const i = indexIn(list, k.kind === "scene" ? k.settings.scene : k.settings.source);
    if (!list[i]) continue;
    k.fetching = true;
    k.fetched = now;
    fetchPng(`/api/key/${k.kind}/${i + 1}.png?size=144`)
      .then((png) => {
        k.png = `data:image/png;base64,${png.toString("base64")}`;
        render(ctx);
      })
      .catch(() => {
        k.png = null;
      })
      .finally(() => {
        k.fetching = false;
      });
  }
}, 100);

// Clocks and long presses move on their own.
setInterval(() => {
  for (const [ctx, k] of keys) {
    if (k.hold || ((k.kind === "record" || k.kind === "live") && cutaway.state)) render(ctx);
  }
}, 250);

// ---- the settings panel -------------------------------------------------------------------

function sendLists(ctx, action) {
  const st = cutaway.state;
  const { host, port } = hostPort();
  sdSend({
    event: "sendToPropertyInspector",
    action,
    context: ctx,
    payload: {
      kind: "lists",
      connected: cutaway.connected,
      status: cutaway.connected ? `Connected to Cutaway ${(st && st.version) || ""} at ${host}:${port}` : `Not connected: ${cutaway.problem}`,
      global: cutaway.global,
      scenes: st ? st.scenes.map((x) => x.name) : [],
      sources: st ? st.sources.map((x) => x.name) : [],
      graphics: st ? st.graphics.map((x) => x.name) : [],
      inputs: st ? st.inputs.map((x) => x.name) : [],
      players: st ? st.players.map((x) => ({ name: x.name, source: x.source })) : [],
      pads: st ? (st.pads || []).map((x) => x.name) : [],
      syrinscape: st ? (st.syrinscape || []).map((x) => x.name) : [],
      clocks: st ? (st.clocks || []).filter((x) => x.timer).map((x) => x.name) : [],
      guests: st ? (st.guests || []).map((x) => x.name) : [],
      macros: st ? (st.macros || []).map((x) => x.name) : [],
      ptz: st ? (st.ptz || []).map((x) => x.name) : [],
    },
  });
}

connect();
