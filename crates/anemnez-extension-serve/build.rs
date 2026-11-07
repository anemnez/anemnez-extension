//! Compiles the contract's protos, written out by the kernel crate this build depends on.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("proto");
    anemnez_extension::protos::write_all(&root)?;
    let files: Vec<_> = anemnez_extension::protos::FILES
        .iter()
        .map(|(path, _)| root.join(path))
        .collect();
    tonic_build::configure().compile_protos(&files, &[root])?;
    Ok(())
}
