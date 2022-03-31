// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::mem;
use std::os::raw::c_void;

use libxen_sys::*;
use super::{xc::XenCtrl, xfm::XenForeignMemory, Result};

pub const GUEST_RAM0_BASE: u64 = 0x40000000; // 3GB of low RAM @ 1GB
pub const GUEST_RAM0_SIZE: u64 = 0xc0000000;
pub const GUEST_RAM1_BASE: u64 = 0x0200000000;

pub struct XenGuestMem {
    base: [u64; GUEST_RAM_BANKS as usize],
    size: [u64; GUEST_RAM_BANKS as usize],
    addr: [*mut c_void; GUEST_RAM_BANKS as usize],
}

impl XenGuestMem {
    pub fn new(xfm: &mut XenForeignMemory, domid: domid_t) -> Result<Self> {
        let size = XenCtrl::new()?.get_dom_mem(domid)?;
        let mut mem: XenGuestMem = unsafe { mem::zeroed() };

        // #define-s below located at include/public/arch-arm.h
        mem.base[0] = GUEST_RAM0_BASE;
        if size <= GUEST_RAM0_SIZE {
            mem.size[0] = size;
        } else {
            mem.size[0] = GUEST_RAM0_SIZE;
            mem.base[1] = GUEST_RAM1_BASE;
            mem.size[1] = size - GUEST_RAM0_SIZE;
        }

        for i in 0..GUEST_RAM_BANKS as usize {
            if mem.base[i] != 0 && mem.size[i] != 0 {
                mem.addr[i] = xfm.map_mem(domid, mem.base[i], mem.size[i])?;
            }
        }

        Ok (mem)
    }

    pub fn addr_and_size(&self) -> (*mut c_void, u64, u64) {
        (self.addr[0], self.base[0], self.size[0])
    }

    pub fn offset_to_addr(&self, offset: u64) -> *mut c_void {
        for i in 0..GUEST_RAM_BANKS as usize {
            if self.addr[i].is_null() {
                continue;
            }

            if offset > self.base[i] && offset < self.base[i] + self.size[i] {
                return unsafe { self.addr[i].offset((offset - self.base[i]) as isize) };
            }
        }

        panic!("Failed to convert from offset to address: {}", offset)
    }
}
