// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs::OpenOptions,
    io::Result as IoResult,
    os::unix::io::AsRawFd,
    sync::Arc,
    sync::Mutex,
    thread::{Builder, JoinHandle},
};

use vhost_user_frontend::{VirtioInterrupt, VirtioInterruptType};
use vmm_sys_util::eventfd::EventFd;

use super::{device::XenDevice, epoll::XenEpoll};

pub struct XenInterrupt {
    dev: Arc<XenDevice>,
    // Single EventFd is enough for any number of queues as there is a single underlying interrupt
    // to guest anyway.
    call: Option<EventFd>,
    handle: Mutex<Option<JoinHandle<()>>>,
    is_irqfd: bool,
}

impl XenInterrupt {
    pub fn new(dev: Arc<XenDevice>, is_irqfd: bool) -> Arc<Self> {
        let call = EventFd::new(0).unwrap();

        let xen_int = Arc::new(XenInterrupt {
            dev: dev.clone(),
            call: Some(call.try_clone().unwrap()),
            handle: Mutex::new(None),
            is_irqfd,
        });

        if is_irqfd {
            xen_int
                .dev
                .guest
                .xdm
                .lock()
                .unwrap()
                .set_irqfd(call, xen_int.dev.irq as u32, true)
                .unwrap()
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .open("/dev/virtio-msg-0")
                .unwrap();

            let fd = call.as_raw_fd();
            let epoll = XenEpoll::new(vec![fd]).unwrap();
            let xen_int2 = xen_int.clone();

            *xen_int.handle.lock().unwrap() = Some(
                Builder::new()
                .spawn(move || {
                    while let Ok(_) = epoll.wait() {
                        xen_int2.call.as_ref().unwrap().read().unwrap();
                        dev.mmio.lock().unwrap().send_event_used(&mut file);
                    }
                })
                .unwrap(),
            )
        }

        xen_int
    }

    pub fn exit(&self) {
        if self.is_irqfd {
            self.dev
                .guest
                .xdm
                .lock()
                .unwrap()
                .set_irqfd(self.call.as_ref().unwrap().try_clone().unwrap(), self.dev.irq as u32, false)
                .unwrap();
        }
    }
}

impl VirtioInterrupt for XenInterrupt {
    fn trigger(&self, _int_type: VirtioInterruptType) -> IoResult<()> {
        Ok(())
    }

    fn notifier(&self, _int_type: VirtioInterruptType) -> Option<EventFd> {
        Some(self.call.as_ref().unwrap().try_clone().unwrap())
    }
}
