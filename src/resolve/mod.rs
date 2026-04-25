#[derive(Debug, Clone, Copy)]
pub struct PortInfo {
    pub bus: u16,
    pub devnum: u8,
    pub vid: u16,
    pub pid: u16,
    pub interface_number: Option<u8>,
    /// Bulk IN endpoint address (bit 7 = 1). None if not discoverable.
    pub bulk_in_ep: Option<u8>,
    /// Bulk OUT endpoint address (bit 7 = 0). None if not discoverable.
    pub bulk_out_ep: Option<u8>,
    /// wMaxPacketSize of the bulk IN endpoint — needed by the FTDI decoder.
    pub bulk_in_max_packet: Option<u16>,
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::resolve;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::resolve;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn resolve(_port: &str) -> anyhow::Result<PortInfo> {
    unreachable!("platform guard should have exited before this point");
}
