// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use super::{device::XenDevice, guest::XenGuest, Result};

#[derive(Default)]
struct FrontendGuests(Vec<Arc<XenGuest>>);

impl FrontendGuests {
    fn find_guest(&self, fe_domid: u16) -> Option<Arc<XenGuest>> {
        self.0
            .iter()
            .find(|guest| guest.fe_domid == fe_domid)
            .cloned()
    }

    fn add_guest(&mut self, fe_domid: u16) -> Result<Arc<XenGuest>> {
        let guest = XenGuest::new(fe_domid)?;
        self.0.push(guest.clone());

        Ok(guest)
    }

    fn add_device(&mut self, fe_domid: u16, dev_id: u32) -> Result<Arc<XenDevice>> {
        let guest = match self.find_guest(fe_domid) {
            Some(guest) => guest,
            None => self.add_guest(fe_domid)?,
        };

        guest.add_device(dev_id)
    }
}

pub struct XenFrontend {
    guests: Mutex<FrontendGuests>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl XenFrontend {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            guests: Mutex::new(FrontendGuests::default()),
            threads: Mutex::new(Vec::new()),
        }))
    }

    pub fn add_device(&self, fe_domid: u16, dev_id: u32) -> Result<()> {
        self.guests.lock().unwrap().add_device(fe_domid, dev_id)?;
        Ok(())
    }

    pub fn push(&self, handle: JoinHandle<()>) {
        self.threads.lock().unwrap().push(handle)
    }
}

impl Drop for XenFrontend {
    fn drop(&mut self) {
        while let Some(handle) = self.threads.lock().unwrap().pop() {
            handle.join().unwrap();
        }
    }
}
