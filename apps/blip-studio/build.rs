use std::{env, error::Error, fs, path::PathBuf};

use wgsl_bindgen::{WgslBindgenOptionBuilder, WgslTypeSerializeStrategy};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let info_plist = manifest_dir.join("Info.plist");
    let output = PathBuf::from(env::var("OUT_DIR")?).join("shader_bindings.rs");
    WgslBindgenOptionBuilder::default()
        .workspace_root("shaders")
        .add_entry_point("shaders/compositor.wgsl")
        .serialization_strategy(WgslTypeSerializeStrategy::Bytemuck)
        .output(&output)
        .build()?
        .generate()?;
    let bindings = fs::read_to_string(&output)?.replace(
        "#![allow(unused, non_snake_case, non_camel_case_types, non_upper_case_globals)]\n",
        "",
    );
    fs::write(output, bindings)?;
    println!("cargo::rerun-if-changed=shaders/compositor.wgsl");
    println!("cargo::rerun-if-changed={}", info_plist.display());
    println!("cargo::rustc-link-arg=-sectcreate");
    println!("cargo::rustc-link-arg=__TEXT");
    println!("cargo::rustc-link-arg=__info_plist");
    println!("cargo::rustc-link-arg={}", info_plist.display());
    Ok(())
}
