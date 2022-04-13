// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    io::Result,
    sync::{Arc, RwLock},
    thread::{spawn, JoinHandle},
};

use vhost_user_master::{VirtioInterrupt, VirtioInterruptType};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::XenState;
use libxen_sys::VIRTIO_MMIO_INT_VRING;

pub struct XenVirtioInterrupt {
    state: Arc<RwLock<XenState>>,
    // Single EventFd is enough for any number of queues as there is a single underlying interrupt
    // to guest anyway.
    call: EventFd,
}

impl XenVirtioInterrupt {
    pub fn new(state: Arc<RwLock<XenState>>) -> XenVirtioInterrupt {
        XenVirtioInterrupt {
            state,
            call: EventFd::new(EFD_NONBLOCK).unwrap(),
        }
    }

    pub fn call(&self) -> Result<u64> {
        self.call.read()
    }
}

impl VirtioInterrupt for XenVirtioInterrupt {
    fn trigger(&self, _int_type: VirtioInterruptType) -> Result<()> {
        let mut state = self.state.write().unwrap();

        // Update interrupt state
        state.mmio.update_interrupt_state(VIRTIO_MMIO_INT_VRING);

        // Raise interrupt to the guest
        state.xdm.set_irq(state.xsd.irq() as u32).unwrap();
        Ok(())
    }

    fn notifier(&self, _int_type: VirtioInterruptType) -> Option<EventFd> {
        Some(self.call.try_clone().unwrap())
    }
}

pub fn handle_interrupt(interrupt: Arc<XenVirtioInterrupt>) -> JoinHandle<()> {
    spawn(move || loop {
        // Wait for backend to notify
        if interrupt.call().is_ok() {
            interrupt.trigger(VirtioInterruptType::Queue(0)).unwrap();
        }
    })
}
