// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::str;

use xen_bindings::bindings::{xs_watch_type, xs_watch_type_XS_WATCH_PATH};
use xen_store::XenStoreHandle;

use super::{epoll::XenEpoll, Error, Result, BACKEND_PATH};

use xen_bindings::bindings::{
    xenbus_state_XenbusStateInitWait, xenbus_state_XenbusStateInitialising,
    xenbus_state_XenbusStateUnknown,
};

pub struct XsHandle {
    handle: XenStoreHandle,
    epoll: Option<XenEpoll>,
}

impl XsHandle {
    pub fn new() -> Result<Self> {
        Ok(Self {
            handle: XenStoreHandle::new().map_err(Error::XenIoctlError)?,
            epoll: None,
        })
    }

    pub fn new_with_epoll() -> Result<Self> {
        let mut xsh = Self::new()?;
        xsh.epoll = Some(XenEpoll::new(vec![xsh.fileno()?])?);

        Ok(xsh)
    }

    pub fn read_str(&self, base: &str, node: &str) -> Result<String> {
        self.handle
            .read_str(format!("{}/{}", base, node).as_str())
            .map_err(Error::XenIoctlError)
    }

    fn write_str(&self, base: &str, node: &str, val: &str) -> Result<()> {
        self.handle
            .write_str(format!("{}/{}", base, node).as_str(), val)
            .map_err(Error::XenIoctlError)
    }

    pub fn read_int(&self, base: &str, node: &str) -> Result<u32> {
        let res = self.read_str(base, node)?;

        match res.strip_prefix("0x") {
            Some(x) => u32::from_str_radix(x, 16),
            None => res.parse::<u32>(),
        }
        .map_err(Error::ParseFailure)
    }

    fn write_int(&self, base: &str, node: &str, val: u32) -> Result<()> {
        let val_str = format!("{}", val);

        self.write_str(base, node, &val_str)
    }

    pub fn fileno(&self) -> Result<i32> {
        self.handle.fileno().map_err(Error::XenIoctlError)
    }

    fn wait_state(&self, base: &str, state: u32) -> Result<u32> {
        let state = state | 1 << xenbus_state_XenbusStateUnknown;

        loop {
            let val = self.read_int(base, "state")?;

            if ((1 << val) & state) != 0 {
                return Ok(val);
            }

            self.read_path()?;
        }
    }

    pub fn create_watch(&mut self, path: String, token: String) -> Result<()> {
        self.handle
            .create_watch(path.as_str(), token.as_str())
            .map_err(Error::XenIoctlError)
    }

    pub fn read_watch(&self, index: xs_watch_type) -> Result<String> {
        self.handle.read_watch(index).map_err(Error::XenIoctlError)
    }

    pub fn read_path(&self) -> Result<String> {
        self.read_watch(xs_watch_type_XS_WATCH_PATH)
    }

    pub fn connect_dom(&mut self, dev_id: u32, fe_domid: u16) -> Result<String> {
        let be = format!("{}/{}/{}", BACKEND_PATH, fe_domid, dev_id);

        let state = self.read_int(&be, "state")?;
        if state != xenbus_state_XenbusStateInitialising {
            return Err(Error::XBInvalidState);
        }
        self.write_int(&be, "state", xenbus_state_XenbusStateInitWait)?;

        let fe = self.read_str(&be, "frontend")?;
        let state = self.read_int(&fe, "state")?;
        if state != xenbus_state_XenbusStateInitialising {
            return Err(Error::XBInvalidState);
        }

        self.create_watch(be.clone(), be.clone())?;
        self.create_watch(fe.clone(), fe)?;

        let state = self.wait_state(&be, 1 << xenbus_state_XenbusStateInitWait)?;
        if state != xenbus_state_XenbusStateInitWait {
            return Err(Error::XBInvalidState);
        }

        Ok(be)
    }

    pub fn wait_for_device(&mut self) -> Result<(u16, u32, bool)> {
        loop {
            self.epoll.as_ref().unwrap().wait()?;

            let path = self.read_path()?;
            let list: Vec<&str> = path.split('/').collect();

            // Only parse events where path matches "BACKEND_PATH/<Guest Num>/<Device Num>"
            if list.len() == 4 {
                let dev_id = list[3].parse::<u32>().map_err(Error::ParseFailure)?;
                let fe_domid = list[2].parse::<u16>().map_err(Error::ParseFailure)?;

                let new = matches!(
                    self.read_str(BACKEND_PATH, format!("{}/{}", fe_domid, dev_id).as_str()),
                    Ok(_)
                );

                return Ok((fe_domid, dev_id, new));
            }
        }
    }
}
