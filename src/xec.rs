// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use super::{xfm::XenForeignMemory, Error, Result};
use xen_ioctls::XenEventChannelHandle;

pub struct XenEventChannel {
    channel: XenEventChannelHandle,
    ports: Vec<u32>,
}

impl XenEventChannel {
    pub fn new() -> Result<Self> {
        let channel = XenEventChannelHandle::new().map_err(Error::XenIoctlError)?;

        Ok(Self {
            channel,
            ports: Vec::new(),
        })
    }

    pub fn ports(&self) -> &[u32] {
        &self.ports
    }

    pub fn bind(&mut self, xfm: &XenForeignMemory, domid: u16, vcpus: u32) -> Result<()> {
        for cpu in 0..vcpus {
            let ioreq = xfm.ioreq(cpu)?;

            self.ports.push(
                self.channel
                    .bind_interdomain(domid as u32, ioreq.vp_eport)
                    .map_err(Error::XenIoctlError)?,
            );
        }
        Ok(())
    }

    pub fn unbind(&self) {
        for port in &self.ports {
            if self.channel.unbind(*port).is_err() {
                println!("XenEventChannel: Failed to unbind port: {}", *port);
            }
        }
    }

    pub fn fd(&self) -> Result<u32> {
        Ok(self.channel.fd().map_err(Error::XenIoctlError)? as u32)
    }

    pub fn pending(&mut self) -> Result<(u32, u32)> {
        let port = self.channel.pending().map_err(Error::XenIoctlError)?;
        let cpu = self.ports.iter().position(|&x| x == port).unwrap();
        Ok((port, cpu as u32))
    }

    pub fn unmask(&mut self, port: u32) -> Result<()> {
        self.channel.unmask(port).map_err(Error::XenIoctlError)
    }

    pub fn notify(&self, port: u32) -> Result<()> {
        self.channel.notify(port).map_err(Error::XenIoctlError)?;
        Ok(())
    }
}

impl Drop for XenEventChannel {
    fn drop(&mut self) {
        self.unbind();
    }
}
