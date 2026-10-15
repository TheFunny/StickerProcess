const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");

const source = fs.readFileSync("assets/ffmpeg-engine.js", "utf8");

// 浏览器桩：返回可供断言的共享状态（实例/blob URL/撤销/下载次数）。
function fakeBrowser({ loadFails = 0 } = {}) {
  const state = { instances: [], objectUrls: [], revoked: [], fetches: 0 };
  let loads = 0;

  class FakeFFmpeg {
    constructor() {
      this.terminated = false;
      this.coreURL = null;
      this.wasmURL = null;
      state.instances.push(this);
    }
    async load({ coreURL, wasmURL }) {
      assert.match(coreURL, /^blob:/);
      assert.match(wasmURL, /^blob:/);
      if (loads++ < loadFails) throw new Error("core load failed");
      this.coreURL = coreURL;
      this.wasmURL = wasmURL;
    }
    async writeFile(name) {
      if (this.terminated) throw new Error("terminated instance reused");
      this.written = name;
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
    state.fetches += 1;
    return new Response(new Uint8Array([1]), {
      status: 200,
      headers: { "Content-Length": "1" },
    });
  };
  URL.createObjectURL = () => {
    const url = `blob:test-${state.objectUrls.length + 1}`;
    state.objectUrls.push(url);
    return url;
  };
  URL.revokeObjectURL = (url) => state.revoked.push(url);

  (0, eval)(source);
  return state;
}

test("cancel drops the poisoned instance but keeps the downloaded core", async () => {
  const s = fakeBrowser();
  while (!global.stickerFfmpegReady) await new Promise((resolve) => setTimeout(resolve, 0));

  await global.stickerFfmpegReady(() => {});
  assert.deepEqual(
    { instances: s.instances.length, fetches: s.fetches, objectUrls: s.objectUrls.length, revoked: s.revoked.length },
    { instances: 1, fetches: 2, objectUrls: 2, revoked: 0 },
    "first load downloads core js + wasm once and keeps both Blob URLs for reuse",
  );

  global.stickerFfmpegCancel();
  const out = await global.stickerFfmpegTranscode(
    new Uint8Array([1]).buffer,
    "retry.gif",
    1000,
    0,
    "yuva420p",
    "",
    () => {},
  );

  assert.equal(out.byteLength, 1);
  assert.equal(s.instances.length, 2, "retry must create a fresh FFmpeg instance");
  assert.equal(s.instances[0].terminated, true, "cancelled instance must be terminated");
  assert.equal(s.fetches, 2, "cached core must not be re-downloaded after cancel");
  assert.equal(s.objectUrls.length, 2, "cached core must not be re-encoded into new Blob URLs");
  assert.equal(s.instances[1].coreURL, s.instances[0].coreURL);
  assert.equal(s.instances[1].wasmURL, s.instances[0].wasmURL);
  assert.equal(s.revoked.length, 0, "core Blob URLs stay alive until a load failure poisons them");
});

test("a failed core load revokes both Blob URLs and re-downloads next time", async () => {
  const s = fakeBrowser({ loadFails: 1 });
  while (!global.stickerFfmpegReady) await new Promise((resolve) => setTimeout(resolve, 0));

  await assert.rejects(() => global.stickerFfmpegReady(() => {}), /core load failed/);
  assert.deepEqual(s.revoked, s.objectUrls, "a failed load must release its Blob URLs");

  await global.stickerFfmpegReady(() => {});
  assert.equal(s.fetches, 4, "recovery must re-download the discarded core");
  assert.equal(s.objectUrls.length, 4);
  assert.equal(s.instances.length, 2);
});
