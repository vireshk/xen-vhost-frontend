// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use libxen_sys::*;
use super::{Error, Result};

pub const VIRTIO_IRQ_HIGH: u32 = 1;

pub struct XenDeviceModel {
    xdh: *mut xendevicemodel_handle,
    id: Option<ioservid_t>,
    domid: domid_t,
    vcpus: u32,
    map_range: Option<(u64, u64)>,
}

impl XenDeviceModel {
    pub fn new(domid: domid_t) -> Result<Self> {
        let xdh = unsafe {
            xendevicemodel_open(std::ptr::null_mut::<xentoollog_logger>(), 0)
        };

        if xdh.is_null() {
            return Err(Error::XsError);
        }

        // Create the domain struct earlier so Drop can be called in case of errors.
        let mut xdm = Self {
            xdh,
            id: None,
            domid,
            vcpus: 0,
            map_range: None,
        };

        let mut num = 0;
        let ret = unsafe { xendevicemodel_nr_vcpus(xdm.xdh, domid, &mut num) };
        if ret < 0 {
            return Err(Error::XsError);
        }

        xdm.vcpus = num;
        Ok (xdm)
    }

    pub fn ioserver_id(&self) -> ioservid_t {
        self.id.unwrap()
    }

    pub fn vcpus(&self) -> u32 {
        self.vcpus
    }

    pub fn create_ioreq_server(&mut self) -> Result<()> {
        let mut id = 0;

        let ret = unsafe { xendevicemodel_create_ioreq_server(self.xdh, self.domid, HVM_IOREQSRV_BUFIOREQ_OFF as i32, &mut id) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            self.id = Some(id);
            Ok(())
        }
    }

    fn destroy_ioreq_server(&mut self) -> Result<()> {
        if self.id.is_none() {
            return Ok(());
        }

        let ret = unsafe { xendevicemodel_destroy_ioreq_server(self.xdh, self.domid, self.ioserver_id()) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            self.id = None;
            Ok(())
        }
    }

    pub fn set_ioreq_server_state(&self, enabled: i32) -> Result<()> {
        let ret = unsafe { xendevicemodel_set_ioreq_server_state(self.xdh, self.domid, self.ioserver_id(), enabled) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            Ok(())
        }
    }

    pub fn map_io_range_to_ioreq_server(&mut self, start: u64, size: u64) -> Result<()> {
        let end = start + size - 1;
        let ret = unsafe { xendevicemodel_map_io_range_to_ioreq_server(self.xdh, self.domid, self.ioserver_id(), 1, start, end) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            self.map_range = Some((start, end));
            Ok(())
        }
    }

    fn ummap_io_range_from_ioreq_server(&self) -> Result<()> {
        if let Some((start, end)) = self.map_range {
            let ret = unsafe { xendevicemodel_unmap_io_range_from_ioreq_server(self.xdh, self.domid, self.ioserver_id(), 1, start, end) };
            if ret < 0 {
                return Err(Error::XsError);
            }
        }

        Ok(())
    }

    pub fn set_irq(&self, irq: u32) -> Result<()> {
        let ret = unsafe { xendevicemodel_set_irq_level(self.xdh, self.domid, irq, VIRTIO_IRQ_HIGH) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            Ok(())
        }
    }
}

impl Drop for XenDeviceModel {
    fn drop(&mut self) {
        self.ummap_io_range_from_ioreq_server().unwrap();
        self.set_ioreq_server_state(0).unwrap();
        self.destroy_ioreq_server().unwrap();
        unsafe{ xendevicemodel_close(self.xdh); }
    }
}
