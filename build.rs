//! Build-time fetch + pin of the USBPcap installer for embedding into the
//! Windows binary. Only runs when targeting Windows.
//!
//! Override the download with:  `USBPCAP_INSTALLER=/path/to/USBPcapSetup-1.5.4.0.exe cargo build`

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const USBPCAP_VERSION: &str = "1.5.4.0";
const USBPCAP_URL: &str =
    "https://github.com/desowin/usbpcap/releases/download/1.5.4.0/USBPcapSetup-1.5.4.0.exe";
/// SHA-256 of the official USBPcap 1.5.4.0 installer (verified 2026-04-25).
const USBPCAP_SHA256: &str = "87a7edf9bbbcf07b5f4373d9a192a6770d2ff3add7aa1e276e82e38582ccb622";
const USBPCAP_OUT_NAME: &str = "USBPcapSetup.exe";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=USBPCAP_INSTALLER");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        // Linux builds don't need the installer. Still write a zero-byte placeholder
        // so include_bytes!() in cfg(windows) code paths can reference a stable path
        // if the file is ever consulted during cross-target type-check sweeps.
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let dest = out_dir.join(USBPCAP_OUT_NAME);

    if let Ok(local) = env::var("USBPCAP_INSTALLER") {
        let local = PathBuf::from(local);
        println!("cargo:warning=USBPCAP_INSTALLER override: {}", local.display());
        fs::copy(&local, &dest).expect("copying USBPCAP_INSTALLER override");
        verify_or_die(&dest);
        return;
    }

    if dest.exists() && verify(&dest).is_ok() {
        return; // cached
    }

    println!(
        "cargo:warning=Downloading USBPcap {USBPCAP_VERSION} installer (≈190 KB) from GitHub..."
    );
    download(USBPCAP_URL, &dest).expect("downloading USBPcap installer");
    verify_or_die(&dest);
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("HTTP error fetching {url}: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading response body: {e}"))?;
    let mut f = fs::File::create(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    f.write_all(&buf)
        .map_err(|e| format!("writing {}: {e}", dest.display()))?;
    Ok(())
}

fn verify(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = h.finalize();
    let got_hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
    if got_hex != USBPCAP_SHA256 {
        return Err(format!(
            "SHA-256 mismatch for {}: expected {USBPCAP_SHA256}, got {got_hex}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_or_die(path: &Path) {
    if let Err(e) = verify(path) {
        let _ = fs::remove_file(path);
        panic!("{e}");
    }
}
