use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/datadiver2.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let rc = out_dir().join("datadiver.rc");
        fs::write(&rc, windows_resource()).expect("failed to write the Windows resource script");
        embed_resource::compile_for(&rc, ["datadiver"], embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the DataDiver icon and version information");
    }
}

fn out_dir() -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"))
}

/// Generated so the exe's version info always matches Cargo.toml.
fn windows_resource() -> String {
    let var = |name: &str| env::var(name).unwrap_or_else(|_| panic!("{name} is set by Cargo"));
    let icon = PathBuf::from(var("CARGO_MANIFEST_DIR")).join("assets").join("datadiver2.ico");
    let icon = icon.display().to_string().replace('\\', "\\\\");
    let version = var("CARGO_PKG_VERSION");
    let numeric = format!(
        "{},{},{},0",
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH")
    );
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "DataDiver\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "datadiver\0"
            VALUE "OriginalFilename", "datadiver.exe\0"
            VALUE "ProductName", "DataDiver\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    )
}
