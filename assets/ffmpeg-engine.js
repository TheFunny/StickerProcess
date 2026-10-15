// StickerProcess web engine glue (Route A: ffmpeg.wasm ST core 0.12.10).
// Loaded by src/transcoder/web.rs via injected <script>; all functions on window.
// Contract: stickerFfmpegReady / stickerFfmpegTranscode / stickerFfmpegCancel
//（探测与下载统一在 webcodecs-engine.js：stickerNativeProbe / stickerDownload）
(async function () {
  // 自举：确保 UMD wrapper（定义 FFmpegWASM 全局）已加载——桥接只注入本文件，
  // wrapper 必须由这里兜底加载（classic worker chunk 814.ffmpeg.js 需与其同目录）。
  if (typeof FFmpegWASM === "undefined") {
    await new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = "ffmpeg.js";
      s.onload = resolve;
      s.onerror = () => reject(new Error("failed to load /ffmpeg.js"));
      document.head.appendChild(s);
    });
  }
  const { FFmpeg } = FFmpegWASM;

  // 单例：core abort 会毒化实例，cancel/abort 后置 null 由下次调用重载。
  // ffmpeg 只在 load 成功后赋值——load 失败不毒化单例，下次调用可重试。
  let ffmpeg = null;
  let loading = null;
  // 核心下载的中止控制器：取消与总超时都 abort 它，挂起的 fetch 随之 reject。
  let loadAbort = null;
  // ponytail: 整段下载 300s 总时长上限，不做停滞检测；慢网误杀时再加大或改 reader 停滞计时。
  const CORE_TIMEOUT_MS = 300000;

  async function toBlobURL(url, mime, signal) {
    const b = await (await fetch(url, { signal })).blob();
    return URL.createObjectURL(new Blob([b], { type: mime }));
  }

  // 流式下载并回报 0..1 进度：32MB core 首载要几秒，进度条不该死在 0。
  // 无 Content-Length/无 ReadableStream 时退化为整块 blob + 一次 100%。
  async function fetchBlobProgress(url, onProgress, signal) {
    const res = await fetch(url, { signal });
    const total = Number(res.headers.get("Content-Length")) || 0;
    if (!res.body || !total) {
      const b = await res.blob();
      onProgress(1);
      return b;
    }
    const reader = res.body.getReader();
    const chunks = [];
    let got = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      got += value.length;
      onProgress(Math.min(got / total, 1));
    }
    return new Blob(chunks, { type: "application/wasm" });
  }

  // 核心 blob URL 跨取消保留：cancel 只杀 worker，重建时不再重下 29.5MB。
  // ponytail: 代价是 core Blob 常驻内存到页面关闭；内存吃紧时改回 load 后 revoke。
  let coreUrls = null;

  function loadCore(onCoreProgress) {
    if (ffmpeg) return Promise.resolve(ffmpeg);
    if (!loading) {
      const ctl = new AbortController();
      loadAbort = ctl;
      const timer = setTimeout(() => ctl.abort(new Error("core download timed out")), CORE_TIMEOUT_MS);
      const p = (async () => {
        const base = location.href.replace(/\/[^/]*$/, "");
        const noop = () => {};
        const progress = onCoreProgress || noop;
        const { signal } = ctl;
        const fresh = [];
        try {
          if (!coreUrls) {
            fresh.push(await toBlobURL(`${base}/ffmpeg-core-st.js`, "text/javascript", signal));
            fresh.push(await fetchBlobProgress(`${base}/ffmpeg-core-st.wasm`, progress, signal)
              .then((b) => URL.createObjectURL(b)));
            coreUrls = fresh;
          }
          const ff = new FFmpeg();
          // ST core (no pthreads): no COOP/COEP required.
          await ff.load({ coreURL: coreUrls[0], wasmURL: coreUrls[1] });
          ffmpeg = ff;
          return ff;
        } catch (error) {
          // 加载失败视为核心可能损坏：撤销全部 URL 并丢弃缓存，下次从头下。
          // coreUrls 一旦赋值即包含 fresh，去重后每个 URL 只撤销一次
          //（wasm 下载中途挂掉时 coreUrls 还是 null，此时才靠 fresh 兜底）。
          for (const url of coreUrls ?? fresh) URL.revokeObjectURL(url);
          coreUrls = null;
          throw error;
        } finally {
          clearTimeout(timer);
        }
      })();
      loading = p;
      // 只清自己：cancel 后若已开始新一轮加载，旧 promise 的 catch 不能抹掉它
      p.catch(() => { if (loading === p) loading = null; });
    }
    return loading;
  }

  window.stickerFfmpegReady = async (onCoreProgress) => { await loadCore(onCoreProgress); return true; };

  window.stickerFfmpegTranscode = async (data, name, bitrate, fps, pixFmt, decoder, onProgress) => {
    const ff = await loadCore();
    // wasm 传入的 data 背靠 wasm 内存，不可 detach——ffmpeg.wasm writeFile 要
    // transfer 所有权，必须先拷到独立 ArrayBuffer
    await ff.writeFile(name, new Uint8Array(data).slice());
    // 单例跨任务复用：exec 结束必须 off，否则监听器随任务数累积，
    // 旧任务的 onProgress 持续向已完行发幽灵进度。
    const handler = ({ progress }) => onProgress(Math.min(progress, 1));
    ff.on("progress", handler);
    // 参数镜像 src/transcoder/command.rs::gen_command 视频分支（码率/crf/pix_fmt
    // 由 Rust 传入）；两处 web 专属偏离：ST core 无 pthreads 省 -row-mt；
    // deadline/cpu-used 提速——wasm 单线程 VP9 默认 good 档太慢（GIF exec ~10s），
    // realtime+4 约 3×，b:v 约束下画质损失有限（超 256KB 有 runner 收缩重试兜底）。
    // 输入侧解码器（webm VP9 alpha 需 libvpx-vp9）须在 -i 之前；"" = 默认解码。
    const args = [
      ...(decoder ? ["-c:v", decoder] : []),
      "-i", name,
      "-vf", "scale=512:512:force_original_aspect_ratio=decrease:flags=lanczos",
    ];
    if (fps > 0) args.push("-r", String(fps));
    args.push(
      "-an",
      "-c:v", "libvpx-vp9",
      "-deadline", "realtime",
      "-cpu-used", "4",
      "-pix_fmt", pixFmt,
      "-crf", "26",
      "-b:v", String(bitrate),
      "-bufsize", String(Math.floor(bitrate * 1.5)),
      "-f", "webm", "out.webm",
    );
    let out;
    try {
      const code = await ff.exec(args);
      if (code !== 0) throw new Error("ffmpeg exited " + code);
      out = await ff.readFile("out.webm");
    } finally {
      ff.off("progress", handler);
      await ff.deleteFile(name).catch(() => {});
      await ff.deleteFile("out.webm").catch(() => {});
    }
    return out;
  };

  window.stickerFfmpegCancel = () => {
    if (loadAbort) loadAbort.abort(new Error("cancelled"));
    if (ffmpeg) ffmpeg.terminate();
    ffmpeg = null;
    loading = null;
  };
})();
