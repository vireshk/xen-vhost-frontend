// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{io::Result as IoResult, sync::Arc};

use vhost_user_frontend::{VirtioInterrupt, VirtioInterruptType};
use vmm_sys_util::eventfd::EventFd;

use super::device::XenDevice;

pub struct XenInterrupt {
    dev: Arc<XenDevice>,
    // Single EventFd is enough for any number of queues as there is a single underlying interrupt
    // to guest anyway.
    call: EventFd,
}

impl XenInterrupt {
    pub fn new(dev: Arc<XenDevice>) -> Arc<Self> {
        let call = EventFd::new(0).unwrap();

        let xen_int = Arc::new(XenInterrupt {
            dev,
            call: call.try_clone().unwrap(),
        });

        xen_int
            .dev
            .guest
            .xdm
            .lock()
            .unwrap()
            .set_irqfd(call, xen_int.dev.irq as u32, true)
            .unwrap();

        xen_int
    }

    pub fn exit(&self) {
        self.dev
            .guest
            .xdm
            .lock()
            .unwrap()
            .set_irqfd(self.call.try_clone().unwrap(), self.dev.irq as u32, false)
            .unwrap();
    }
}

impl VirtioInterrupt for XenInterrupt {
    fn trigger(&self, _int_type: VirtioInterruptType) -> IoResult<()> {
        Ok(())
    }

    fn notifier(&self, _int_type: VirtioInterruptType) -> Option<EventFd> {
        Some(self.call.try_clone().unwrap())
    }
}
