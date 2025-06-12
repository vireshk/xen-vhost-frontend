// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::sync::{Arc, Mutex};

use super::{
    device::XenDevice,
    Result,
};

#[derive(Default)]
struct GuestDevices(Vec<Arc<XenDevice>>);

impl GuestDevices {
    fn push(&mut self, dev: Arc<XenDevice>) {
        self.0.push(dev);
    }
}

pub struct XenGuest {
    pub fe_domid: u16,
    devices: Mutex<GuestDevices>,
}

// SAFETY: Safe as the fields are protected with Mutex.
unsafe impl Send for XenGuest {}
// SAFETY: Safe as the fields are protected with Mutex.
unsafe impl Sync for XenGuest {}

impl XenGuest {
    pub fn new(fe_domid: u16) -> Result<Arc<Self>> {
        let guest = Arc::new(Self {
            fe_domid,
            devices: Mutex::new(GuestDevices::default()),
        });

        Ok(guest)
    }

    pub fn add_device(self: Arc<Self>, dev_id: u32) -> Result<Arc<XenDevice>> {
        let dev = XenDevice::new()?;
        self.devices.lock().unwrap().push(dev.clone());

        println!("Created device {} / {}", self.fe_domid, dev_id);
        Ok(dev)
    }
}
