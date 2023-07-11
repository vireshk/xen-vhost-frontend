// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use vmm_sys_util::eventfd::EventFd;

use super::{Error, Result};
use xen_ioctls::{XenDeviceModelHandle, HVM_IOREQSRV_BUFIOREQ_OFF};

pub const VIRTIO_IRQ_HIGH: u32 = 1;

pub struct XenDeviceModel {
    xdmh: XenDeviceModelHandle,
    id: Option<u16>,
    domid: u16,
    vcpus: u32,
}

impl XenDeviceModel {
    pub fn new(domid: u16) -> Result<Self> {
        let xdmh = XenDeviceModelHandle::new().map_err(Error::XenIoctlError)?;

        // Create the domain struct earlier so Drop can be called in case of errors.
        let mut xdm = Self {
            xdmh,
            id: None,
            domid,
            vcpus: 0,
        };

        xdm.vcpus = xdm.xdmh.nr_vcpus(domid).map_err(Error::XenIoctlError)?;

        Ok(xdm)
    }

    pub fn ioserver_id(&self) -> u16 {
        self.id.unwrap()
    }

    pub fn vcpus(&self) -> u32 {
        self.vcpus
    }

    pub fn create_ioreq_server(&mut self) -> Result<()> {
        self.id = Some(
            self.xdmh
                .create_ioreq_server(self.domid, HVM_IOREQSRV_BUFIOREQ_OFF)
                .map_err(Error::XenIoctlError)?,
        );

        Ok(())
    }

    fn destroy_ioreq_server(&mut self) -> Result<()> {
        if let Some(id) = self.id.take() {
            self.xdmh
                .destroy_ioreq_server(self.domid, id)
                .map_err(Error::XenIoctlError)
        } else {
            Ok(())
        }
    }

    pub fn set_ioreq_server_state(&self, enabled: i32) -> Result<()> {
        self.xdmh
            .set_ioreq_server_state(self.domid, self.ioserver_id(), enabled)
            .map_err(Error::XenIoctlError)
    }

    pub fn map_io_range_to_ioreq_server(&mut self, start: u64, size: u64) -> Result<()> {
        let end = start + size - 1;

        self.xdmh
            .map_io_range_to_ioreq_server(self.domid, self.ioserver_id(), 1, start, end)
            .map_err(Error::XenIoctlError)
    }

    pub fn ummap_io_range_from_ioreq_server(&self, start: u64, size: u64) -> Result<()> {
        let end = start + size - 1;

        self.xdmh
            .unmap_io_range_from_ioreq_server(self.domid, self.ioserver_id(), 1, start, end)
            .map_err(Error::XenIoctlError)
    }

    pub fn set_irqfd(&self, fd: EventFd, irq: u32, set: bool) -> Result<()> {
        if set {
            self.xdmh
                .set_irqfd(fd, self.domid, irq, VIRTIO_IRQ_HIGH as u8)
        } else {
            self.xdmh
                .clear_irqfd(fd, self.domid, irq, VIRTIO_IRQ_HIGH as u8)
        }
        .map_err(Error::XenIoctlError)
    }
}

impl Drop for XenDeviceModel {
    fn drop(&mut self) {
        self.set_ioreq_server_state(0).ok();
        self.destroy_ioreq_server().ok();
    }
}
