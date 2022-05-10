// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

mod interrupt;
mod mmio;
mod xdm;
mod xec;
mod xfm;
mod xgm;
mod xs;

use clap::Parser;
use seccompiler::SeccompAction;
use std::{
    io,
    num::ParseIntError,
    str,
    sync::{atomic::fence, atomic::Ordering, Arc, RwLock},
    thread::{spawn, JoinHandle},
};
use thiserror::Error as ThisError;

use vhost_user_master::{
    Generic, VhostUserConfig, VirtioDevice, VirtioDeviceType, VirtioInterrupt,
};
use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use interrupt::{handle_interrupt, XenVirtioInterrupt};
use libxen_sys::{
    domid_t, xenbus_state_XenbusStateInitialising, xenbus_state_XenbusStateUnknown,
    xs_watch_type_XS_WATCH_TOKEN, STATE_IOREQ_INPROCESS, STATE_IOREQ_READY, STATE_IORESP_READY,
};
use mmio::XenMmio;
use xdm::XenDeviceModel;
use xec::XenEvtChnHandle;
use xfm::XenForeignMemory;
use xgm::XenGuestMem;
use xs::{XsDev, XsReadWatch};

/// Result for xen-vhost-master operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error codes for xen-vhost-master operations
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Invalid Domain info, len {0:?}, domid expected {1:?} actual {2:?}")]
    InvalidDomainInfo(usize, domid_t, domid_t),
    #[error("Invalid MMIO {0:} Address {1:?}")]
    InvalidMmioAddr(&'static str, u64),
    #[error("MMIO Legacy not supported by Guest")]
    MmioLegacyNotSupported,
    #[error("Invalid feature select {0:}")]
    InvalidFeatureSel(u32),
    #[error("Invalid MMIO direction {0:}")]
    InvalidMmioDir(u8),
    #[error("Xen device model failure")]
    XenDeviceModelFailure,
    #[error("Xen event channel handle failure")]
    XenEvtChnHandleFailure,
    #[error("Xen foreign memory failure")]
    XenForeignMemoryFailure,
    #[error("Vhost user master error")]
    VhostMasterError(vhost_user_master::Error),
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

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct DeviceArgs {
    /// Location of vhost-user Unix domain socket. This is suffixed by 0,1,2..socket_count-1.
    #[clap(short, long)]
    socket_path: String,

    /// Name of the device.
    #[clap(short, long)]
    name: String,
}

fn create_device() -> Result<Generic> {
    let args = DeviceArgs::parse();
    let device_type = VirtioDeviceType::from(args.name.as_str());

    let vu_cfg = VhostUserConfig {
        device_type,
        socket: args.socket_path,
    };

    let dev = Generic::new(
        vu_cfg,
        SeccompAction::Allow,
        EventFd::new(EFD_NONBLOCK).unwrap(),
    )
    .map_err(Error::VhostMasterError)?;

    Ok(dev)
}

pub struct XenState {
    mmio: XenMmio,
    xdm: XenDeviceModel,
    xec: XenEvtChnHandle,
    xfm: XenForeignMemory,
    xgm: XenGuestMem,
    xsd: XsDev,
}

unsafe impl Send for XenState {}
unsafe impl Sync for XenState {}

impl XenState {
    pub fn new(dev: &Generic) -> Result<Self> {
        let mut xsd = XsDev::new(dev.name())?;
        xsd.get_be_domid()?;
        xsd.get_fe_domid()?;
        xsd.connect_dom()?;

        println!(
            "Backend domid {}, Frontend domid {}",
            xsd.be_domid(),
            xsd.fe_domid(),
        );

        let mut xdm = XenDeviceModel::new(xsd.fe_domid())?;
        xdm.create_ioreq_server()?;

        let mut xfm = XenForeignMemory::new()?;
        xfm.map_resource(xsd.fe_domid(), xdm.ioserver_id())?;
        xdm.set_ioreq_server_state(1)?;

        let mut xec = XenEvtChnHandle::new()?;
        xec.bind(&xfm, xsd.fe_domid(), xdm.vcpus())?;

        let xgm = XenGuestMem::new(&mut xfm, xsd.fe_domid())?;
        let mmio = XenMmio::new(&mut xdm, dev, xsd.addr() as u64)?;

        Ok(Self {
            xsd,
            xec,
            xfm,
            xdm,
            xgm,
            mmio,
        })
    }

    fn handle_be_state_change(&self) -> Result<()> {
        let state = self.xsd.read_be_int("state")?;

        if state == xenbus_state_XenbusStateUnknown {
            Err(Error::XsInvalidState)
        } else {
            Ok(())
        }
    }

    fn handle_fe_state_change(&self) -> Result<()> {
        let state = self.xsd.read_fe_int("state")?;

        if state == xenbus_state_XenbusStateInitialising {
            Ok(())
        } else if state == xenbus_state_XenbusStateUnknown {
            Err(Error::XsInvalidState)
        } else {
            panic!();
        }
    }

    fn handle_io_event(&mut self, dev: Arc<RwLock<Generic>>) -> Result<()> {
        let (port, cpu) = self.xec.pending()?;
        self.xec.unmask(port)?;

        let ioreq = self.xfm.ioreq(cpu)?;
        if ioreq.state() != STATE_IOREQ_READY as u8 {
            return Ok(());
        }

        // Memory barrier
        fence(Ordering::SeqCst);

        ioreq.set_state(STATE_IOREQ_INPROCESS as u8);

        self.mmio
            .handle_ioreq(ioreq, &mut dev.write().unwrap(), &self.xgm)?;

        // Memory barrier
        fence(Ordering::SeqCst);

        ioreq.set_state(STATE_IORESP_READY as u8);

        // Memory barrier
        fence(Ordering::SeqCst);

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

pub fn handle_events(
    state: Arc<RwLock<XenState>>,
    dev: Arc<RwLock<Generic>>,
) -> Result<JoinHandle<()>> {
    let efd = state.read().unwrap().xec.fd()?;
    let xfd = state.read().unwrap().xsd.fileno()?;

    Ok(spawn(move || {
        let epoll = Epoll::new().map_err(Error::EpollCreateFd).unwrap();
        epoll
            .ctl(
                ControlOperation::Add,
                efd as i32,
                EpollEvent::new(
                    EventSet::IN | EventSet::ERROR | EventSet::HANG_UP,
                    efd as u64,
                ),
            )
            .unwrap();

        epoll
            .ctl(
                ControlOperation::Add,
                xfd,
                EpollEvent::new(
                    EventSet::IN | EventSet::ERROR | EventSet::HANG_UP,
                    xfd as u64,
                ),
            )
            .unwrap();

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
                            state.write().unwrap().handle_io_event(dev.clone()).ok();
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

fn activate_device(
    dev: Arc<RwLock<Generic>>,
    state: Arc<RwLock<XenState>>,
    interrupt_cb: Arc<dyn VirtioInterrupt>,
) {
    // Wait for virtqueues to initialize
    let ready = state.write().unwrap().mmio.ready();
    while ready.read().is_err() {}

    let state = state.write().unwrap();
    let mem = state.xgm.mem();
    let kick = state.mmio.get_kick();
    let queues = state.mmio.queues();

    // Drop the lock before activating the device.
    drop(state);

    dev.write()
        .unwrap()
        .activate(mem, interrupt_cb, queues, kick)
        .unwrap();
}

fn main() {
    let dev = Arc::new(RwLock::new(create_device().unwrap()));
    let state = Arc::new(RwLock::new(
        XenState::new(dev.write().as_ref().unwrap()).unwrap(),
    ));
    let interrupt = Arc::new(XenVirtioInterrupt::new(state.clone()));

    let mut handles = vec![handle_events(state.clone(), dev.clone()).unwrap()];
    handles.push(handle_interrupt(interrupt.clone()));

    activate_device(dev, state, interrupt);

    for handle in handles {
        handle.join().unwrap();
    }
}
