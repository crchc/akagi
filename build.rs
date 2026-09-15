use std::{env, path::PathBuf};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    prost_build::Config::new()
        .protoc_executable(protoc)
        .type_attribute(".", "#[allow(dead_code)]")
        .file_descriptor_set_path(out_dir.join("liqi_desc.bin"))
        .compile_protos(
            &["src/bridge/majsoul/proto/liqi.proto"],
            &["src/bridge/majsoul/proto/"],
        )
        .expect("failed to compile liqi.proto");

    println!("cargo:rerun-if-changed=src/bridge/majsoul/proto/liqi.proto");
    println!("cargo:rerun-if-changed=build.rs");

    // Used to locate the matching bundled Python and uv binaries at runtime.
    let target = env::var("TARGET").expect("TARGET not set by cargo");
    println!("cargo:rustc-env=TARGET_TRIPLE={target}");
}
