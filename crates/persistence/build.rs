fn main() -> Result<(), std::env::VarError> {
    println!("cargo:rustc-cfg=feature=\"local_fs\"");

    Ok(())
}
