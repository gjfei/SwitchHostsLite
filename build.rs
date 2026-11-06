fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // `rust_i18n::i18n!` reads the locale files at compile time but does not
    // tell cargo, so edits there would otherwise not trigger a rebuild.
    println!("cargo:rerun-if-changed=locales");
    // `embed-resource` is only a build dependency on Windows hosts, so cross
    // builds from other hosts go without the icon.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/app.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        // Only the icon: gpui already links its own application manifest.
        embed_resource::compile("assets/app.rc", embed_resource::ParamsIncludeDirs(["assets"]))
            .manifest_optional()
            .unwrap();
    }
}
