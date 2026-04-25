#[cfg(target_os = "macos")]
compile_error!(
    "serial-capture is not supported on macOS. \
     USBPcap (Windows) and usbmon (Linux) have no macOS equivalent that this tool targets."
);

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
compile_error!(
    "serial-capture is not supported on Windows on ARM. \
     USBPcap has no signed ARM64 driver."
);

pub fn enforce_at_runtime() -> ! {
    eprintln!(
        "This platform is not supported.\n\
         serial-capture currently runs on Windows x64 and Linux x86_64/aarch64.\n\
         Detected: {} {}.",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    std::process::exit(2);
}

pub fn check() {
    let supported = matches!(
        (std::env::consts::OS, std::env::consts::ARCH),
        ("linux", "x86_64") | ("linux", "aarch64") | ("windows", "x86_64"),
    );
    if !supported {
        enforce_at_runtime();
    }
}
