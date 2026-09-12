// StickerProcess web engine glue (Route A: ffmpeg.wasm ST core 0.12.10).
// Loaded by src/transcoder/web.rs via injected <script>; all functions on window.
// Contract: stickerFfmpegReady / stickerFfmpegTranscode / stickerFfmpegProbe /
//           stickerFfmpegCancel / stickerDownload
(async function () {
  // 自举：确保 UMD wrapper（定义 FFmpegWASM 全局）已加载——桥接只注入本文件，
  // wrapper 必须由这里兜底加载（classic worker chunk 814.ffmpeg.js 需与其同目录）。
  if (typeof FFmpegWASM === "undefined") {
    await new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = "/ffmpeg.js";
      s.onload = resolve;
      s.onerror = () => reject(new Error("failed to load /ffmpeg.js"));
      document.head.appendChild(s);
    });
  }
  const { FFmpeg } = FFmpegWASM;

  // 单例：core abort 会毒化实例，cancel/abort 后置 null 由下次调用重载
  let ffmpeg = null;

  async function toBlobURL(url, mime) {
    const b = await (await fetch(url)).blob();
    return URL.createObjectURL(new Blob([b], { type: mime }));
  }

  async function loadCore() {
    if (ffmpeg) return ffmpeg;
    const base = location.href.replace(/\/[^/]*$/, "");
    ffmpeg = new FFmpeg();
    await ffmpeg.load({
      // ST core (no pthreads): no COOP/COEP required.
      coreURL: await toBlobURL(`${base}/ffmpeg-core-st.js`, "text/javascript"),
      wasmURL: await toBlobURL(`${base}/ffmpeg-core-st.wasm`, "application/wasm"),
    });
    return ffmpeg;
  }

  window.stickerFfmpegReady = async () => { await loadCore(); return true; };

  window.stickerFfmpegTranscode = async (data, name, bitrate, fps, onProgress) => {
    const ff = await loadCore();
    // wasm 传入的 data 背靠 wasm 内存，不可 detach——ffmpeg.wasm writeFile 要
    // transfer 所有权，必须先拷到独立 ArrayBuffer
    await ff.writeFile(name, new Uint8Array(data).slice());
    ff.on("progress", ({ progress }) => onProgress(Math.min(progress, 1)));
    // 参数镜像 src/transcoder/command.rs::gen_command 视频分支：
    // ST core 无 pthreads，-row-mt 无意义；GIF/APNG 恒 yuva420p（W1 仅接这两类）
    const args = [
      "-i", name,
      "-vf", "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos",
    ];
    if (fps > 0) args.push("-r", String(fps));
    args.push(
      "-an",
      "-c:v", "libvpx-vp9",
      "-pix_fmt", "yuva420p",
      "-crf", "26",
      "-b:v", String(bitrate),
      "-bufsize", String(Math.floor(bitrate * 1.5)),
      "-f", "webm", "out.webm",
    );
    const code = await ff.exec(args);
    if (code !== 0) throw new Error("ffmpeg exited " + code);
    const out = await ff.readFile("out.webm");
    await ff.deleteFile(name).catch(() => {});
    await ff.deleteFile("out.webm").catch(() => {});
    return out;
  };

  window.stickerFfmpegCancel = () => {
    if (ffmpeg) { ffmpeg.terminate(); ffmpeg = null; }
  };

  // W1 探测：GIF/APNG 用 ImageDecoder 求总时长（Σ frame.duration/1e6）；其余返回 0。
  // APNG 按 image/png 交给 ImageDecoder；frameCount 上限 500 防御异常文件。
  window.stickerFfmpegProbe = async (name, data) => {
    if (!/\.(gif|apng|png)$/i.test(name) || typeof ImageDecoder === "undefined") return 0;
    const dec = new ImageDecoder({
      data,
      type: name.toLowerCase().endsWith(".gif") ? "image/gif" : "image/png",
    });
    await dec.tracks.ready;
    await dec.completed;
    let total = 0;
    const track = dec.tracks.selectedTrack;
    for (let i = 0; i < Math.min(track.frameCount, 500); i++) {
      const { image } = await dec.decode({ frameIndex: i });
      total += (image.duration || 0) / 1e6;
      image.close();
    }
    return total;
  };

  window.stickerDownload = (data, filename) => {
    const url = URL.createObjectURL(new Blob([data]));
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
  };
})();
