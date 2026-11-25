//! Sends IPMI Platform Event Messages through the local Linux system interface.
//!
//! A direct ioctl is deliberate for this one local command: it avoids ipmi-rs's broad RMCP
//! crypto dependencies and ipmitool's image binary and temporary-file dependency.
//!
//! Wire-value references:
//!
//! | Values | Definition |
//! | --- | --- |
//! | address `0Ch`, channel `0Fh`, receive types `1` and `2` | [Linux IPMI userspace API] |
//! | NetFn `04h`/`05h`, command `02h`, LUN `00b` | [IPMI v2.0 revision 1.1], sections 29.1, 29.3 and 29.6 |
//! | software ID `20h`, encoded Generator ID `41h` | IPMI sections 5.4 and 29.4 |
//! | Event Message revision `04h` | IPMI section 29.4 |
//! | sensor type `1Fh`, offsets `07h`-`0Ah` | IPMI section 42.2 |
//! | sensor-specific event type `6Fh` | IPMI sections 41.2 and 42.1 |
//! | sensor number and unused data `00h` | [Anaconda's software-generated event layout] |
//!
//! [Linux IPMI userspace API]: https://github.com/torvalds/linux/blob/4dce97c6c265544342e716e62fd24bbb232590c5/include/uapi/linux/ipmi.h
//! [IPMI v2.0 revision 1.1]: https://www.intel.com/content/dam/www/public/us/en/documents/product-briefs/ipmi-second-gen-interface-spec-v2-rev1-1.pdf
//! [Anaconda's software-generated event layout]: https://github.com/rhinstaller/anaconda/blob/8f123da05ee8e8936b201265ade18a15c267f2ac/pyanaconda/core/util.py#L605-L623

use std::{
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::{
        fd::{AsFd, AsRawFd},
        unix::fs::OpenOptionsExt,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Error};
use nix::{
    libc::{self, c_int, c_long, c_short, c_uchar, c_uint, c_ushort},
    poll::{self, PollFd, PollFlags, PollTimeout},
};

/// Primary local Linux IPMI character device.
const IPMI_DEVICE: &str = "/dev/ipmi0";
/// Maximum time to wait for the BMC response to one request.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Linux local-system-interface address type from `linux/ipmi.h`.
const IPMI_SYSTEM_INTERFACE_ADDR_TYPE: c_int = 0x0c;
/// Linux BMC channel sentinel from `linux/ipmi.h`.
const IPMI_BMC_CHANNEL: c_short = 0x0f;
/// Linux response receive type from `linux/ipmi.h`.
const IPMI_RESPONSE_RECV_TYPE: c_int = 1;
/// Linux asynchronous-event receive type from `linux/ipmi.h`.
const IPMI_ASYNC_EVENT_RECV_TYPE: c_int = 2;
/// Sensor/Event request NetFn from IPMI v2.0 section 29.1.
const IPMI_REQUEST_NETFN: c_uchar = 0x04;
/// Sensor/Event response NetFn from IPMI v2.0 section 29.1.
const IPMI_RESPONSE_NETFN: c_uchar = 0x05;
/// Platform Event Message command from IPMI v2.0 section 29.3.
const IPMI_PLATFORM_EVENT_COMMAND: c_uchar = 0x02;
/// Local BMC logical unit used by the system-interface format in IPMI v2.0 section 29.6.
const IPMI_BMC_LUN: c_uchar = 0x00;
/// Correlation identifier for this module's single synchronous request.
const IPMI_MESSAGE_ID: c_long = 1;
/// Address buffer size accepted by `IPMICTL_RECEIVE_MSG_TRUNC`.
const IPMI_MAX_ADDR_SIZE: usize = 32;
/// Response buffer size for the completion code and any BMC response data.
const IPMI_RESPONSE_DATA_SIZE: usize = 32;
/// System Management Software ID assigned by IPMI v2.0 table 5-4.
const IPMI_SYSTEM_MANAGEMENT_SOFTWARE_ID: u8 = 0x20;
/// Marks a Generator ID as a software ID per IPMI v2.0 sections 5.4 and 29.4.
const IPMI_SOFTWARE_GENERATOR_FLAG: u8 = 0x01;
/// Event Message revision required by IPMI v2.0 section 29.4.
const IPMI_EVENT_MESSAGE_REVISION: u8 = 0x04;
/// Base OS Boot/Installation Status sensor type from IPMI v2.0 section 42.2.
const IPMI_INSTALLATION_SENSOR_TYPE: u8 = 0x1f;
/// Synthetic sensor number used by Anaconda-compatible software-generated installation events.
const IPMI_INSTALLATION_SENSOR_NUMBER: u8 = 0x00;
/// Sensor-specific event type from IPMI v2.0 sections 41.2 and 42.1.
const IPMI_SENSOR_SPECIFIC_EVENT_TYPE: u8 = 0x6f;
/// Unused Event Data fields for the installation-status offsets in IPMI v2.0 section 42.2.
const IPMI_UNUSED_EVENT_DATA: u8 = 0x00;
/// Successful command completion code from IPMI v2.0 section 5.2.
const IPMI_COMPLETION_CODE_SUCCESS: u8 = 0x00;

/// Base OS or hypervisor installation state defined by IPMI v2.0 section 42.2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[repr(u8)]
pub enum InstallationStatus {
    /// Base OS or hypervisor installation started (`07h`).
    Started = 0x07,
    /// Base OS or hypervisor installation completed (`08h`).
    Completed = 0x08,
    /// Base OS or hypervisor installation was explicitly aborted (`09h`).
    Aborted = 0x09,
    /// Base OS or hypervisor installation failed (`0Ah`).
    Failed = 0x0a,
}

/// Result of attempting to send an event to the local BMC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SendOutcome {
    /// The BMC acknowledged the event.
    Sent,
    /// The local IPMI device is not present.
    Unavailable,
}

/// Reports an IPMI-defined base OS or hypervisor installation state to the local BMC.
///
/// The event uses the system-management software Generator ID and the Base OS Boot/Installation
/// Status sensor layout defined by IPMI v2.0. The call waits up to five seconds for the matching
/// BMC response, ignoring unrelated asynchronous messages.
///
/// Returns [`SendOutcome::Unavailable`] only when `/dev/ipmi0` does not exist. Device-open errors,
/// ioctl failures, timeouts, malformed replies, and BMC completion errors are returned to the
/// caller.
pub fn send_installation_status(status: InstallationStatus) -> Result<SendOutcome, Error> {
    send_with_opener(installation_status_payload(status), LinuxTransport::open)
}

/// Encodes the IPMI v2.0 system-interface Platform Event Message for an installation state.
const fn installation_status_payload(status: InstallationStatus) -> [u8; 8] {
    [
        (IPMI_SYSTEM_MANAGEMENT_SOFTWARE_ID << 1) | IPMI_SOFTWARE_GENERATOR_FLAG,
        IPMI_EVENT_MESSAGE_REVISION,
        IPMI_INSTALLATION_SENSOR_TYPE,
        IPMI_INSTALLATION_SENSOR_NUMBER,
        IPMI_SENSOR_SPECIFIC_EVENT_TYPE,
        status as u8,
        IPMI_UNUSED_EVENT_DATA,
        IPMI_UNUSED_EVENT_DATA,
    ]
}

/// Internal representation of one Linux IPMI request.
#[derive(Clone, Copy, Debug)]
struct Request {
    /// Linux IPMI destination address type.
    address_type: c_int,
    /// Destination IPMI channel.
    channel: c_short,
    /// Destination logical unit number.
    lun: c_uchar,
    /// Request network function.
    netfn: c_uchar,
    /// Request command number.
    cmd: c_uchar,
    /// Identifier used to correlate the response.
    msgid: c_long,
    /// Platform Event Message system-interface payload.
    data: [u8; 8],
}

/// Owned fields extracted from one Linux IPMI receive operation.
#[derive(Debug)]
struct Response {
    /// Linux IPMI receive classification.
    recv_type: c_int,
    /// Identifier copied from the matching request.
    msgid: c_long,
    /// Response network function.
    netfn: c_uchar,
    /// Response command number.
    cmd: c_uchar,
    /// Completion code followed by optional response data.
    data: Vec<u8>,
}

/// Minimal synchronous transport contract used by production and tests.
trait Transport {
    /// Submits one request to the local BMC.
    fn send(&mut self, request: &Request) -> Result<(), Error>;
    /// Receives one response or returns `None` when the timeout expires.
    fn receive(&mut self, timeout: Duration) -> Result<Option<Response>, Error>;
}

/// Linux character-device transport for the local IPMI system interface.
struct LinuxTransport {
    /// Open read/write handle used for polling and ioctls.
    device: File,
}

impl LinuxTransport {
    /// Opens the primary local IPMI device without leaking the descriptor across exec.
    fn open() -> Result<Self, io::Error> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(IPMI_DEVICE)
            .map(|device| Self { device })
    }
}

impl Transport for LinuxTransport {
    /// Converts the internal request into the Linux `ipmi_req` UAPI layout.
    fn send(&mut self, request: &Request) -> Result<(), Error> {
        let mut address = IpmiSystemInterfaceAddr {
            addr_type: request.address_type,
            channel: request.channel,
            lun: request.lun,
        };
        let mut data = request.data;
        let mut request = IpmiReq {
            addr: (&mut address as *mut IpmiSystemInterfaceAddr).cast(),
            addr_len: size_to_c_uint::<IpmiSystemInterfaceAddr>()?,
            msgid: request.msgid,
            msg: IpmiMsg {
                netfn: request.netfn,
                cmd: request.cmd,
                data_len: data
                    .len()
                    .try_into()
                    .context("IPMI request payload length does not fit the Linux UAPI")?,
                data: data.as_mut_ptr(),
            },
        };

        // SAFETY: `request` and every pointer it contains refer to initialized, writable values
        // that remain alive for the ioctl. Their C layouts match linux/ipmi.h.
        unsafe { ioctl::send_command(self.device.as_raw_fd(), &mut request) }
            .context("IPMICTL_SEND_COMMAND failed")?;
        Ok(())
    }

    /// Polls the IPMI descriptor and copies one kernel response into owned storage.
    fn receive(&mut self, timeout: Duration) -> Result<Option<Response>, Error> {
        let mut descriptors = [PollFd::new(self.device.as_fd(), PollFlags::POLLIN)];
        let timeout = PollTimeout::try_from(timeout).context("IPMI response timeout is invalid")?;
        if poll::poll(&mut descriptors, timeout).context("Failed to poll the IPMI device")? == 0 {
            return Ok(None);
        }
        let events = descriptors[0]
            .revents()
            .ok_or_else(|| anyhow!("IPMI poll returned unrecognized event flags"))?;
        if events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
            bail!("IPMI device poll failed with {events:?}");
        }
        if !events.contains(PollFlags::POLLIN) {
            bail!("IPMI device became ready without readable data: {events:?}");
        }

        let mut address = [0_u8; IPMI_MAX_ADDR_SIZE];
        let mut data = [0_u8; IPMI_RESPONSE_DATA_SIZE];
        let mut response = IpmiRecv {
            recv_type: 0,
            addr: address.as_mut_ptr(),
            addr_len: address
                .len()
                .try_into()
                .context("IPMI address buffer length does not fit the Linux UAPI")?,
            msgid: 0,
            msg: IpmiMsg {
                netfn: 0,
                cmd: 0,
                data_len: data
                    .len()
                    .try_into()
                    .context("IPMI response buffer length does not fit the Linux UAPI")?,
                data: data.as_mut_ptr(),
            },
        };

        // SAFETY: `response` points to initialized address and data buffers that outlive the ioctl.
        // The kernel may update the scalar fields and buffer contents according to linux/ipmi.h.
        unsafe { ioctl::receive_msg_trunc(self.device.as_raw_fd(), &mut response) }
            .context("IPMICTL_RECEIVE_MSG_TRUNC failed")?;
        let data_len = usize::from(response.msg.data_len);
        if data_len > data.len() {
            bail!(
                "IPMI response length {data_len} exceeds the {}-byte receive buffer",
                data.len()
            );
        }
        Ok(Some(Response {
            recv_type: response.recv_type,
            msgid: response.msgid,
            netfn: response.msg.netfn,
            cmd: response.msg.cmd,
            data: data[..data_len].to_vec(),
        }))
    }
}

/// Converts a Rust structure size into the Linux UAPI's unsigned integer type.
fn size_to_c_uint<T>() -> Result<c_uint, Error> {
    size_of::<T>()
        .try_into()
        .context("IPMI structure size does not fit the Linux UAPI")
}

/// Opens a transport and distinguishes a missing local device from operational errors.
fn send_with_opener<T>(
    data: [u8; 8],
    open: impl FnOnce() -> Result<T, io::Error>,
) -> Result<SendOutcome, Error>
where
    T: Transport,
{
    let mut transport = match open() {
        Ok(transport) => transport,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SendOutcome::Unavailable);
        }
        Err(error) => return Err(error).context("Failed to open local IPMI device"),
    };
    send_with_transport(data, &mut transport)?;
    Ok(SendOutcome::Sent)
}

/// Sends one event and waits for its correlated successful response.
fn send_with_transport(data: [u8; 8], transport: &mut impl Transport) -> Result<(), Error> {
    let request = Request {
        address_type: IPMI_SYSTEM_INTERFACE_ADDR_TYPE,
        channel: IPMI_BMC_CHANNEL,
        lun: IPMI_BMC_LUN,
        netfn: IPMI_REQUEST_NETFN,
        cmd: IPMI_PLATFORM_EVENT_COMMAND,
        msgid: IPMI_MESSAGE_ID,
        data,
    };
    transport
        .send(&request)
        .context("Failed to send IPMI Platform Event Message to the local BMC")?;

    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Timed out waiting for the local BMC Platform Event Message response");
        }
        let Some(response) = transport
            .receive(remaining)
            .context("Failed to receive Platform Event Message response from the local BMC")?
        else {
            bail!("Timed out waiting for the local BMC Platform Event Message response");
        };
        if response.recv_type == IPMI_ASYNC_EVENT_RECV_TYPE || response.msgid != request.msgid {
            continue;
        }
        validate_response(&response)?;
        return Ok(());
    }
}

/// Verifies the response envelope and BMC completion code.
fn validate_response(response: &Response) -> Result<(), Error> {
    if response.recv_type != IPMI_RESPONSE_RECV_TYPE {
        bail!(
            "Unexpected IPMI receive type {} for Platform Event Message response",
            response.recv_type
        );
    }
    if response.netfn != IPMI_RESPONSE_NETFN {
        bail!(
            "Unexpected IPMI response NetFn {:#04x}; expected {IPMI_RESPONSE_NETFN:#04x}",
            response.netfn
        );
    }
    if response.cmd != IPMI_PLATFORM_EVENT_COMMAND {
        bail!(
            "Unexpected IPMI response command {:#04x}; expected {IPMI_PLATFORM_EVENT_COMMAND:#04x}",
            response.cmd
        );
    }
    let completion = response
        .data
        .first()
        .ok_or_else(|| anyhow!("IPMI Platform Event Message response has no completion code"))?;
    if *completion != IPMI_COMPLETION_CODE_SUCCESS {
        bail!("BMC rejected Platform Event Message with completion code {completion:#04x}");
    }
    Ok(())
}

/// Linux `ipmi_system_interface_addr` layout from `linux/ipmi.h`.
#[repr(C)]
struct IpmiSystemInterfaceAddr {
    /// Address discriminator.
    addr_type: c_int,
    /// Local BMC channel.
    channel: c_short,
    /// Logical unit number.
    lun: c_uchar,
}

/// Linux `ipmi_msg` layout from `linux/ipmi.h`.
#[repr(C)]
struct IpmiMsg {
    /// Request or response network function.
    netfn: c_uchar,
    /// Request or response command.
    cmd: c_uchar,
    /// Number of bytes referenced by `data`.
    data_len: c_ushort,
    /// Pointer to the request or response buffer.
    data: *mut c_uchar,
}

/// Linux `ipmi_req` layout from `linux/ipmi.h`.
#[repr(C)]
struct IpmiReq {
    /// Pointer to an address structure.
    addr: *mut c_uchar,
    /// Size of the address structure.
    addr_len: c_uint,
    /// Caller-selected response correlation identifier.
    msgid: c_long,
    /// Message envelope and payload.
    msg: IpmiMsg,
}

/// Linux `ipmi_recv` layout from `linux/ipmi.h`.
#[repr(C)]
struct IpmiRecv {
    /// Kernel receive classification.
    recv_type: c_int,
    /// Address output buffer.
    addr: *mut c_uchar,
    /// Input capacity and output length of `addr`.
    addr_len: c_uint,
    /// Response correlation identifier.
    msgid: c_long,
    /// Response envelope and output buffer.
    msg: IpmiMsg,
}

/// Linux ioctl bindings from `linux/ipmi.h`.
mod ioctl {
    use super::{IpmiRecv, IpmiReq};

    nix::ioctl_read!(send_command, b'i', 13, IpmiReq);
    nix::ioctl_readwrite!(receive_msg_trunc, b'i', 11, IpmiRecv);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeTransport {
        requests: Vec<Request>,
        responses: VecDeque<Result<Option<Response>, Error>>,
        send_error: bool,
    }

    impl FakeTransport {
        fn successful() -> Self {
            Self {
                responses: VecDeque::from([Ok(Some(successful_response()))]),
                ..Self::default()
            }
        }
    }

    impl Transport for FakeTransport {
        fn send(&mut self, request: &Request) -> Result<(), Error> {
            self.requests.push(*request);
            if self.send_error {
                bail!("injected send failure");
            }
            Ok(())
        }

        fn receive(&mut self, _timeout: Duration) -> Result<Option<Response>, Error> {
            self.responses.pop_front().unwrap_or_else(|| Ok(None))
        }
    }

    fn successful_response() -> Response {
        Response {
            recv_type: IPMI_RESPONSE_RECV_TYPE,
            msgid: IPMI_MESSAGE_ID,
            netfn: IPMI_RESPONSE_NETFN,
            cmd: IPMI_PLATFORM_EVENT_COMMAND,
            data: vec![0],
        }
    }

    #[test]
    fn installation_statuses_use_ipmi_payload_bytes() {
        for (status, event) in [
            (InstallationStatus::Started, 0x07),
            (InstallationStatus::Completed, 0x08),
            (InstallationStatus::Aborted, 0x09),
            (InstallationStatus::Failed, 0x0a),
        ] {
            assert_eq!(
                installation_status_payload(status),
                [0x41, 0x04, 0x1f, 0x00, 0x6f, event, 0x00, 0x00]
            );
        }
    }

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[test]
    fn linux_ipmi_uapi_layout_matches_supported_targets() {
        assert_eq!(size_of::<IpmiSystemInterfaceAddr>(), 8);
        assert_eq!(size_of::<IpmiMsg>(), 16);
        assert_eq!(size_of::<IpmiReq>(), 40);
        assert_eq!(size_of::<IpmiRecv>(), 48);
    }

    #[test]
    fn request_uses_platform_event_command_and_payload() {
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut transport = FakeTransport::successful();
        send_with_transport(data, &mut transport).unwrap();
        assert_eq!(transport.requests.len(), 1);
        assert_eq!(transport.requests[0].address_type, 0x0c);
        assert_eq!(transport.requests[0].channel, 0x0f);
        assert_eq!(transport.requests[0].lun, 0);
        assert_eq!(transport.requests[0].netfn, 0x04);
        assert_eq!(transport.requests[0].cmd, 0x02);
        assert_eq!(transport.requests[0].data, data);
    }

    #[test]
    fn successful_response_reports_sent() {
        assert_eq!(
            send_with_opener([0; 8], || { Ok(FakeTransport::successful()) }).unwrap(),
            SendOutcome::Sent
        );
    }

    #[test]
    fn missing_device_reports_unavailable() {
        assert_eq!(
            send_with_opener::<FakeTransport>([0; 8], || {
                Err(io::Error::from(io::ErrorKind::NotFound))
            })
            .unwrap(),
            SendOutcome::Unavailable
        );
    }

    #[test]
    fn real_errors_are_propagated() {
        let open = send_with_opener::<FakeTransport>([0; 8], || {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert!(format!("{open:#}").contains("Failed to open local IPMI device"));

        let mut transport = FakeTransport {
            send_error: true,
            ..FakeTransport::default()
        };
        let send = send_with_transport([0; 8], &mut transport).unwrap_err();
        assert!(format!("{send:#}").contains("injected send failure"));
    }

    #[test]
    fn nonzero_completion_code_is_an_error() {
        let mut response = successful_response();
        response.data[0] = 0xc1;
        let mut transport = FakeTransport {
            responses: VecDeque::from([Ok(Some(response))]),
            ..FakeTransport::default()
        };
        let error = send_with_transport([0; 8], &mut transport).unwrap_err();
        assert!(format!("{error:#}").contains("completion code 0xc1"));
    }

    #[test]
    fn timeout_is_an_error() {
        let mut transport = FakeTransport::default();
        let error = send_with_transport([0; 8], &mut transport).unwrap_err();
        assert!(format!("{error:#}").contains("Timed out"));
    }

    #[test]
    fn unrelated_messages_are_ignored() {
        let mut async_event = successful_response();
        async_event.recv_type = IPMI_ASYNC_EVENT_RECV_TYPE;
        let mut other_request = successful_response();
        other_request.msgid += 1;
        let mut transport = FakeTransport {
            responses: VecDeque::from([
                Ok(Some(async_event)),
                Ok(Some(other_request)),
                Ok(Some(successful_response())),
            ]),
            ..FakeTransport::default()
        };
        send_with_transport([0; 8], &mut transport).unwrap();
    }

    #[test]
    fn matching_response_fields_are_validated() {
        let cases = [
            (
                Response {
                    recv_type: 3,
                    ..successful_response()
                },
                "receive type",
            ),
            (
                Response {
                    netfn: 0x07,
                    ..successful_response()
                },
                "NetFn",
            ),
            (
                Response {
                    cmd: 0x03,
                    ..successful_response()
                },
                "command",
            ),
            (
                Response {
                    data: Vec::new(),
                    ..successful_response()
                },
                "completion code",
            ),
        ];
        for (response, expected) in cases {
            let mut transport = FakeTransport {
                responses: VecDeque::from([Ok(Some(response))]),
                ..FakeTransport::default()
            };
            let error = send_with_transport([0; 8], &mut transport).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }
    }
}
