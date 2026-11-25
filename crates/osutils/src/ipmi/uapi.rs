//! Minimal bindings for the Linux IPMI userspace ABI.
//!
//! The layouts and ioctl numbers mirror
//! [`linux/ipmi.h`](https://github.com/torvalds/linux/blob/4dce97c6c265544342e716e62fd24bbb232590c5/include/uapi/linux/ipmi.h).

use std::os::fd::RawFd;

use nix::libc::{c_int, c_long, c_short, c_uchar, c_uint, c_ushort};

/// Local-system-interface address type.
pub(super) const SYSTEM_INTERFACE_ADDRESS_TYPE: c_int = 0x0c;
/// Channel sentinel for addressing the local BMC directly.
pub(super) const BMC_CHANNEL: c_short = 0x0f;
/// Receive classification for a command response.
pub(super) const RESPONSE_RECEIVE_TYPE: c_int = 1;
/// Receive classification for an unsolicited asynchronous event.
pub(super) const ASYNC_EVENT_RECEIVE_TYPE: c_int = 2;
/// Maximum address size accepted by the Linux IPMI receive API.
pub(super) const MAX_ADDRESS_SIZE: usize = 32;

/// Linux `ipmi_system_interface_addr`.
#[repr(C)]
pub(super) struct IpmiSystemInterfaceAddr {
    /// Address discriminator.
    pub(super) address_type: c_int,
    /// Local BMC channel.
    pub(super) channel: c_short,
    /// Logical unit number.
    pub(super) lun: c_uchar,
}

/// Linux `ipmi_msg`.
#[repr(C)]
pub(super) struct IpmiMsg {
    /// Request or response network function.
    pub(super) netfn: c_uchar,
    /// Request or response command.
    pub(super) cmd: c_uchar,
    /// Number of bytes referenced by `data`.
    pub(super) data_len: c_ushort,
    /// Pointer to the request or response buffer.
    pub(super) data: *mut c_uchar,
}

/// Linux `ipmi_req`.
#[repr(C)]
pub(super) struct IpmiReq {
    /// Pointer to an address structure.
    pub(super) addr: *mut c_uchar,
    /// Size of the address structure.
    pub(super) addr_len: c_uint,
    /// Caller-selected response correlation identifier.
    pub(super) msgid: c_long,
    /// Message envelope and payload.
    pub(super) msg: IpmiMsg,
}

/// Linux `ipmi_recv`.
#[repr(C)]
pub(super) struct IpmiRecv {
    /// Kernel receive classification.
    pub(super) recv_type: c_int,
    /// Address output buffer.
    pub(super) addr: *mut c_uchar,
    /// Input capacity and output length of `addr`.
    pub(super) addr_len: c_uint,
    /// Response correlation identifier.
    pub(super) msgid: c_long,
    /// Response envelope and output buffer.
    pub(super) msg: IpmiMsg,
}

/// Submits one request with `IPMICTL_SEND_COMMAND`.
///
/// # Safety
///
/// Every pointer reachable from `request` must remain valid for the ioctl.
pub(super) unsafe fn send_command(fd: RawFd, request: &mut IpmiReq) -> nix::Result<c_int> {
    // SAFETY: upheld by the caller.
    unsafe { ioctl::send_command(fd, request) }
}

/// Receives one message with `IPMICTL_RECEIVE_MSG_TRUNC`.
///
/// # Safety
///
/// Every output pointer and capacity in `response` must describe valid writable storage.
pub(super) unsafe fn receive_msg_trunc(fd: RawFd, response: &mut IpmiRecv) -> nix::Result<c_int> {
    // SAFETY: upheld by the caller.
    unsafe { ioctl::receive_msg_trunc(fd, response) }
}

/// ioctl numbers from `linux/ipmi.h`.
mod ioctl {
    use super::{IpmiRecv, IpmiReq};

    nix::ioctl_read!(send_command, b'i', 13, IpmiReq);
    nix::ioctl_readwrite!(receive_msg_trunc, b'i', 11, IpmiRecv);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::mem::{align_of, offset_of, size_of};

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[test]
    fn layouts_match_linux_uapi_on_supported_targets() {
        assert_eq!(size_of::<IpmiSystemInterfaceAddr>(), 8);
        assert_eq!(align_of::<IpmiSystemInterfaceAddr>(), 4);
        assert_eq!(offset_of!(IpmiSystemInterfaceAddr, address_type), 0);
        assert_eq!(offset_of!(IpmiSystemInterfaceAddr, channel), 4);
        assert_eq!(offset_of!(IpmiSystemInterfaceAddr, lun), 6);

        assert_eq!(size_of::<IpmiMsg>(), 16);
        assert_eq!(align_of::<IpmiMsg>(), 8);
        assert_eq!(offset_of!(IpmiMsg, netfn), 0);
        assert_eq!(offset_of!(IpmiMsg, cmd), 1);
        assert_eq!(offset_of!(IpmiMsg, data_len), 2);
        assert_eq!(offset_of!(IpmiMsg, data), 8);

        assert_eq!(size_of::<IpmiReq>(), 40);
        assert_eq!(align_of::<IpmiReq>(), 8);
        assert_eq!(offset_of!(IpmiReq, addr), 0);
        assert_eq!(offset_of!(IpmiReq, addr_len), 8);
        assert_eq!(offset_of!(IpmiReq, msgid), 16);
        assert_eq!(offset_of!(IpmiReq, msg), 24);

        assert_eq!(size_of::<IpmiRecv>(), 48);
        assert_eq!(align_of::<IpmiRecv>(), 8);
        assert_eq!(offset_of!(IpmiRecv, recv_type), 0);
        assert_eq!(offset_of!(IpmiRecv, addr), 8);
        assert_eq!(offset_of!(IpmiRecv, addr_len), 16);
        assert_eq!(offset_of!(IpmiRecv, msgid), 24);
        assert_eq!(offset_of!(IpmiRecv, msg), 32);
    }
}
