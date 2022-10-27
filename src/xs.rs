// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::{str, thread, time};

use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use xen_bindings::bindings::{xs_watch_type, xs_watch_type_XS_WATCH_PATH};
use xen_store::XenStoreHandle;

use super::{Error, Result};

use xen_bindings::bindings::{
    xenbus_state_XenbusStateInitWait, xenbus_state_XenbusStateInitialising,
    xenbus_state_XenbusStateUnknown,
};

pub struct XsDev {
    xsh: XenStoreHandle,
    be_domid: u16,
    fe_domid: u16,
    dev_id: i32,
    dev_name: String,
    be: String,
    fe: String,
    be_state: u32,
    path: String,
    addr: u32,
    irq: u8,
}

impl XsDev {
    pub fn new(dev_name: String) -> Result<Self> {
        let xsh = XenStoreHandle::new().map_err(Error::XenIoctlError)?;

        Ok(Self {
            xsh,
            be_domid: 0,
            fe_domid: 0,
            dev_id: 0,
            dev_name: dev_name.clone(),
            be: "".to_string(),
            fe: "".to_string(),
            be_state: 0,
            path: format!("backend/{}", dev_name),
            addr: 0,
            irq: 0,
        })
    }

    pub fn be_domid(&self) -> u16 {
        self.be_domid
    }

    pub fn fe_domid(&self) -> u16 {
        self.fe_domid
    }

    pub fn addr(&self) -> u32 {
        self.addr
    }

    pub fn irq(&self) -> u8 {
        self.irq
    }

    pub fn be(&self) -> &str {
        &self.be
    }

    pub fn fe(&self) -> &str {
        &self.fe
    }

    pub fn read_str_raw(&self, path: &str) -> Result<String> {
        self.xsh.read_str(path).map_err(Error::XenIoctlError)
    }

    pub fn read_str(&self, base: &str, node: &str) -> Result<String> {
        self.read_str_raw(format!("{}/{}", base, node).as_str())
    }

    pub fn write_str(&self, base: &str, node: &str, val: &str) -> Result<()> {
        self.xsh
            .write_str(format!("{}/{}", base, node).as_str(), val)
            .map_err(|_| Error::XsError)
    }

    pub fn read_be_str(&self, node: &str) -> Result<String> {
        self.read_str(&self.be, node)
    }

    pub fn read_int(&self, base: &str, node: &str) -> Result<u32> {
        let res = self.read_str(base, node)?;

        res.parse::<u32>().map_err(Error::ParseFailure)
    }

    pub fn write_int(&self, base: &str, node: &str, val: u32) -> Result<()> {
        let val_str = format!("{}", val);

        self.write_str(base, node, &val_str)
    }

    pub fn read_fe_int(&self, node: &str) -> Result<u32> {
        self.read_int(&self.fe, node)
    }

    pub fn read_be_int(&self, node: &str) -> Result<u32> {
        self.read_int(&self.be, node)
    }

    pub fn write_be_int(&self, node: &str, val: u32) -> Result<()> {
        self.write_int(&self.be, node, val)
    }

    pub fn set_be_state(&mut self, state: u32) -> Result<()> {
        self.write_be_int("state", state)?;
        self.be_state = state;
        Ok(())
    }

    pub fn fileno(&self) -> Result<i32> {
        self.xsh.fileno().map_err(|_| Error::FileOpenFailed)
    }

    pub fn wait_be_state(&self, state: u32) -> Result<u32> {
        let state = state | 1 << xenbus_state_XenbusStateUnknown;

        loop {
            let val = self.read_be_int("state")?;

            if ((1 << val) & state) != 0 {
                return Ok(val);
            }

            self.read_watch(0).map_err(|_| Error::XsReadFailed)?;
        }
    }

    pub fn get_be_domid(&mut self) -> Result<()> {
        let id = self.read_str_raw("domid")?;
        self.be_domid = id.parse::<u16>().map_err(Error::ParseFailure)?;

        Ok(())
    }

    fn update_fe_domid(&mut self) -> Result<()> {
        let name = self.read_watch(xs_watch_type_XS_WATCH_PATH)?;
        if !self.path.eq(&name) {
            return Err(Error::XsError);
        }

        let directory = self
            .xsh
            .directory(&self.path)
            .map_err(|_| Error::XsDirectoryFailed)?;

        for id in directory {
            if id as u16 > self.fe_domid {
                self.fe_domid = id as u16;
            }
        }

        self.check_fe_exists()
    }

    fn check_fe_exists(&mut self) -> Result<()> {
        // TODO: We need some sign that all devid subdirs are already written to
        // Xenstore, so it's time to parse them. This delay although works, doesn't
        // guarantee that.
        thread::sleep(time::Duration::from_millis(200));

        let path = format!("backend/{}/{}", self.dev_name, self.fe_domid);
        let directory = self
            .xsh
            .directory(path.as_str())
            .map_err(|_| Error::XsDirectoryFailed)?;

        if directory.len() > 1 {
            println!(
                "got {} devices, but only single device is supported\n",
                directory.len(),
            );
        }

        self.dev_id = directory[0];

        match self.read_str_raw(
            format!(
                "/local/domain/{}/device/{}/{}",
                self.fe_domid, self.dev_name, self.dev_id,
            )
            .as_str(),
        ) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.dev_id = 0;
                Err(e)
            }
        }
    }

    pub fn get_fe_domid(&mut self) -> Result<()> {
        self.create_watch(self.path.clone(), self.path.clone())?;

        loop {
            let fd = self.fileno()?;
            let epoll = Epoll::new().map_err(Error::EpollCreateFd)?;
            epoll
                .ctl(
                    ControlOperation::Add,
                    fd,
                    EpollEvent::new(EventSet::IN | EventSet::ERROR | EventSet::HANG_UP, 0),
                )
                .map_err(Error::RegisterExitEvent)?;

            let mut events = vec![EpollEvent::new(EventSet::empty(), 0); 1];

            loop {
                match epoll.wait(-1, &mut events[..]) {
                    Ok(_) => {
                        let evset = events[0].event_set();
                        if evset == EventSet::IN && self.update_fe_domid().is_ok() {
                            return Ok(());
                        } else {
                            thread::sleep(time::Duration::from_millis(100));
                        }
                    }

                    Err(e) => {
                        // It's well defined from the epoll_wait() syscall
                        // documentation that the epoll loop can be interrupted
                        // before any of the requested events occurred or the
                        // timeout expired. In both those cases, epoll_wait()
                        // returns an error of type EINTR, but this should not
                        // be considered as a regular error. Instead it is more
                        // appropriate to retry, by calling into epoll_wait().
                        if e.kind() != io::ErrorKind::Interrupted {
                            return Err(Error::EpollWait(e));
                        }
                    }
                }
            }
        }
    }

    pub fn create_watch(&mut self, path: String, token: String) -> Result<()> {
        self.xsh
            .create_watch(path.as_str(), token.as_str())
            .map_err(|_| Error::XsWatchFailed)
    }

    pub fn read_watch(&self, index: xs_watch_type) -> Result<String> {
        self.xsh.read_watch(index).map_err(|_| Error::XsWatchFailed)
    }

    pub fn connect_dom(&mut self) -> Result<()> {
        // Update be path
        self.be = format!(
            "backend/{}/{}/{}",
            self.dev_name, self.fe_domid, self.dev_id,
        );

        self.be_state = self.read_be_int("state")?;
        if self.be_state != xenbus_state_XenbusStateInitialising {
            return Err(Error::XsInvalidState);
        }

        self.set_be_state(xenbus_state_XenbusStateInitWait)?;
        self.fe = self.read_be_str("frontend")?;

        let state = self.read_fe_int("state")?;
        if state != xenbus_state_XenbusStateInitialising {
            return Err(Error::XsInvalidState);
        }

        self.create_watch(self.be.clone(), self.be.clone())?;
        self.create_watch(self.fe.clone(), self.fe.clone())?;

        let state = self.wait_be_state(1 << xenbus_state_XenbusStateInitWait)?;
        if state != xenbus_state_XenbusStateInitWait {
            return Err(Error::XsInvalidState);
        }

        self.addr = self.read_be_int("base")?;
        self.irq = self.read_be_int("irq")? as u8;

        Ok(())
    }
}
