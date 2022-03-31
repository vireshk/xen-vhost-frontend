// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

mod i2c;
mod xc;
mod xs;
mod xec;
mod xfm;
mod xdm;
mod xgm;
mod mmio;

use libxen_sys::*;
use std::io::{self};
use std::num::ParseIntError;
use std::sync::{Arc, RwLock};
use std::{thread::{spawn, JoinHandle}, str};
use thiserror::Error as ThisError;

use vhost_user_master::device::Generic;
use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use xs::{XsDev, XsReadWatch};
use xec::XenEvtChnHandle;
use xfm::XenForeignMemory;
use xdm::XenDeviceModel;
use xgm::XenGuestMem;
use mmio::XenMmio;

/// Result of libgpiod operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error codes for libgpiod operations
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Failed to connect to xenstore")]
    XsOpenFailed,
    #[error("Failed to get Backend DomId")]
    XsBeDomIdFailed,
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
    #[error("Xs directory failed")]
    XsDirectoryFailed,
    #[error("Xs read failed")]
    XsReadFailed,
    #[error("Xs watch failed")]
    XsWatchFailed,
    #[error("Xs Error")]
    XsError,
    #[error("Xs Invalid State")]
    XsInvalidState,
    #[error("Failed to kick backend: {0:?}")]
    EventFdWriteFailed(io::Error),
}

pub struct XenState {
    xsd: XsDev,
    xec: XenEvtChnHandle,
    xfm: XenForeignMemory,
    xdm: XenDeviceModel,
    xgm: XenGuestMem,
    mmio: XenMmio,
}

unsafe impl Send for XenState {}
unsafe impl Sync for XenState {}

impl XenState {
    pub fn new() -> Result <Self> {
        let mut xsd = XsDev::new("i2c")?;
        xsd.get_be_domid()?;
        xsd.wait_for_fe_domid()?;
        xsd.connect_dom()?;

        println!("Backend domid {}, Frontend domid {}", xsd.be_domid(), xsd.fe_domid());

        let mut xdm = XenDeviceModel::new(xsd.fe_domid())?;
        xdm.create_ioreq_server()?;

        let mut xfm = XenForeignMemory::new()?;
        xfm.map_resource(xsd.fe_domid(), xdm.ioserver_id())?;
        xdm.set_ioreq_server_state(1)?;

        let mut xec = XenEvtChnHandle::new()?;
        xec.bind(&xfm, xsd.fe_domid(), xdm.vcpus())?;

        let xgm = XenGuestMem::new(&mut xfm, xsd.fe_domid())?;
        let mmio = XenMmio::new(&mut xdm, xsd.addr() as u64, xsd.irq())?;

        Ok(Self {xsd, xec, xfm, xdm, xgm, mmio})
    }

    fn handle_be_state_change(&self) -> Result<()> {
        let state = self.xsd.read_be_int("state")?;

        if state == xenbus_state_XenbusStateUnknown {
            Err(Error::XsError)
        } else {
            Ok(())
        }
    }

    fn handle_fe_state_change(&self) -> Result<()> {
        let state = self.xsd.read_fe_int("state")?;

        if state == xenbus_state_XenbusStateInitialising {
            Ok(())
        } else if state == xenbus_state_XenbusStateUnknown {
            Err(Error::XsError)
        } else {
            panic!();
        }
    }

    fn handle_io_event(&mut self) -> Result<()> {
        let (port, cpu) = self.xec.pending()?;
        self.xec.unmask(port)?;

        let ioreq = self.xfm.ioreq(cpu)?;
        if ioreq.state() != STATE_IOREQ_READY as u8 {
            return Ok(())
        }

        unsafe { xen_mb() };
        ioreq.set_state(STATE_IOREQ_INPROCESS as u8);

        self.mmio.handle_ioreq(&self.xgm, ioreq)?;
        unsafe { xen_mb() };

        ioreq.set_state(STATE_IORESP_READY as u8);

        unsafe { xen_mb() };
        self.xec.notify(port)?;

        Ok(())
    }

    fn handle_xen_store_event(&mut self) -> Result<()> {
        let watch = XsReadWatch::new(&self.xsd)?;
        let name = watch.data(xs_watch_type_XS_WATCH_TOKEN)?;

        if self.xsd.be().eq(name) {
            self.handle_be_state_change()
        } else if self.xsd.fe().eq(name) {
            self.handle_fe_state_change()
        } else {
            Ok(())
        }
    }
}

pub fn handle_events(state: Arc<RwLock<XenState>>) -> Result<JoinHandle<()>> {
    let efd = state.read().unwrap().xec.fd()?;
    let xfd = state.read().unwrap().xsd.fileno()?;

    Ok(spawn(move || {
        let epoll = Epoll::new().map_err(Error::EpollCreateFd).unwrap();
        epoll
            .ctl(
                ControlOperation::Add,
                efd as i32,
                EpollEvent::new(EventSet::IN | EventSet::ERROR | EventSet::HANG_UP, efd as u64),
                ).unwrap();

        epoll
            .ctl(
                ControlOperation::Add,
                xfd,
                EpollEvent::new(EventSet::IN | EventSet::ERROR | EventSet::HANG_UP, xfd as u64),
                ).unwrap();

        let mut events = vec![EpollEvent::new(EventSet::empty(), 0); 10];

        loop {
            match epoll.wait(-1, &mut events[..]) {
                Ok(num) => {
                    for event in events.iter().take(num) {
                        let evset = event.event_set();
                        if evset & EventSet::IN == EventSet::empty() {
                            continue;
                        }

                        let fd = event.fd();
                        if fd == efd as i32 {
                            match state.write().unwrap().handle_io_event() {
                                _ => continue,
                            }
                        } else if fd == xfd {
                            if state.write().unwrap().handle_xen_store_event().is_err() {
                                return;
                            }
                        } else {
                            panic!();
                        }
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
                        return;
                    }
                }
            }
        }
    }))
}

pub fn handle_interrupt(state: Arc<RwLock<XenState>>) -> JoinHandle<()> {
    let vq = 0;
    let call = state.read().unwrap().mmio.get_call(vq);

    spawn(move || loop {
        // Wait for backend to notify
        while call.read().is_err() {}

        let mut state = state.write().unwrap();

        // Update interrupt state
        state.mmio.update_interrupt_state(VIRTIO_MMIO_INT_VRING);

        // Forward the interrupt to guest
        state.xdm.set_irq(state.mmio.irq() as u32).unwrap();
    })
}

fn dev_init(state: Arc<RwLock<XenState>>) -> Generic {
    let state = state.read().unwrap();

    let (vaddr, addr, size) = state.xgm.addr_and_size();
    let kick = state.mmio.get_kick(0);
    let call = state.mmio.get_call(0);
    let vring = state.mmio.get_vring(0);
    drop(state);

    i2c::initialize(kick, call, vring, vaddr, addr, size)
}

fn main() {
    let state = Arc::new(RwLock::new(XenState::new().unwrap()));
    let mut handles = Vec::new();

    handles.push(handle_events(state.clone()).unwrap());
    handles.push(handle_interrupt(state.clone()));

    // Hack, we wait here until fully initialized.
    let ready = EventFd::new(EFD_NONBLOCK).unwrap();
    state.write().unwrap().mmio.set_ready(ready.try_clone().unwrap());
    while ready.read().is_err() {}

    let _device = dev_init(state.clone());

    for handle in handles {
        handle.join().unwrap();
    }
}
