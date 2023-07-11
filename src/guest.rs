// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    os::unix::io::AsRawFd,
    sync::{atomic::fence, atomic::Ordering, Arc, Mutex},
    thread::{Builder, JoinHandle},
};

use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};
use xen_bindings::bindings::{
    ioreq, IOREQ_TYPE_COPY, IOREQ_TYPE_INVALIDATE, STATE_IOREQ_INPROCESS, STATE_IOREQ_READY,
    STATE_IORESP_READY,
};

use super::{
    device::XenDevice, epoll::XenEpoll, xdm::XenDeviceModel, xec::XenEventChannel,
    xfm::XenForeignMemory, Result,
};

#[derive(Default)]
struct GuestDevices(Vec<Arc<XenDevice>>);

impl GuestDevices {
    fn push(&mut self, dev: Arc<XenDevice>) {
        self.0.push(dev);
    }

    fn remove(&mut self, dev_id: u32) -> Arc<XenDevice> {
        self.0
            .remove(self.0.iter().position(|dev| dev.dev_id == dev_id).unwrap())
    }

    fn io_event(&self, ioreq: &mut ioreq) -> Result<()> {
        for dev in &self.0 {
            if ioreq.addr >= dev.addr && ioreq.addr < dev.addr + 0x200 {
                dev.io_event(ioreq)?;
                return Ok(());
            }
        }

        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub struct XenGuest {
    pub xdm: Mutex<XenDeviceModel>,
    pub xec: Mutex<XenEventChannel>,
    pub xfm: Mutex<XenForeignMemory>,
    pub fe_domid: u16,
    devices: Mutex<GuestDevices>,
    handle: Mutex<Option<JoinHandle<()>>>,
    exit: EventFd,
}

// SAFETY: Safe as the fields are protected with Mutex.
unsafe impl Send for XenGuest {}
// SAFETY: Safe as the fields are protected with Mutex.
unsafe impl Sync for XenGuest {}

impl XenGuest {
    pub fn new(fe_domid: u16) -> Result<Arc<Self>> {
        let mut xdm = XenDeviceModel::new(fe_domid)?;
        xdm.create_ioreq_server()?;

        let mut xfm = XenForeignMemory::new()?;
        xfm.map_resource(fe_domid, xdm.ioserver_id())?;
        xdm.set_ioreq_server_state(1)?;

        let mut xec = XenEventChannel::new()?;
        xec.bind(&xfm, fe_domid, xdm.vcpus())?;

        let guest = Arc::new(Self {
            xdm: Mutex::new(xdm),
            xec: Mutex::new(xec),
            xfm: Mutex::new(xfm),
            fe_domid,
            devices: Mutex::new(GuestDevices::default()),
            handle: Mutex::new(None),
            exit: EventFd::new(EFD_NONBLOCK).unwrap(),
        });

        guest.clone().setup_events()?;
        Ok(guest)
    }

    pub fn add_device(self: Arc<Self>, dev_id: u32) -> Result<Arc<XenDevice>> {
        let dev = XenDevice::new(dev_id, self.clone())?;
        self.devices.lock().unwrap().push(dev.clone());

        println!("Created device {} / {}", self.fe_domid, dev_id);
        Ok(dev)
    }

    pub fn remove_device(&self, dev_id: u32) {
        let dev = self.devices.lock().unwrap().remove(dev_id);

        println!("Removed device {} / {}", self.fe_domid, dev_id);
        dev.exit();
    }

    fn io_event(&self) -> Result<()> {
        let mut xec = self.xec.lock().unwrap();
        let xfm = self.xfm.lock().unwrap();

        let (port, cpu) = xec.pending()?;
        xec.unmask(port)?;

        let ioreq = xfm.ioreq(cpu)?;
        if ioreq.state() != STATE_IOREQ_READY as u8 {
            return Ok(());
        }

        // Memory barrier
        fence(Ordering::SeqCst);

        ioreq.set_state(STATE_IOREQ_INPROCESS as u8);

        match ioreq.type_ as u32 {
            IOREQ_TYPE_COPY => {
                self.devices.lock().unwrap().io_event(ioreq)?;
            }

            IOREQ_TYPE_INVALIDATE => println!("Invalidate Ioreq type is Not implemented"),
            t => println!("Ioreq type unknown: {}", t),
        }

        // Memory barrier
        fence(Ordering::SeqCst);

        ioreq.set_state(STATE_IORESP_READY as u8);

        // Memory barrier
        fence(Ordering::SeqCst);

        xec.notify(port)?;

        Ok(())
    }

    fn setup_events(self: Arc<Self>) -> Result<()> {
        let xfd = self.xec.lock().unwrap().fd()? as i32;
        let efd = self.exit.as_raw_fd();
        let epoll = XenEpoll::new(vec![efd, xfd])?;
        let guest = self.clone();

        *self.handle.lock().unwrap() = Some(
            Builder::new()
                .name(format!("guest {}", self.fe_domid))
                .spawn(move || {
                    while let Ok(fd) = epoll.wait() {
                        // Exit event received
                        if fd == efd {
                            break;
                        }

                        guest.io_event().ok();
                    }
                })
                .unwrap(),
        );

        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.devices.lock().unwrap().is_empty()
    }

    pub fn exit(&self) {
        self.exit.write(1).unwrap();
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }
}
