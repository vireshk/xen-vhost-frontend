// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::ptr;

use libxen_sys::*;
use super::{xfm::XenForeignMemory, Error, Result};

pub struct XenEvtChnHandle {
    xeh: *mut xenevtchn_handle,
    ports: Vec<evtchn_port_t>,
}

impl XenEvtChnHandle {
    pub fn new() -> Result<Self> {
        let xeh = unsafe {
            xenevtchn_open(ptr::null_mut::<xentoollog_logger>(), 0)
        };

        if xeh.is_null() {
            return Err(Error::XsError);
        }

        Ok (Self {
            xeh,
            ports: Vec::new(),
        })
    }

    pub fn bind(&mut self, xfm: &XenForeignMemory, domid: domid_t, vcpus: u32) -> Result<()> {
        for cpu in 0..vcpus {
            let ioreq = xfm.ioreq(cpu)?;
            let local_port = unsafe { xenevtchn_bind_interdomain(self.xeh, domid as u32, ioreq.vp_eport) };
            if local_port < 0 {
                return Err(Error::XsError);
            } else {
                self.ports.push(local_port as evtchn_port_t);
            }
        }
        Ok(())
    }

    pub fn unbind(&self) {
        for port in &self.ports {
            let ret = unsafe { xenevtchn_unbind(self.xeh, *port) };
            if ret < 0 {
                println!("XenEvtChnHandle: Failed to unbind port: {}", port);
            }
        }
    }

    pub fn fd(&self) -> Result<u32> {
        let fd = unsafe { xenevtchn_fd(self.xeh) };
        if fd < 0 {
            return Err(Error::XsError);
        } else {
            Ok(fd as u32)
        }
    }

    pub fn pending(&self) -> Result <(u32, u32)> {
        let port = unsafe { xenevtchn_pending(self.xeh) };
        if port < 0 {
            return Err(Error::XsError);
        } else {
            let cpu = self.ports.iter().position(|&x| x == port as u32).unwrap();
            Ok((port as u32, cpu as u32))
        }
    }

    pub fn unmask(&self, port: u32) -> Result <()> {
        let ret = unsafe { xenevtchn_unmask(self.xeh, port) };
        if ret  < 0 {
            Err(Error::XsError)
        } else {
            Ok(())
        }
    }

    pub fn notify(&self, port: u32) -> Result <()> {
        let ret = unsafe { xenevtchn_notify(self.xeh, port) };
        if ret  < 0 {
            Err(Error::XsError)
        } else {
            Ok(())
        }
    }
}

impl Drop for XenEvtChnHandle {
    fn drop(&mut self) {
        self.unbind();
        unsafe{ xenevtchn_close(self.xeh); }
    }
}
