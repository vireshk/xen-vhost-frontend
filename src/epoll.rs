// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};

use super::{Error, Result};

pub struct XenEpoll(Epoll);

impl XenEpoll {
    pub fn new(fds: Vec<i32>) -> Result<Self> {
        let epoll = Epoll::new().map_err(Error::EpollCreateFd)?;

        for fd in fds {
            epoll
                .ctl(
                    ControlOperation::Add,
                    fd,
                    EpollEvent::new(EventSet::IN, fd as u64),
                )
                .map_err(Error::RegisterExitEvent)?;
        }

        Ok(Self(epoll))
    }

    pub fn wait(&self) -> Result<i32> {
        let mut events = vec![EpollEvent::new(EventSet::empty(), 0); 1];

        loop {
            match self.0.wait(-1, &mut events[..]) {
                Ok(_) => {
                    return Ok(events[0].fd());
                }

                Err(e) => {
                    // It's well defined from the epoll_wait() syscall
                    // documentation that the epoll loop can be interrupted
                    // before any of the requested events occurred or the
                    // timeout expired. In both those cases, epoll_wait()
                    // returns an error of type EINTR, but this should not
                    // be considered as a regular error. Instead it is more
                    // appropriate to retry, by calling into epoll_wait().
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(Error::EpollWait(e));
                    }
                }
            }
        }
    }
}
