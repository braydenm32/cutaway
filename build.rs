//! Windows: the program's icon and version details, built into Cutaway.exe.

fn main() {
    println!("cargo:rerun-if-changed=windows/cutaway.ico");
    println!("cargo:rerun-if-changed=build.rs");
    // Only when building on Windows (the resource compiler is part of the
    // Windows SDK); a check from another system goes without.
    let windows_target = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !(windows_target && cfg!(windows)) {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let parts: Vec<&str> = version.split(['.', '-']).take(3).collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("windows").join("cutaway.ico");
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Cutaway"
      VALUE "FileDescription", "Cutaway: multi-camera recorder, switcher and streamer"
      VALUE "ProductName", "Cutaway"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "Cutaway.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
        a = num(0),
        b = num(1),
        c = num(2),
    );
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("cutaway.rc");
    std::fs::write(&out, rc).expect("writing the resource script");
    let _ = embed_resource::compile(&out, embed_resource::NONE).manifest_optional();
}
