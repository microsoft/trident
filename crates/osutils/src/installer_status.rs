//! Reports installer lifecycle events through the local Linux IPMI system interface.
//!
//! A direct ioctl is deliberate for this one fixed local command: it avoids ipmi-rs's broad RMCP
//! crypto dependencies and ipmitool's image binary and temporary-file dependency.

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
use libc::{c_int, c_long, c_short, c_uchar, c_uint, c_ushort};
use nix::poll::{self, PollFd, PollFlags, PollTimeout};

const IPMI_DEVICE: &str = "/dev/ipmi0";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const IPMI_SYSTEM_INTERFACE_ADDR_TYPE: c_int = 0x0c;
const IPMI_BMC_CHANNEL: c_short = 0x0f;
const IPMI_RESPONSE_RECV_TYPE: c_int = 1;
const IPMI_ASYNC_EVENT_RECV_TYPE: c_int = 2;
const IPMI_REQUEST_NETFN: c_uchar = 0x04;
const IPMI_RESPONSE_NETFN: c_uchar = 0x05;
const IPMI_PLATFORM_EVENT_COMMAND: c_uchar = 0x02;
const IPMI_MESSAGE_ID: c_long = 1;
const IPMI_MAX_ADDR_SIZE: usize = 32;
const IPMI_RESPONSE_DATA_SIZE: usize = 32;

/// Installer lifecycle state reported to the local BMC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InstallerStatus {
    Started,
    Completed,
    Aborted,
    Failed,
}

/// Result of attempting to report an installer lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReportOutcome {
    Sent,
    Unavailable,
}

/// Reports an installer lifecycle state through `/dev/ipmi0`.
pub fn report(status: InstallerStatus) -> Result<ReportOutcome, Error> {
    report_with_opener(status, LinuxTransport::open)
}

#[derive(Clone, Copy, Debug)]
struct Request {
    address_type: c_int,
    channel: c_short,
    lun: c_uchar,
    netfn: c_uchar,
    cmd: c_uchar,
    msgid: c_long,
    data: [u8; 8],
}

#[derive(Debug)]
struct Response {
    recv_type: c_int,
    msgid: c_long,
    netfn: c_uchar,
    cmd: c_uchar,
    data: Vec<u8>,
}

trait Transport {
    fn send(&mut self, request: &Request) -> Result<(), Error>;
    fn receive(&mut self, timeout: Duration) -> Result<Option<Response>, Error>;
}

struct LinuxTransport {
    device: File,
}

impl LinuxTransport {
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

fn size_to_c_uint<T>() -> Result<c_uint, Error> {
    size_of::<T>()
        .try_into()
        .context("IPMI structure size does not fit the Linux UAPI")
}

fn report_with_opener<T>(
    status: InstallerStatus,
    open: impl FnOnce() -> Result<T, io::Error>,
) -> Result<ReportOutcome, Error>
where
    T: Transport,
{
    let mut transport = match open() {
        Ok(transport) => transport,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ReportOutcome::Unavailable);
        }
        Err(error) => return Err(error).context("Failed to open /dev/ipmi0 for installer status"),
    };
    report_with_transport(status, &mut transport)?;
    Ok(ReportOutcome::Sent)
}

fn report_with_transport(
    status: InstallerStatus,
    transport: &mut impl Transport,
) -> Result<(), Error> {
    let request = Request {
        address_type: IPMI_SYSTEM_INTERFACE_ADDR_TYPE,
        channel: IPMI_BMC_CHANNEL,
        lun: 0,
        netfn: IPMI_REQUEST_NETFN,
        cmd: IPMI_PLATFORM_EVENT_COMMAND,
        msgid: IPMI_MESSAGE_ID,
        data: payload(status),
    };
    transport
        .send(&request)
        .context("Failed to send installer status to the local BMC")?;

    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Timed out waiting for the local BMC installer status response");
        }
        let Some(response) = transport
            .receive(remaining)
            .context("Failed to receive installer status response from the local BMC")?
        else {
            bail!("Timed out waiting for the local BMC installer status response");
        };
        if response.recv_type == IPMI_ASYNC_EVENT_RECV_TYPE || response.msgid != request.msgid {
            continue;
        }
        validate_response(&response)?;
        return Ok(());
    }
}

fn validate_response(response: &Response) -> Result<(), Error> {
    if response.recv_type != IPMI_RESPONSE_RECV_TYPE {
        bail!(
            "Unexpected IPMI receive type {} for installer status response",
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
        .ok_or_else(|| anyhow!("IPMI installer status response has no completion code"))?;
    if *completion != 0 {
        bail!("BMC rejected installer status with completion code {completion:#04x}");
    }
    Ok(())
}

const fn payload(status: InstallerStatus) -> [u8; 8] {
    let event = match status {
        InstallerStatus::Started => 0x07,
        InstallerStatus::Completed => 0x08,
        InstallerStatus::Aborted => 0x09,
        InstallerStatus::Failed => 0x0a,
    };
    [0x41, 0x04, 0x1f, 0x00, 0x6f, event, 0x00, 0x00]
}

#[repr(C)]
struct IpmiSystemInterfaceAddr {
    addr_type: c_int,
    channel: c_short,
    lun: c_uchar,
}

#[repr(C)]
struct IpmiMsg {
    netfn: c_uchar,
    cmd: c_uchar,
    data_len: c_ushort,
    data: *mut c_uchar,
}

#[repr(C)]
struct IpmiReq {
    addr: *mut c_uchar,
    addr_len: c_uint,
    msgid: c_long,
    msg: IpmiMsg,
}

#[repr(C)]
struct IpmiRecv {
    recv_type: c_int,
    addr: *mut c_uchar,
    addr_len: c_uint,
    msgid: c_long,
    msg: IpmiMsg,
}

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

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[test]
    fn linux_ipmi_uapi_layout_matches_supported_targets() {
        assert_eq!(size_of::<IpmiSystemInterfaceAddr>(), 8);
        assert_eq!(size_of::<IpmiMsg>(), 16);
        assert_eq!(size_of::<IpmiReq>(), 40);
        assert_eq!(size_of::<IpmiRecv>(), 48);
    }

    #[test]
    fn statuses_use_anaconda_payload_bytes() {
        for (status, event) in [
            (InstallerStatus::Started, 0x07),
            (InstallerStatus::Completed, 0x08),
            (InstallerStatus::Aborted, 0x09),
            (InstallerStatus::Failed, 0x0a),
        ] {
            let mut transport = FakeTransport::successful();
            report_with_transport(status, &mut transport).unwrap();
            assert_eq!(transport.requests.len(), 1);
            assert_eq!(transport.requests[0].address_type, 0x0c);
            assert_eq!(transport.requests[0].channel, 0x0f);
            assert_eq!(transport.requests[0].lun, 0);
            assert_eq!(transport.requests[0].netfn, 0x04);
            assert_eq!(transport.requests[0].cmd, 0x02);
            assert_eq!(
                transport.requests[0].data,
                [0x41, 0x04, 0x1f, 0x00, 0x6f, event, 0x00, 0x00]
            );
        }
    }

    #[test]
    fn successful_response_reports_sent() {
        assert_eq!(
            report_with_opener(InstallerStatus::Completed, || {
                Ok(FakeTransport::successful())
            })
            .unwrap(),
            ReportOutcome::Sent
        );
    }

    #[test]
    fn missing_device_reports_unavailable() {
        assert_eq!(
            report_with_opener::<FakeTransport>(InstallerStatus::Started, || {
                Err(io::Error::from(io::ErrorKind::NotFound))
            })
            .unwrap(),
            ReportOutcome::Unavailable
        );
    }

    #[test]
    fn real_errors_are_propagated() {
        let open = report_with_opener::<FakeTransport>(InstallerStatus::Started, || {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert!(format!("{open:#}").contains("Failed to open /dev/ipmi0"));

        let mut transport = FakeTransport {
            send_error: true,
            ..FakeTransport::default()
        };
        let send = report_with_transport(InstallerStatus::Started, &mut transport).unwrap_err();
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
        let error = report_with_transport(InstallerStatus::Failed, &mut transport).unwrap_err();
        assert!(format!("{error:#}").contains("completion code 0xc1"));
    }

    #[test]
    fn timeout_is_an_error() {
        let mut transport = FakeTransport::default();
        let error = report_with_transport(InstallerStatus::Started, &mut transport).unwrap_err();
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
        report_with_transport(InstallerStatus::Aborted, &mut transport).unwrap();
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
            let error =
                report_with_transport(InstallerStatus::Completed, &mut transport).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }
    }
}
