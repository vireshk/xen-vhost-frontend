// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use libc::{MAP_SHARED, PROT_READ, PROT_WRITE};
use std::fs::OpenOptions;
use std::os::raw::c_void;

use vhost_user_frontend::{GuestMemoryMmap, GuestRegionMmap};
use vm_memory::{
    bitmap::AtomicBitmap, guest_memory::FileOffset, mmap::NewBitmap, GuestAddress,
    GuestMemoryAtomic,
};

use super::{xfm::XenForeignMemory, Error, Result};
#[cfg(target_arch = "aarch64")]
use xen_bindings::bindings::GUEST_RAM_BANKS;
use xen_bindings::bindings::XC_PAGE_SHIFT;
use xen_ioctls::xc_domain_info;

type MmapRegionBuilder = vm_memory::mmap::MmapRegionBuilder<AtomicBitmap>;

// GUEST_RAM_BANKS is only defined in public/arch-arm.h
#[cfg(target_arch = "x86_64")]
pub const GUEST_RAM_BANKS: u32 = 2;
pub const GUEST_RAM0_BASE: u64 = 0x40000000; // 3GB of low RAM @ 1GB
pub const GUEST_RAM0_SIZE: u64 = 0xc0000000;
pub const GUEST_RAM1_BASE: u64 = 0x0200000000;

fn get_dom_mem(domid: u16) -> Result<u64> {
    let info = xc_domain_info(domid, 1);

    if info.len() != 1 {
        Err(Error::InvalidDomainInfo(info.len(), domid, 0))
    } else if info[0].domid != domid {
        Err(Error::InvalidDomainInfo(
            info.len(),
            domid,
            info[0].domid as usize,
        ))
    } else {
        Ok((info[0].nr_pages - 4) << XC_PAGE_SHIFT)
    }
}

pub struct XenGuestMem {
    base: [u64; GUEST_RAM_BANKS as usize],
    size: [u64; GUEST_RAM_BANKS as usize],
    addr: [*mut c_void; GUEST_RAM_BANKS as usize],
    mem: Option<GuestMemoryAtomic<GuestMemoryMmap>>,
}

impl XenGuestMem {
    pub fn new(xfm: &mut XenForeignMemory, domid: u16) -> Result<Self> {
        let size = get_dom_mem(domid)?;
        let mut mem = XenGuestMem {
            base: [0; GUEST_RAM_BANKS as usize],
            size: [0; GUEST_RAM_BANKS as usize],
            addr: [std::ptr::null_mut(); GUEST_RAM_BANKS as usize],
            mem: None,
        };

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

        // TODO, handle case of divided address space
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/xen/privcmd")
            .unwrap();

        // SAFETY: Safe as addr and range are guaranteed to be valid here.
        let mmap_region = unsafe {
            MmapRegionBuilder::new_with_bitmap(
                size as usize,
                vm_memory::bitmap::AtomicBitmap::with_len(size as usize),
            )
            .with_raw_mmap_pointer(mem.addr[0] as *mut u8)
            .with_mmap_prot(PROT_READ | PROT_WRITE)
            .with_mmap_flags(MAP_SHARED)
            .with_file_offset(FileOffset::new(file, 0))
            .build()
            .unwrap()
        };

        let region = GuestRegionMmap::new(mmap_region, GuestAddress(mem.base[0])).unwrap();
        mem.mem = Some(GuestMemoryAtomic::new(
            GuestMemoryMmap::from_regions(vec![region]).unwrap(),
        ));

        Ok(mem)
    }

    pub fn mem(&self) -> GuestMemoryAtomic<GuestMemoryMmap> {
        self.mem.as_ref().unwrap().clone()
    }
}
