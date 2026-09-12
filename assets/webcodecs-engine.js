// StickerProcess web engine glue (Route B: WebCodecs + webm-muxer).
// Loaded by src/transcoder/web.rs via injected <script>; all functions on window.
// Contract: stickerWebcodecsProbeSupport / stickerWebcodecsTranscode /
//           stickerWebcodecsCancel / stickerNativeProbe
// 管线移植自 wasm-demo/webcodecs-demo：解码（<video>+rVFC / ImageDecoder）→
// OffscreenCanvas 512 fit → VideoEncoder(vp9 8-bit) → webm-muxer。
// 码率/factor 全由 Rust 侧算好传入（与桌面 command.rs 一致，JS 不重复实现）。
(() => {
  let cancelRequested = false;

  const MIME_BY_EXT = {
    jpg: "image/jpeg", jpeg: "image/jpeg", png: "image/png",
    webp: "image/webp", gif: "image/gif",
  };
  const extOf = (name) => {
    const m = /\.([a-z0-9]+)$/i.exec(name);
    return m ? m[1].toLowerCase() : "";
  };
  function fitDims(w, h) {
    const s = Math.min(512 / w, 512 / h, 1);
    const even = (n) => Math.max(2, Math.floor((n * s) / 2) * 2);
    return { width: even(w), height: even(h) };
  };
  // 超时兜底：无 H.264 解码器的浏览器里 <video> 不报错而是永久挂起
  // （networkState=2 无事件）；缺它任务会永远卡 Probing。
  const withTimeout = (promise, ms, msg) =>
    Promise.race([
      promise,
      new Promise((_, rej) => setTimeout(() => rej(new Error(msg)), ms)),
    ]);

  // ---- 统一原生探测（GIF/APNG/MP4/图片）：不加载任何引擎、不依赖 VideoEncoder ----
  // 返回 { duration: 秒, apng: bool }。0/false 表示未知/否。
  // GIF 用 ImageDecoder 累加帧时长；PNG 靠 frameCount>1 判 APNG；MP4 走 <video> 元数据。
  window.stickerNativeProbe = async (name, data) => {
    try {
      const ext = extOf(name);
      if (ext === "gif") {
        const d = new ImageDecoder({ data, type: "image/gif" });
        await d.tracks.ready;
        const t = d.tracks.selectedTrack;
        let total = 0;
        for (let i = 0; i < Math.min(t.frameCount, 500); i++) {
          const { image } = await d.decode({ frameIndex: i });
          total += (image.duration ?? 100000) / 1e6;
          image.close();
        }
        return { duration: total, apng: false };
      }
      if (ext === "png") {
        const d = new ImageDecoder({ data, type: "image/png" });
        await d.tracks.ready;
        const n = d.tracks.selectedTrack.frameCount;
        return { duration: 0, apng: n > 1 };
      }
      if (ext === "mp4") {
        const url = URL.createObjectURL(new Blob([data], { type: "video/mp4" }));
        try {
          const v = document.createElement("video");
          v.preload = "metadata";
          v.src = url;
          await withTimeout(
            new Promise((res, rej) => {
              v.onloadedmetadata = res;
              v.onerror = () => rej(new Error("metadata failed"));
            }),
            10000,
            "mp4 metadata timeout"
          );
          return { duration: isFinite(v.duration) ? v.duration : 0, apng: false };
        } finally {
          URL.revokeObjectURL(url);
        }
      }
      return { duration: 0, apng: false };
    } catch {
      return { duration: 0, apng: false };
    }
  };

  // ---- capability probe：B 引擎是否可用（Chromium 系 vp9 软编）----
  window.stickerWebcodecsProbeSupport = async () => {
    if (typeof VideoEncoder === "undefined" || typeof WebMMuxer === "undefined")
      return false;
    const r = await VideoEncoder.isConfigSupported({
      codec: "vp09.00.10.08", width: 512, height: 512, bitrate: 500000,
    }).catch(() => null);
    return !!(r && r.supported);
  };

  // ---- 图片 → PNG：解码首帧 → canvas 512 fit → PNG 字节（无损）----
  async function transcodeImage(data, name, onProgress) {
    const mime = MIME_BY_EXT[extOf(name)] || "image/png";
    const decoder = new ImageDecoder({ data, type: mime });
    await decoder.tracks.ready;
    const { image } = await decoder.decode({ frameIndex: 0 });
    const dims = fitDims(image.displayWidth, image.displayHeight);
    const c = new OffscreenCanvas(dims.width, dims.height);
    const ctx = c.getContext("2d");
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(image, 0, 0, dims.width, dims.height);
    image.close();
    onProgress(0.9);
    const blob = await c.convertToBlob({ type: "image/png" });
    const out = new Uint8Array(await blob.arrayBuffer());
    onProgress(1);
    return out;
  }

  // ---- 视频（仅 MP4）→ VP9 webm（无 alpha；GIF/APNG 归 A 引擎）----
  async function transcodeVideo(data, name, bitrate, fps, onProgress) {
    if (typeof WebMMuxer === "undefined") throw new Error("webm-muxer.js not loaded");
    if (cancelRequested) throw new Error("cancelled");
    const url = URL.createObjectURL(new Blob([data], { type: "video/mp4" }));
    const v = document.createElement("video");
    v.src = url;
    v.muted = true;
    v.playsInline = true;
    try {
      await withTimeout(
        new Promise((res, rej) => {
          v.onloadedmetadata = res;
          v.onerror = () => rej(new Error("mp4 open failed"));
        }),
        10000,
        "mp4 metadata timeout"
      );
      const dims = fitDims(v.videoWidth, v.videoHeight);
      const frameRate = fps > 0 ? fps : 30;

      const muxer = new WebMMuxer.Muxer({
        target: new WebMMuxer.ArrayBufferTarget(),
        video: { codec: "V_VP9", width: dims.width, height: dims.height, frameRate },
        firstTimestampBehavior: "offset",
      });
      let encodeError = null;
      const encoder = new VideoEncoder({
        output: (chunk, meta) => muxer.addVideoChunk(chunk, meta),
        error: (e) => { encodeError = e; },
      });
      encoder.configure({
        codec: "vp09.00.10.08",
        width: dims.width, height: dims.height,
        bitrate, framerate: frameRate,
        latencyMode: "quality",
        // ponytail: alpha:'keep' 真机不支持；MP4 无透明需求，GIF/APNG 走 A 引擎
      });

      let frameNo = 0;
      let lastUs = -1;
      const durUs = Math.round((v.duration || 0) * 1e6);
      const gop = Math.max(1, Math.round(frameRate * 2));

      // rVFC 播放抓帧（demo 实测可靠）：每个唯一媒体时间取一帧 → 缩放 → 编码。
      // 整体超时（≥10× 实时）：中途解码 stall 不让任务永远卡 Processing。
      const playMs = 30000 + Math.ceil(v.duration || 10) * 20000;
      await withTimeout(
        new Promise((res, rej) => {
        let settled = false;
        const fail = (e) => { if (!settled) { settled = true; rej(e); } };
        const grab = async (_now, meta) => {
          if (settled) return;
          try {
            if (cancelRequested) return fail(new Error("cancelled"));
            if (encodeError) return fail(encodeError);
            const tsUs = Math.round(meta.mediaTime * 1e6);
            if (tsUs > lastUs) {
              lastUs = tsUs;
              const bmp = await createImageBitmap(v);
              const c = new OffscreenCanvas(dims.width, dims.height);
              const ctx = c.getContext("2d");
              ctx.imageSmoothingEnabled = true;
              ctx.imageSmoothingQuality = "high";
              ctx.drawImage(bmp, 0, 0, dims.width, dims.height);
              bmp.close();
              const vf = new VideoFrame(c, {
                timestamp: tsUs,
                duration: Math.round(1e6 / frameRate),
              });
              encoder.encode(vf, { keyFrame: frameNo % gop === 0 });
              vf.close();
              frameNo++;
              if (durUs > 0) onProgress(Math.min(tsUs / durUs, 0.99));
            }
            v.requestVideoFrameCallback(grab);
          } catch (e) { fail(e); }
        };
        v.onended = () => { if (!settled) { settled = true; res(); } };
        v.onerror = () => fail(new Error("video playback failed"));
        v.requestVideoFrameCallback(grab);
        v.play().catch((e) => fail(e));
        }),
        playMs,
        "mp4 playback stalled (watchdog)"
      );

      await encoder.flush();
      encoder.close();
      muxer.finalize();
      onProgress(1);
      return new Uint8Array(muxer.target.buffer);
    } finally {
      URL.revokeObjectURL(url);
      v.removeAttribute("src");
      v.load();
    }
  }

  // kind: "video" | "image"（Rust 按媒体类型决定；video 仅 MP4）
  window.stickerWebcodecsTranscode = async (data, name, bitrate, fps, kind, onProgress) => {
    cancelRequested = false;
    // wasm 内存 buffer 不可 transfer——ImageDecoder/Blob 会拷贝，仍统一 slice 兜底
    const buf = new Uint8Array(data).slice();
    if (kind === "image") return transcodeImage(buf, name, onProgress);
    return transcodeVideo(buf, name, bitrate, fps, onProgress);
  };

  window.stickerWebcodecsCancel = () => { cancelRequested = true; };
})();
