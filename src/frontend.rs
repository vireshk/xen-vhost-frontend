// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
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

    fn remove_guest(&mut self, fe_domid: u16) {
        self.0
            .remove(self.0.iter().position(|g| g.fe_domid == fe_domid).unwrap())
            .exit()
    }

    fn add_device(&mut self, fe_domid: u16, dev_id: u32) -> Result<Arc<XenDevice>> {
        let guest = match self.find_guest(fe_domid) {
            Some(guest) => guest,
            None => self.add_guest(fe_domid)?,
        };

        guest.add_device(dev_id)
    }

    fn remove_device(&mut self, fe_domid: u16, dev_id: u32) {
        let guest = self.find_guest(fe_domid).unwrap();
        guest.remove_device(dev_id).exit();

        if guest.is_empty() {
            self.remove_guest(fe_domid);
        }
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
        // TODO: We need some sign that all devid subdirs are already written to
        // Xenstore, so it's time to parse them. This delay although works, doesn't
        // guarantee that.
        thread::sleep(std::time::Duration::from_millis(400));

        let dev = self.guests.lock().unwrap().add_device(fe_domid, dev_id)?;

        // Device is ready to accept ioreq() updates now, lets enable that.
        dev.setup_ioreq()?;
        Ok(())
    }

    pub fn remove_device(&self, fe_domid: u16, dev_id: u32) {
        self.guests.lock().unwrap().remove_device(fe_domid, dev_id);
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
