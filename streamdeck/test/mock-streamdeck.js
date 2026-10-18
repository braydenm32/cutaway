// End-to-end test of the plugin against a running Cutaway, standing in for
// the Stream Deck app (which talks to plugins over a local WebSocket).
//
//   node test/mock-streamdeck.js 127.0.0.1:8790 [PIN]
//
// Cutaway must be running with remote control on. The test takes scene 2,
// starts and stops a recording, turns a dial and checks the key pictures.

"use strict";

const net = require("net");
const http = require("http");
const crypto = require("crypto");
const path = require("path");
const { spawn } = require("child_process");

const address = process.argv[2] || "127.0.0.1:8790";
const pin = process.argv[3] || "";
const [cHost, cPort] = address.split(":");

// ---- a minimal WebSocket server (one client) --------------------------------

function server(onClient) {
  const srv = net.createServer((sock) => {
    let buf = Buffer.alloc(0);
    let open = false;
    const client = {
      handlers: [],
      send(obj) {
        const data = Buffer.from(JSON.stringify(obj));
        let head;
        if (data.length < 126) head = Buffer.from([0x81, data.length]);
        else if (data.length < 65536) {
          head = Buffer.from([0x81, 126, 0, 0]);
          head.writeUInt16BE(data.length, 2);
        } else {
          head = Buffer.alloc(10);
          head[0] = 0x81;
          head[1] = 127;
          head.writeBigUInt64BE(BigInt(data.length), 2);
        }
        sock.write(Buffer.concat([head, data]));
      },
    };
    sock.on("data", (d) => {
      buf = Buffer.concat([buf, d]);
      if (!open) {
        const end = buf.indexOf("\r\n\r\n");
        if (end < 0) return;
        const head = buf.subarray(0, end).toString();
        buf = buf.subarray(end + 4);
        const key = head.match(/Sec-WebSocket-Key: (\S+)/i)[1];
        const accept = crypto.createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
        sock.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
        open = true;
        onClient(client);
      }
      for (;;) {
        if (buf.length < 2) return;
        let len = buf[1] & 0x7f;
        let at = 2;
        if (len === 126) {
          len = buf.readUInt16BE(2);
          at = 4;
        } else if (len === 127) {
          len = Number(buf.readBigUInt64BE(2));
          at = 10;
        }
        if (buf.length < at + 4 + len) return;
        const mask = buf.subarray(at, at + 4);
        const payload = Buffer.from(buf.subarray(at + 4, at + 4 + len));
        for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
        const opcode = buf[0] & 0x0f;
        buf = buf.subarray(at + 4 + len);
        if (opcode === 1) {
          const m = JSON.parse(payload.toString());
          client.handlers.forEach((h) => h(m));
        }
      }
    });
    sock.on("error", () => {});
  });
  return new Promise((resolve) => srv.listen(0, "127.0.0.1", () => resolve(srv)));
}

// ---- helpers --------------------------------------------------------------------

function state() {
  return new Promise((resolve, reject) => {
    http
      .get({ host: cHost, port: Number(cPort), path: "/api/state", headers: pin ? { "x-cutaway-pin": pin } : {} }, (res) => {
        let s = "";
        res.on("data", (c) => (s += c));
        res.on("end", () => {
          try {
            resolve(JSON.parse(s));
          } catch (e) {
            reject(new Error(`bad state (HTTP ${res.statusCode}): ${s.slice(0, 80)}`));
          }
        });
      })
      .on("error", reject);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(what, test, ms = 6000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await test();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}

let failures = 0;
function check(ok, what) {
  console.log(`${ok ? "ok  " : "FAIL"}  ${what}`);
  if (!ok) failures++;
}

// ---- the test ---------------------------------------------------------------------

(async () => {
  const received = [];
  let sd;
  const srv = await server((c) => {
    sd = c;
    c.handlers.push((m) => received.push(m));
  });
  const port = srv.address().port;
  const plugin = spawn(process.execPath, [path.join(__dirname, "..", "app.cutaway.streamdeck.sdPlugin", "bin", "plugin.js"), "-port", String(port), "-pluginUUID", "PLUGIN", "-registerEvent", "registerPlugin", "-info", "{}"], {
    stdio: ["ignore", "inherit", "inherit"],
  });
  const last = (event, ctx) => [...received].reverse().find((m) => m.event === event && (!ctx || m.context === ctx));
  try {
    await until("the plugin to register", () => received.find((m) => m.event === "registerPlugin" && m.uuid === "PLUGIN"));
    check(true, "plugin registers with the Stream Deck app");
    await until("a request for global settings", () => last("getGlobalSettings"));
    sd.send({ event: "didReceiveGlobalSettings", payload: { settings: { address, pin, thumbnails: "250" } } });

    const appear = (context, action, settings, controller = "Keypad") =>
      sd.send({ event: "willAppear", action: `app.cutaway.streamdeck.${action}`, context, device: "D", payload: { settings, controller, coordinates: { column: 0, row: 0 }, isInMultiAction: false } });
    const key = (event, context, action, extra = {}) => sd.send({ event, action: `app.cutaway.streamdeck.${action}`, context, device: "D", payload: { settings: {}, ...extra } });

    appear("k-scene", "scene", { scene: "2" });
    appear("k-source", "source", { source: "1" });
    appear("k-rec", "record", {});
    appear("k-dial", "audio", { input: "1", step: "1" }, "Encoder");
    appear("k-take", "take", { how: "cut" });

    // Live pictures with tally.
    const img = await until("a live picture on the scene key", () => {
      const m = last("setImage", "k-scene");
      return m && m.payload.image.startsWith("data:image/png;base64,") && m;
    });
    const png = Buffer.from(img.payload.image.split(",")[1], "base64");
    check(png.subarray(1, 4).toString() === "PNG" && png.length > 500, `scene key shows a live picture (${png.length} bytes)`);
    check(!!(await until("source key picture", () => last("setImage", "k-source"))), "source key has a picture");
    check(!!last("setImage", "k-take"), "take key has a picture");

    // Take scene 2.
    const before = await state();
    key("keyDown", "k-scene", "scene", { settings: { scene: "2" } });
    key("keyUp", "k-scene", "scene", { settings: { scene: "2" } });
    const s2 = await until("scene 2 on air", async () => {
      const s = await state();
      return s.scenes[1] && s.scenes[1].program && s;
    });
    check(s2.program === before.scenes[1].name, `pressing the scene key takes "${s2.program}"`);

    // Record: a press starts; a short press doesn't stop; a held press does.
    key("keyDown", "k-rec", "record");
    key("keyUp", "k-rec", "record");
    await until("recording", async () => (await state()).recording);
    check(true, "record key starts recording");
    await until("the record key to turn red", () => {
      const m = last("setImage", "k-rec");
      return m && decodeURIComponent(m.payload.image).includes("#e5303e") && decodeURIComponent(m.payload.image).includes("REC");
    });
    check(true, "record key shows recording, with the time");
    key("keyDown", "k-rec", "record");
    await sleep(200);
    key("keyUp", "k-rec", "record");
    await sleep(600);
    check((await state()).recording, "a short press doesn't stop the recording");
    check(decodeURIComponent((last("setImage", "k-rec") || { payload: { image: "" } }).payload.image).includes("Hold"), "the key says to hold");
    key("keyDown", "k-rec", "record");
    await sleep(1300);
    key("keyUp", "k-rec", "record");
    await until("recording to stop", async () => !(await state()).recording, 8000);
    check(true, "holding the key stops the recording");

    // Stream Deck+ dial: level.
    const g0 = (await state()).inputs[0].gain_db;
    sd.send({ event: "dialRotate", action: "app.cutaway.streamdeck.audio", context: "k-dial", device: "D", payload: { settings: { input: "1", step: "1" }, ticks: 3, pressed: false } });
    const g1 = await until("the level to change", async () => {
      const g = (await state()).inputs[0].gain_db;
      return Math.abs(g - (g0 + 3)) < 0.01 ? g : null;
    });
    check(true, `turning the dial 3 clicks: ${g0} dB -> ${g1} dB`);
    const fb = await until("dial feedback", () => {
      const m = last("setFeedback", "k-dial");
      return m && m.payload.value && m.payload.value.includes(`${g1 > 0 ? "+" : ""}${g1.toFixed(1)} dB`) && m;
    });
    check(typeof fb.payload.indicator.value === "number", `dial display: ${fb.payload.title} ${fb.payload.value}`);
    sd.send({ event: "dialRotate", action: "app.cutaway.streamdeck.audio", context: "k-dial", device: "D", payload: { settings: { input: "1", step: "1" }, ticks: -3, pressed: false } });
    await until("the level back", async () => Math.abs((await state()).inputs[0].gain_db - g0) < 0.01);

    // The settings panel gets Cutaway's names.
    sd.send({ event: "propertyInspectorDidAppear", action: "app.cutaway.streamdeck.scene", context: "k-scene", device: "D" });
    const lists = await until("lists for the settings panel", () => last("sendToPropertyInspector", "k-scene"));
    check(lists.payload.connected && lists.payload.scenes.length === s2.scenes.length, `settings panel lists ${lists.payload.scenes.length} scenes: ${lists.payload.status}`);

    // Save every key's picture, to look at (DUMP_DIR=folder).
    if (process.env.DUMP_DIR) {
      const fs = require("fs");
      appear("k-transition", "transition", { style: "mix" });
      appear("k-live", "live", {});
      appear("k-marker", "marker", {});
      appear("k-graphic", "graphic", { graphic: "1" });
      appear("k-audio", "audio", { input: "1" });
      appear("k-auto", "auto", {});
      appear("k-captions", "captions", {});
      appear("k-media", "media", { player: "1", button: "toggle" });
      appear("k-command", "command", { command: "preview:2", label: "Preview 2" });
      key("keyDown", "k-rec", "record");
      key("keyUp", "k-rec", "record");
      await sleep(2500);
      fs.mkdirSync(process.env.DUMP_DIR, { recursive: true });
      for (const ctx of ["k-scene", "k-source", "k-take", "k-transition", "k-rec", "k-live", "k-marker", "k-graphic", "k-audio", "k-auto", "k-captions", "k-media", "k-command"]) {
        const m = last("setImage", ctx);
        if (!m) continue;
        const [head, data] = m.payload.image.split(",");
        const svg = head.includes("svg");
        fs.writeFileSync(path.join(process.env.DUMP_DIR, `${ctx}.${svg ? "svg" : "png"}`), svg ? decodeURIComponent(data) : Buffer.from(data, "base64"));
      }
      key("keyDown", "k-rec", "record");
      await sleep(1300);
      key("keyUp", "k-rec", "record");
      await until("recording to stop", async () => !(await state()).recording, 8000);
    }

    // A wrong PIN is reported.
    if (pin) {
      sd.send({ event: "didReceiveGlobalSettings", payload: { settings: { address, pin: "0000", thumbnails: "250" } } });
      await sleep(1500);
      received.length = 0;
      sd.send({ event: "sendToPlugin", action: "app.cutaway.streamdeck.scene", context: "k-scene", payload: { kind: "lists" } });
      const l = await until("status", () => last("sendToPropertyInspector", "k-scene"));
      check(!l.payload.connected && /PIN/.test(l.payload.status), `wrong PIN: "${l.payload.status}"`);
      const off = await until("keys show not connected", () => {
        const m = last("setImage", "k-scene");
        return m && decodeURIComponent(m.payload.image).includes("not connected");
      });
      check(!!off, "keys show they're not connected");
    }
  } catch (e) {
    console.log(`FAIL  ${e.message}`);
    failures++;
  } finally {
    plugin.kill();
    srv.close();
  }
  console.log(failures ? `${failures} failed` : "all passed");
  process.exit(failures ? 1 : 0);
})();
