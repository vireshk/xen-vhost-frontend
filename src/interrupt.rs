// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    io::Result,
    os::unix::io::AsRawFd,
    sync::{Arc, RwLock},
    thread::{spawn, JoinHandle},
};

use vhost_user_frontend::{VirtioInterrupt, VirtioInterruptType};
use virtio_bindings::virtio_mmio::VIRTIO_MMIO_INT_VRING;
use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use vmm_sys_util::eventfd::EventFd;

use super::{Error, XenState};

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
            call: EventFd::new(0).unwrap(),
        }
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.call.as_raw_fd()
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

pub fn handle_interrupt(interrupt: Arc<XenVirtioInterrupt>, exit: EventFd) -> JoinHandle<()> {
    let epoll = Epoll::new().map_err(Error::EpollCreateFd).unwrap();

    // Handle Interrupt eventfd notifications
    epoll
        .ctl(
            ControlOperation::Add,
            interrupt.as_raw_fd(),
            EpollEvent::new(
                EventSet::IN | EventSet::ERROR | EventSet::HANG_UP,
                interrupt.as_raw_fd() as u64,
            ),
        )
        .unwrap();

    // Handle exit eventfd notifications
    epoll
        .ctl(
            ControlOperation::Add,
            exit.as_raw_fd(),
            EpollEvent::new(
                EventSet::IN | EventSet::ERROR | EventSet::HANG_UP,
                exit.as_raw_fd() as u64,
            ),
        )
        .unwrap();

    let mut events = vec![EpollEvent::new(EventSet::empty(), 0); 10];

    spawn(move || loop {
        match epoll.wait(-1, &mut events[..]) {
            Ok(num) => {
                for event in events.iter().take(num) {
                    if event.fd() == exit.as_raw_fd() {
                        return;
                    }

                    interrupt.trigger(VirtioInterruptType::Queue(0)).unwrap();
                }
            }

            Err(e) => {
                if e.kind() != std::io::ErrorKind::Interrupted {
                    // It's well defined from the epoll_wait() syscall
                    // documentation that the epoll loop can be interrupted
                    // before any of the requested events occurred or the
                    // timeout expired. In both those cases, epoll_wait()
                    // returns an error of type EINTR, but this should not
                    // be considered as a regular error. Instead it is more
                    // appropriate to retry, by calling into epoll_wait().
                    continue;
                }
                return;
            }
        }
    })
}
