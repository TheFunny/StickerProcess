fn main() {
    // vcpkg 静态库用 /MT（LIBCMT），Rust debug 默认引 LIBCMTD —— 冲突告警抑制
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:LIBCMTD");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:LIBCMT");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:MSVCRTD");
    // ffmpeg 库目录解析顺序：FFMPEG_DIR 显式指定 → VCPKG_ROOT 推导
    // （installed/<triplet>，triplet 取 VCPKG_DEFAULT_TRIPLET 或默认
    // x64-windows-static——本工程静态链接目标）。两者都缺 → 明确报错。
    let ffmpeg_dir = match std::env::var("FFMPEG_DIR") {
        Ok(dir) => dir,
        Err(_) => {
            let triplet = std::env::var("VCPKG_DEFAULT_TRIPLET")
                .unwrap_or_else(|_| "x64-windows-static".into());
            let derived =
                std::env::var("VCPKG_ROOT").map(|root| format!("{root}/installed/{triplet}"));
            match derived {
                Ok(dir) if std::path::Path::new(&dir).join("lib").is_dir() => {
                    println!(
                        "cargo:warning=FFMPEG_DIR 未设置，使用 VCPKG_ROOT 推导的库目录（信息性提示）: {dir}"
                    );
                    dir
                }
                _ => panic!(
                    "找不到 ffmpeg 静态库：请设置 FFMPEG_DIR（含 lib/*.lib 的安装根目录，\
                     如 D:\\Tools\\vcpkg\\installed\\x64-windows-static）\
                     或 VCPKG_ROOT（vcpkg 根目录，自动推导 installed/<triplet>）。\
                     搭建方法见 docs/E6_INPROCESS_RESEARCH.md §7"
                ),
            }
        }
    };
    if !std::path::Path::new(&ffmpeg_dir).join("lib").is_dir() {
        panic!("FFMPEG_DIR 无效：{ffmpeg_dir} 下不存在 lib/ 目录（需要 avcodec.lib 等静态库）");
    }
    println!("cargo:rustc-link-search=native={ffmpeg_dir}/lib");

    // vfwcap（avdevice）依赖 avicap32，Windows SDK 不带其导入库。
    let out_dir = std::env::var("OUT_DIR").unwrap();
    generate_avicap32_import_lib(&out_dir);
    println!("cargo:rustc-link-search=native={out_dir}");

    // ffmpeg-sys 的 EXTRALIBS 透传只在 --features build 路径生效，
    // FFMPEG_DIR 路径需手动补齐 vpx 与 avdevice 的全部系统依赖。
    for lib in [
        "vpx", "strmiids", "mfuuid", "uuid", "winmm", "ws2_32", "secur32", "bcrypt", "user32",
        "avicap32", "msvfw32", "gdi32", "oleaut32", "shlwapi", "psapi", "ncrypt", "crypt32", "zs",
    ] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
}

/// 用 MSVC lib.exe 从 .def 生成 avicap32 导入库。
/// def 文件仅列出 vfwcap 实际引用的两个符号（capCreateCaptureWindowA /
/// capGetDriverDescriptionA），与 SDK 官方导入库语义一致。
fn generate_avicap32_import_lib(out_dir: &str) {
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("build/avicap32.def");
    let lib_path = std::path::Path::new(out_dir).join("avicap32.lib");
    if lib_path.exists() {
        return; // 已生成，跳过
    }
    // 定位 lib.exe：VSWHERE 找最新 VS 安装
    let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
    let vs_root = match std::process::Command::new(vswhere)
        .args(["-latest", "-property", "installationPath"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => panic!("vswhere failed: {e}"),
    };
    if vs_root.is_empty() {
        panic!("vswhere returned empty path: MSVC 2022 required for static ffmpeg build");
    }
    let msvc_root = std::path::Path::new(&vs_root).join("VC/Tools/MSVC");
    let toolset = std::fs::read_dir(&msvc_root)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .max()
        })
        .unwrap_or_else(|| panic!("no MSVC toolset found under {}", msvc_root.display()));
    let lib_exe = msvc_root.join(toolset).join("bin/Hostx64/x64/lib.exe");
    let status = std::process::Command::new(&lib_exe)
        .args([
            format!("/def:{}", manifest.display()),
            "/machine:x64".to_string(),
            format!("/out:{}", lib_path.display()),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", lib_exe.display()));
    if !status.success() || !lib_path.exists() {
        panic!("lib.exe failed to generate avicap32.lib");
    }
}
