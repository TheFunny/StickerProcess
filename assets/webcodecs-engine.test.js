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
