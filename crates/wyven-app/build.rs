//! Puts Wyven's icon and version info into wyven.exe (Explorer, taskbar, installer).

fn main() {
    println!("cargo:rerun-if-changed=wyven.rc");
    println!("cargo:rerun-if-changed=../../assets/wyven.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = env!("CARGO_PKG_VERSION");
        let parts: Vec<&str> = version.split('.').collect();
        let num = |i: usize| parts.get(i).copied().unwrap_or("0");
        let comma = format!("{},{},{},0", num(0), num(1), num(2));
        embed_resource::compile(
            "wyven.rc",
            [format!("WYVEN_VERSION_NUM={comma}"), format!("WYVEN_VERSION_STR=\"{version}\"")],
        )
        .manifest_optional()
        .unwrap();
    }
}
