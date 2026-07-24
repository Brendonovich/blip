use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let info_plist = manifest_dir.join("Info.plist");
    println!("cargo::rerun-if-changed={}", info_plist.display());
    println!("cargo::rustc-link-arg=-sectcreate");
    println!("cargo::rustc-link-arg=__TEXT");
    println!("cargo::rustc-link-arg=__info_plist");
    println!("cargo::rustc-link-arg={}", info_plist.display());
    Ok(())
}
