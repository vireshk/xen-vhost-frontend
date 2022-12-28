// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    io::Result as IoResult,
    os::unix::io::AsRawFd,
    sync::{Arc, Mutex},
    thread::{Builder, JoinHandle},
};

use vhost_user_frontend::{VirtioDevice, VirtioInterrupt, VirtioInterruptType};
use virtio_bindings::virtio_mmio::VIRTIO_MMIO_INT_VRING;
use vmm_sys_util::eventfd::EventFd;

use super::{device::XenDevice, epoll::XenEpoll, Result};

pub struct XenInterrupt {
    dev: Arc<XenDevice>,
    // Single EventFd is enough for any number of queues as there is a single underlying interrupt
    // to guest anyway.
    call: EventFd,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl XenInterrupt {
    pub fn new(dev: Arc<XenDevice>) -> Arc<Self> {
        Arc::new(XenInterrupt {
            dev,
            call: EventFd::new(0).unwrap(),
            handle: Mutex::new(None),
        })
    }

    fn as_raw_fd(&self) -> i32 {
        self.call.as_raw_fd()
    }

    fn clear_event(&self) {
        self.call.read().unwrap();
    }

    pub fn setup(self: Arc<Self>) -> Result<()> {
        let xfd = self.dev.xsh.fileno()?;
        let ifd = self.as_raw_fd();
        let epoll = XenEpoll::new(vec![xfd, ifd])?;
        let dev = self.dev.clone();
        let interrupt = self.clone();

        *self.handle.lock().unwrap() = Some(
            Builder::new()
                .name(format!("interrupt {}", dev.dev_id))
                .spawn(move || {
                    while let Ok(fd) = epoll.wait() {
                        if fd == ifd as i32 {
                            interrupt.trigger(VirtioInterruptType::Queue(0)).unwrap();
                        } else if dev.xs_event().is_err() {
                            dev.gdev.lock().unwrap().reset();
                            dev.gdev.lock().unwrap().shutdown();
                            break;
                        }
                    }
                })
                .unwrap(),
        );

        Ok(())
    }

    pub fn exit(&self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }
}

impl VirtioInterrupt for XenInterrupt {
    fn trigger(&self, _int_type: VirtioInterruptType) -> IoResult<()> {
        // Clear the eventfd from backend
        self.clear_event();

        // Update interrupt state
        self.dev
            .mmio
            .lock()
            .unwrap()
            .update_interrupt_state(VIRTIO_MMIO_INT_VRING);

        // Raise interrupt to the guest
        self.dev
            .guest
            .xdm
            .lock()
            .unwrap()
            .set_irq(self.dev.irq as u32)
            .unwrap();
        Ok(())
    }

    fn notifier(&self, _int_type: VirtioInterruptType) -> Option<EventFd> {
        Some(self.call.try_clone().unwrap())
    }
}
