const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");

const source = fs.readFileSync("assets/webcodecs-engine.js", "utf8");

test("native probe sums every GIF frame", async () => {
  let decodedFrames = 0;
  class FakeImageDecoder {
    constructor() {
      this.tracks = {
        ready: Promise.resolve(),
        selectedTrack: { frameCount: 1000 },
      };
    }
    async decode() {
      decodedFrames += 1;
      return { image: { duration: 1000, close() {} } };
    }
  }
  global.window = global;
  global.ImageDecoder = FakeImageDecoder;

  (0, eval)(source);
  const result = await global.stickerNativeProbe(
    "long.gif",
    new Uint8Array([0]).buffer,
  );

  assert.equal(decodedFrames, 1000);
  assert.ok(Math.abs(result.duration - 1) < 1e-9);
});

test("native probe sums APNG frame delays and flags apng", async () => {
  const delays = [1e6, 3e6, 0.5e6];
  class FakeImageDecoder {
    constructor() {
      this.tracks = {
        ready: Promise.resolve(),
        selectedTrack: { frameCount: delays.length },
      };
    }
    async decode({ frameIndex }) {
      return { image: { duration: delays[frameIndex], close() {} } };
    }
  }
  global.window = global;
  global.ImageDecoder = FakeImageDecoder;

  (0, eval)(source);
  const result = await global.stickerNativeProbe("anim.apng", new Uint8Array([0]).buffer);

  assert.equal(result.apng, true);
  assert.ok(Math.abs(result.duration - 4.5) < 1e-9);
});

test("native probe leaves static png duration at 0", async () => {
  class FakeImageDecoder {
    constructor() {
      this.tracks = { ready: Promise.resolve(), selectedTrack: { frameCount: 1 } };
    }
    async decode() {
      throw new Error("static png must not decode frames");
    }
  }
  global.window = global;
  global.ImageDecoder = FakeImageDecoder;

  (0, eval)(source);
  const result = await global.stickerNativeProbe("still.png", new Uint8Array([0]).buffer);

  assert.deepEqual(result, { duration: 0, apng: false });
});

test("compatibility assets include the ffmpeg glue", async () => {
  const requested = [];
  global.window = global;
  global.location = { href: "http://localhost/index.html" };
  global.fetch = async (url, options) => {
    requested.push({ url, method: options.method });
    return new Response(null, {
      status: 200,
      headers: { "Content-Length": "1", "Content-Type": "text/javascript" },
    });
  };

  (0, eval)(source);
  const result = await global.stickerCompatAssets();

  assert.deepEqual(result.ffmpegMissing, []);
  assert.ok(requested.some(({ url }) => url.endsWith("/ffmpeg-engine.js")));
});

test("image transcode upscales small inputs, handles 0, and keeps odd dimensions", async () => {
  let canvasSize = null;
  let imgDims = { w: 200, h: 100 };
  global.window = global;
  global.ImageDecoder = class {
    constructor() { this.tracks = { ready: Promise.resolve() }; }
    async decode() {
      return { image: { displayWidth: imgDims.w, displayHeight: imgDims.h, close() {} } };
    }
  };
  global.OffscreenCanvas = class {
    constructor(w, h) { canvasSize = { w, h }; }
    getContext() { return { drawImage() {} }; }
    async convertToBlob() { return { async arrayBuffer() { return new Uint8Array([1]).buffer; } }; }
  };

  (0, eval)(source);
  const bytes = new Uint8Array([1]);

  await global.stickerWebcodecsTranscode(bytes, "small.png", 0, 0, "image", 512, 512, () => {});
  assert.deepEqual(canvasSize, { w: 512, h: 256 }, "small image must upscale to fit 512x512");

  await global.stickerWebcodecsTranscode(bytes, "w.png", 0, 0, "image", 512, 0, () => {});
  assert.deepEqual(canvasSize, { w: 512, h: 256 }, "width-only scaling");

  await global.stickerWebcodecsTranscode(bytes, "h.png", 0, 0, "image", 0, 512, () => {});
  assert.deepEqual(canvasSize, { w: 1024, h: 512 }, "height-only scaling");

  imgDims = { w: 101, h: 73 };
  await global.stickerWebcodecsTranscode(bytes, "odd.png", 0, 0, "image", 0, 0, () => {});
  assert.deepEqual(canvasSize, { w: 101, h: 73 }, "0x0 image keeps exact odd dimensions");
});
