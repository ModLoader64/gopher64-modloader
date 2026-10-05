use std::path::{Path, PathBuf};

pub fn build(os: &str, rdp_build: &mut cc::Build, volk_build: &mut cc::Build) {
    rdp_build
        .file("parallel-rdp/modloader_headless.cpp")
        .flag("-Wno-unused-variable");

    if os == "linux" {
        volk_build.define("VK_USE_PLATFORM_XLIB_KHR", None);
    }

    let out_path = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    generate_renderer_bindings(&out_path);
    build_rt64(os, &out_path.join("rt64"));
}

fn generate_renderer_bindings(out_path: &Path) {
    bindgen::Builder::default()
        .header("parallel-rdp/modloader_headless.h")
        .header("modloader-rt64/modloader_rt64.h")
        .allowlist_type("(ModLoader|RDP|RT64)_.*")
        .allowlist_var("(MODLOADER_RENDERER|RT64)_.*")
        .allowlist_function("(rdp_set_frame_callback|rdp_set_shared_frames|rt64_.*)")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate renderer bindings")
        .write_to_file(out_path.join("modloader_renderer_bindings.rs"))
        .expect("Unable to write renderer bindings");
}

fn build_rt64(os: &str, out: &Path) {
    println!("cargo::rerun-if-changed=rt64");
    println!("cargo::rerun-if-changed=modloader-rt64");
    let mut configure = std::process::Command::new("cmake");
    let mut build = std::process::Command::new("cmake");
    configure
        .args(["-S", "modloader-rt64", "-B"])
        .arg(out)
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"]);
    build
        .arg("--build")
        .arg(out)
        .args(["--target", "modloader_rt64"]);
    if os == "windows" {
        configure.args([
            "-DCMAKE_C_COMPILER=clang-cl",
            "-DCMAKE_CXX_COMPILER=clang-cl",
        ]);
        let target = std::env::var("TARGET").unwrap();
        if let Some(tool) = cc::windows_registry::find_tool(&target, "cl.exe") {
            for (key, value) in tool.env() {
                configure.env(key, value);
                build.env(key, value);
            }
        }
    }

    assert!(
        configure.status().unwrap().success(),
        "RT64 configure failed"
    );

    assert!(build.status().unwrap().success(), "RT64 build failed");

    for dir in [
        ".",
        "rt64",
        "rt64/src/contrib/plume",
        "rt64/src/contrib/re-spirv",
        "rt64/src/contrib/nativefiledialog-extended/src",
        "rt64/src/contrib/zstd/build/cmake/lib",
    ] {
        println!("cargo:rustc-link-search=native={}", out.join(dir).display());
    }

    for lib in [
        "modloader_rt64",
        "rt64",
        "plume",
        "re-spirv",
        "nfd",
        "zstd_static",
    ] {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    if os == "windows" {
        println!("cargo:rustc-link-search=native=rt64/src/contrib/dxc/lib/x64");
        for lib in [
            "dxcompiler",
            "d3d12",
            "dxgi",
            "shcore",
            "ole32",
            "uuid",
            "shell32",
            "user32",
            "gdi32",
            "dwmapi",
            "delayimp",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }

        // RT64 loads dxcompiler.dll from the adapter's folder
        println!("cargo:rustc-cdylib-link-arg=/DELAYLOAD:dxcompiler.dll");
    } else if os == "linux" {
        let libraries = std::fs::read_to_string(out.join("system-libraries.txt"))
            .expect("Unable to read RT64 system libraries");
        for library in libraries.lines() {
            println!("cargo:rustc-cdylib-link-arg={library}");
        }
        println!("cargo:rustc-cdylib-link-arg=-Wl,-z,defs");
    }
}
