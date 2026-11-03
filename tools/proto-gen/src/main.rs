//! Regenerates the Rust code for `proto/` into `core/asiri-proto/src/gen/`.
//!
//! The generated files are committed so that ordinary builds (including the relay's Docker image
//! and the Mac XCFramework) never execute `protoc`. CI re-runs this tool and fails if the output
//! differs from what is committed.
//!
//! Usage: `cargo run -p proto-gen`

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let proto_dir = root.join("proto");
    let out_dir = root.join("core/asiri-proto/src/gen");
    std::fs::create_dir_all(&out_dir)?;

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.out_dir(&out_dir);
    config.btree_map(["."]);
    config.compile_protos(
        &[
            proto_dir.join("asiri/relay/v1/relay.proto"),
            proto_dir.join("asiri/app/v1/app.proto"),
            proto_dir.join("asiri/pairing/v1/pairing.proto"),
        ],
        &[proto_dir],
    )?;
    eprintln!("generated into {}", out_dir.display());
    Ok(())
}
