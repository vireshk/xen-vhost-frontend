// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

mod device;
mod epoll;
mod frontend;
mod guest;
mod interrupt;
mod mmio;
mod supported_devices;
mod xdm;
mod xec;
mod xfm;
mod xgm;
mod xs;

use std::{io, num::ParseIntError, str, thread::Builder};

use frontend::XenFrontend;
use xs::XsHandle;

pub const BACKEND_PATH: &str = "backend/virtio";

/// Result for xen-vhost-frontend operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error codes for xen-vhost-frontend operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid Domain info, len {0:?}, domid expected {1:?} info length {2:?}")]
    InvalidDomainInfo(usize, u16, usize),
    #[error("Invalid MMIO {0:} Address {1:?}")]
    InvalidMmioAddr(&'static str, u64),
    #[error("MMIO Legacy not supported by Guest")]
    MmioLegacyNotSupported,
    #[error("Invalid feature select {0:}")]
    InvalidFeatureSel(u32),
    #[error("Invalid MMIO direction {0:}")]
    InvalidMmioDir(u8),
    #[error("Device not supported: {0:}")]
    XenDevNotSupported(String),
    #[error("Xen foreign memory failure")]
    XenForeignMemoryFailure,
    #[error("Xen foreign memory failure: {0:?}")]
    XenIoctlError(io::Error),
    #[error("Vhost user frontend error")]
    VhostFrontendError(vhost_user_frontend::Error),
    #[error("Vhost user frontend activate error")]
    VhostFrontendActivateError(vhost_user_frontend::ActivateError),
    #[error("Invalid String: {0:?}")]
    InvalidString(str::Utf8Error),
    #[error("Failed while parsing to integer: {0:?}")]
    ParseFailure(ParseIntError),
    #[error("Failed to create epoll context: {0:?}")]
    EpollCreateFd(io::Error),
    #[error("Failed to open XS file")]
    FileOpenFailed,
    #[error("Failed to add event to epoll: {0:?}")]
    RegisterExitEvent(io::Error),
    #[error("Failed while waiting on epoll: {0:?}")]
    EpollWait(io::Error),
    #[error("Xen Bus Invalid State")]
    XBInvalidState,
    #[error("Failed to kick backend: {0:?}")]
    EventFdWriteFailed(io::Error),
}

fn main() -> Result<()> {
    let frontend = XenFrontend::new()?;
    let mut xsh = XsHandle::new_with_epoll()?;
    xsh.create_watch(BACKEND_PATH.to_string(), BACKEND_PATH.to_string())?;

    loop {
        let (fe_domid, dev_id, new) = xsh.wait_for_device()?;

        // Handle events in individual threads, in order to support multiple
        // devices / guests.
        let f = frontend.clone();
        frontend.push(
            Builder::new()
                .name(format!("frontend {} - {}", fe_domid, dev_id))
                .spawn(move || {
                    if new {
                        f.add_device(fe_domid, dev_id).unwrap();
                    } else {
                        f.remove_device(fe_domid, dev_id);
                    }
                })
                .unwrap(),
        );
    }
}
