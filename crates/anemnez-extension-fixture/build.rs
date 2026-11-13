//! Compiles the fixture's protos, with the contract's options from the kernel crate, and keeps each
//! variant's real protoc FileDescriptorSet for Describe to serve.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let contract = out.join("proto");
    anemnez_extension::protos::write_all(&contract)?;
    let own = std::path::PathBuf::from("proto");
    println!("cargo:rerun-if-changed=proto");
    let includes = [own.clone(), contract];
    tonic_build::configure()
        .file_descriptor_set_path(out.join("good.fds"))
        .compile_protos(&[own.join("anemnez/fixture/v1/good.proto")], &includes)?;
    for variant in ["wrong_major", "unbounded", "person_act_allow", "colliding_names", "reserved_name"] {
        tonic_build::configure()
            .build_server(false)
            .build_client(false)
            .file_descriptor_set_path(out.join(format!("{variant}.fds")))
            .compile_protos(&[own.join(format!("anemnez/fixture/v1/{variant}.proto"))], &includes)?;
    }
    Ok(())
}
