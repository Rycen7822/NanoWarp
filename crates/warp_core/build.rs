use anyhow::Result;

fn main() -> Result<()> {
    println!("cargo:rustc-cfg=feature=\"local_fs\"");

    Ok(())
}
