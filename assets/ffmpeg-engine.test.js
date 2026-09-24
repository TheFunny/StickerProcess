const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");

test("cancel clears the poisoned singleton and core URLs", async () => {
  const instances = [];
  const revoked = [];
  let objectUrl = 0;
  let fetches = 0;

  class FakeFFmpeg {
    constructor() {
      this.terminated = false;
      instances.push(this);
    }
    async load({ coreURL, wasmURL }) {
      assert.match(coreURL, /^blob:/);
      assert.match(wasmURL, /^blob:/);
    }
    async writeFile() {
      if (this.terminated) throw new Error("terminated instance reused");
    }
    async exec() { return 0; }
    async readFile() { return new Uint8Array([1]); }
    on() {}
    off() {}
    async deleteFile() {}
    terminate() { this.terminated = true; }
  }

  global.window = global;
  global.location = { href: "http://localhost/index.html" };
  global.FFmpegWASM = { FFmpeg: FakeFFmpeg };
  global.fetch = async () => {
    fetches += 1;
    return new Response(new Uint8Array([1]), {
      status: 200,
      headers: { "Content-Length": "1" },
    });
  };
  URL.createObjectURL = () => `blob:test-${++objectUrl}`;
  URL.revokeObjectURL = (url) => revoked.push(url);

  (0, eval)(fs.readFileSync("assets/ffmpeg-engine.js", "utf8"));
  while (!global.stickerFfmpegReady) await new Promise((resolve) => setTimeout(resolve, 0));

  await global.stickerFfmpegReady(() => {});
  assert.deepEqual({ instances: instances.length, fetches, revoked: revoked.length }, {
    instances: 1,
    fetches: 2,
    revoked: 2,
  });

  global.stickerFfmpegCancel();
  const out = await global.stickerFfmpegTranscode(
    new Uint8Array([1]).buffer,
    "retry.gif",
    1000,
    0,
    "yuva420p",
    () => {},
  );

  assert.equal(out.byteLength, 1);
  assert.equal(instances.length, 2, "retry must create a fresh FFmpeg instance");
  assert.equal(instances[0].terminated, true);
  assert.equal(revoked.length, 4, "both Blob URLs are released for each load");
});
