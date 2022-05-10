// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use libc::free;
use std::ffi::{CStr, CString};
use std::io;
use std::os::raw::{c_char, c_void};
use std::{slice, str, thread, time};

use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};

use super::{Error, Result};
use libxen_sys::{
    domid_t, strlen, xenbus_state_XenbusStateInitWait, xenbus_state_XenbusStateInitialising,
    xenbus_state_XenbusStateUnknown, xs_close, xs_directory, xs_fileno, xs_handle, xs_open,
    xs_read, xs_read_watch, xs_transaction_t, xs_unwatch, xs_watch, xs_watch_type,
    xs_watch_type_XS_WATCH_PATH, xs_write, XenbusState, XBT_NULL,
};

struct XsWatch {
    xsh: *mut xs_handle,
    path: CString,
    token: CString,
}

impl XsWatch {
    fn new(dev: &XsDev, path: CString, token: CString) -> Result<Self> {
        let ret = unsafe { xs_watch(dev.xsh(), path.as_ptr(), token.as_ptr()) };

        if !ret {
            return Err(Error::XsWatchFailed);
        }

        Ok(Self {
            xsh: dev.xsh(),
            path,
            token,
        })
    }
}

impl Drop for XsWatch {
    fn drop(&mut self) {
        unsafe {
            xs_unwatch(self.xsh, self.path.as_ptr(), self.token.as_ptr());
        }
    }
}

pub struct XsReadWatch {
    buf: *mut *mut c_char,
    num: u32,
}

impl XsReadWatch {
    pub fn new(dev: &XsDev) -> Result<Self> {
        let mut num: u32 = 0;
        let buf = unsafe { xs_read_watch(dev.xsh, &mut num) };
        if buf.is_null() {
            return Err(Error::XsError);
        }

        Ok(Self { buf, num })
    }

    pub fn buf(&self) -> *mut *mut c_char {
        self.buf
    }

    pub fn num(&self) -> u32 {
        self.num
    }

    pub fn data(&self, index: xs_watch_type) -> Result<&str> {
        // SAFETY: The array is guaranteed to be valid here.
        let vec = unsafe { slice::from_raw_parts(self.buf(), self.num() as usize) };

        // SAFETY: The string is guaranteed to be valid here.
        let path = str::from_utf8(unsafe {
            slice::from_raw_parts(
                vec[index as usize] as *const u8,
                strlen(vec[index as usize] as *const u8) as usize,
            )
        })
        .map_err(Error::InvalidString)?;

        Ok(path)
    }
}

impl Drop for XsReadWatch {
    fn drop(&mut self) {
        unsafe { free(self.buf as *mut c_void) }
    }
}

struct XsDirectory {
    buf: *mut *mut c_char,
    num: u32,
}

impl XsDirectory {
    pub fn new(dev: &XsDev, path: &CStr) -> Result<Self> {
        let mut num: u32 = 0;
        let buf = unsafe { xs_directory(dev.xsh, XBT_NULL, path.as_ptr(), &mut num) };
        if buf.is_null() {
            return Err(Error::XsDirectoryFailed);
        }

        Ok(Self { buf, num })
    }

    pub fn num(&self) -> u32 {
        self.num
    }

    pub fn entries(&self) -> Result<Vec<i32>> {
        let mut values = Vec::new();

        // SAFETY: The string is guaranteed to be valid here.
        let entries =
            unsafe { slice::from_raw_parts(self.buf as *mut *mut c_char, self.num as usize) };

        for entry in entries {
            // SAFETY: The string is guaranteed to be valid here.
            let buf = str::from_utf8(unsafe {
                slice::from_raw_parts(*entry as *const u8, strlen(*entry as *const u8) as usize)
            })
            .map_err(Error::InvalidString)?;

            values.push(buf.parse::<i32>().map_err(Error::ParseFailure)?);
        }

        Ok(values)
    }
}

impl Drop for XsDirectory {
    fn drop(&mut self) {
        unsafe { free(self.buf as *mut c_void) }
    }
}

struct XsRead {
    buf: *mut c_void,
    len: u32,
}

impl XsRead {
    pub fn new(dev: &XsDev, transaction: xs_transaction_t, path: &CStr) -> Result<Self> {
        let mut len: u32 = 0;
        let buf = unsafe { xs_read(dev.xsh, transaction, path.as_ptr(), &mut len) };
        if buf.is_null() {
            return Err(Error::XsReadFailed);
        }

        Ok(Self { buf, len })
    }

    pub fn buf(&self) -> *mut c_void {
        self.buf
    }

    pub fn len(&self) -> u32 {
        self.len
    }
}

impl Drop for XsRead {
    fn drop(&mut self) {
        unsafe { free(self.buf) }
    }
}

pub struct XsDev {
    xsh: *mut xs_handle,
    be_domid: domid_t,
    fe_domid: domid_t,
    dev_id: i32,
    dev_name: String,
    be: String,
    fe: String,
    be_state: u32,
    path: CString,
    addr: u32,
    irq: u8,
    be_watch: Option<XsWatch>,
    fe_watch: Option<XsWatch>,
}

impl XsDev {
    pub fn new(dev_name: String) -> Result<Self> {
        let xsh = unsafe { xs_open(0) };
        if xsh.is_null() {
            return Err(Error::XsOpenFailed);
        }

        Ok(Self {
            xsh,
            be_domid: 0,
            fe_domid: 0,
            dev_id: 0,
            dev_name: dev_name.clone(),
            be: "".to_string(),
            fe: "".to_string(),
            be_state: 0,
            path: CString::new(format!("backend/{}", dev_name)).unwrap(),
            addr: 0,
            irq: 0,
            be_watch: None,
            fe_watch: None,
        })
    }

    pub fn xsh(&self) -> *mut xs_handle {
        self.xsh
    }

    pub fn be_domid(&self) -> domid_t {
        self.be_domid
    }

    pub fn fe_domid(&self) -> domid_t {
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

    pub fn read_str_raw(&self, transaction: xs_transaction_t, path: &CStr) -> Result<String> {
        let read = XsRead::new(self, transaction, path)?;

        // SAFETY: The string is guaranteed to be valid here.
        let val = str::from_utf8(unsafe {
            slice::from_raw_parts(read.buf() as *const u8, read.len() as usize)
        })
        .map_err(Error::InvalidString)?;

        let val = val.to_string();

        Ok(val)
    }

    pub fn read_str(&self, base: &str, node: &str) -> Result<String> {
        let path = CString::new(format!("{}/{}", base, node)).unwrap();

        self.read_str_raw(0, &path)
    }

    pub fn write_str(&self, base: &str, node: &str, val: &str) -> Result<()> {
        let path = CString::new(format!("{}/{}", base, node)).unwrap();
        let val = CString::new(val).unwrap();

        match unsafe {
            xs_write(
                self.xsh,
                0,
                path.as_ptr(),
                val.as_ptr() as *const c_void,
                strlen(val.as_ptr()) as u32,
            )
        } {
            true => Ok(()),
            false => Err(Error::XsError),
        }
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

    pub fn set_be_state(&mut self, state: XenbusState) -> Result<()> {
        self.write_be_int("state", state)?;
        self.be_state = state;
        Ok(())
    }

    pub fn fileno(&self) -> Result<i32> {
        let fd = unsafe { xs_fileno(self.xsh) };
        if fd < 0 {
            Err(Error::FileOpenFailed)
        } else {
            Ok(fd)
        }
    }

    pub fn wait_be_state(&self, state: XenbusState) -> Result<u32> {
        let state = state | 1 << xenbus_state_XenbusStateUnknown;

        loop {
            let val = self.read_be_int("state")?;

            if ((1 << val) & state) != 0 {
                return Ok(val);
            }

            XsReadWatch::new(self)?;
        }
    }

    pub fn get_be_domid(&mut self) -> Result<()> {
        let name = CString::new("domid").unwrap();

        let id = self.read_str_raw(XBT_NULL, &name)?;
        self.be_domid = id.parse::<u16>().map_err(Error::ParseFailure)?;

        Ok(())
    }

    fn update_fe_domid(&mut self) -> Result<()> {
        let watch = XsReadWatch::new(self)?;
        if !self
            .path
            .eq(&CString::new(watch.data(xs_watch_type_XS_WATCH_PATH)?).unwrap())
        {
            return Err(Error::XsError);
        }

        let directory = XsDirectory::new(self, &self.path)?;
        let values = directory.entries()?;

        for id in values {
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

        let path = CString::new(format!("backend/{}/{}", self.dev_name, self.fe_domid)).unwrap();

        let directory = XsDirectory::new(self, &path)?;
        if directory.num() > 1 {
            println!(
                "got {} devices, but only single device is supported\n",
                directory.num(),
            );
        }

        self.dev_id = directory.entries()?[0];

        let path = CString::new(format!(
            "/local/domain/{}/device/{}/{}",
            self.fe_domid, self.dev_name, self.dev_id,
        ))
        .unwrap();
        match self.read_str_raw(XBT_NULL, &path) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.dev_id = 0;
                Err(e)
            }
        }
    }

    pub fn get_fe_domid(&mut self) -> Result<()> {
        let _watch = XsWatch::new(self, self.path.clone(), self.path.clone())?;

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

        let be = CString::new(self.be.clone()).unwrap();
        self.be_watch = Some(XsWatch::new(self, be.clone(), be)?);

        let fe = CString::new(self.fe.clone()).unwrap();
        self.fe_watch = Some(XsWatch::new(self, fe.clone(), fe)?);

        let state = self.wait_be_state(1 << xenbus_state_XenbusStateInitWait)?;
        if state != xenbus_state_XenbusStateInitWait {
            return Err(Error::XsInvalidState);
        }

        self.addr = self.read_be_int("base")?;
        self.irq = self.read_be_int("irq")? as u8;

        Ok(())
    }
}

impl Drop for XsDev {
    fn drop(&mut self) {
        unsafe { xs_close(self.xsh) }
    }
}
