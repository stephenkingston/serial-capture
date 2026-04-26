//! Copy the vendored USBPcap installer into OUT_DIR so the Windows binary
//! can `include_bytes!` it at compile time. The installer lives in `vendor/`
//! and ships with the source — no network access needed at build time.
//!
//! Override with: `USBPCAP_INSTALLER=/path/to/USBPcapSetup.exe cargo build`

use std::env;
use std::fs;
use std::path::PathBuf;

const VENDORED: &str = "vendor/USBPcapSetup-1.5.4.0.exe";
const OUT_NAME: &str = "USBPcapSetup.exe";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={VENDORED}");
    println!("cargo:rerun-if-env-changed=USBPCAP_INSTALLER");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let dest = out_dir.join(OUT_NAME);

    let src = match env::var("USBPCAP_INSTALLER") {
        Ok(p) => {
            println!("cargo:warning=USBPCAP_INSTALLER override: {p}");
            PathBuf::from(p)
        }
        Err(_) => {
            let manifest_dir =
                PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
            manifest_dir.join(VENDORED)
        }
    };

    fs::copy(&src, &dest).unwrap_or_else(|e| {
        panic!(
            "copying USBPcap installer from {} to {}: {e}",
            src.display(),
            dest.display()
        )
    });
}
