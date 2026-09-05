fn main() {
    // 静态 ffmpeg（vcpkg x64-windows-static）经 FFMPEG_DIR 链接时，
    // ffmpeg-sys 不透传外部依赖库（EXTRALIBS 仅 --features build 路径处理），
    // 这里补齐 libvpx 与 avdevice(DirectShow/MediaFoundation) 所需的系统库。
    // vcpkg 静态库用 /MT（LIBCMT），Rust debug 默认引 LIBCMTD —— 冲突告警抑制
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:LIBCMTD");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:LIBCMT");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:MSVCRTD");
    if let Ok(ffmpeg_dir) = std::env::var("FFMPEG_DIR") {
        println!("cargo:rustc-link-search=native={}\\lib", ffmpeg_dir);
        // SDK 不自带 avicap32.lib（vfwcap 依赖），用 lib.exe /def 生成后放在固定目录
        println!("cargo:rustc-link-search=native=D:/Tools/ffmpeg-static-extras");
        println!("cargo:rustc-link-lib=static=vpx");
        println!("cargo:rustc-link-lib=static=strmiids");
        println!("cargo:rustc-link-lib=static=mfuuid");
        println!("cargo:rustc-link-lib=static=uuid");
        println!("cargo:rustc-link-lib=static=winmm");
        println!("cargo:rustc-link-lib=static=ws2_32");
        println!("cargo:rustc-link-lib=static=secur32");
        println!("cargo:rustc-link-lib=static=bcrypt");
        println!("cargo:rustc-link-lib=static=user32");
        println!("cargo:rustc-link-lib=static=avicap32");
        println!("cargo:rustc-link-lib=static=msvfw32");
        println!("cargo:rustc-link-lib=static=gdi32");
        println!("cargo:rustc-link-lib=static=oleaut32");
        println!("cargo:rustc-link-lib=static=shlwapi");
        println!("cargo:rustc-link-lib=static=psapi");
        println!("cargo:rustc-link-lib=static=ncrypt");
        println!("cargo:rustc-link-lib=static=crypt32");
        println!("cargo:rustc-link-lib=static=zs");
    }
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
}
