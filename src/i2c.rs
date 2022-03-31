// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use vm_memory::mmap::NewBitmap;
use std::fs::OpenOptions;
use libc::{
    MAP_SHARED, PROT_READ, PROT_WRITE,
};
use vm_memory::guest_memory::FileOffset;


use vhost_user_master::vhost_user::vu_common_ctrl::VhostUserConfig;
use vhost_user_master::device::Generic;
use vhost_user_master::vhost_user::NoopVirtioInterrupt;
use vhost_user_master::vhost_user::{GuestMemoryMmap, GuestRegionMmap, MmapRegionBuilder};
use seccompiler::SeccompAction;
use std::sync::Arc;
use virtio_queue::{Queue, QueueState};
use vm_memory::{GuestAddress, GuestMemoryAtomic};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use std::os::raw::c_void;

use libxen_sys::*;
use vhost_user_master::vhost_user::VirtioDevice;

const QUEUE_SIZE: usize = 1024;
const NUM_QUEUES: usize = 1;

pub fn initialize(kick: EventFd, call: EventFd, vring: vring, vaddr: *mut c_void, addr: u64, size: u64) -> Generic {
    let vu_cfg = VhostUserConfig {
        socket: "/root/vi2c.sock0".to_string(),
        num_queues: NUM_QUEUES,
        queue_size: QUEUE_SIZE as u16,
    };

    println!("{:?}", (vring, vaddr, addr, size));
    let descriptor: u64 = vring.desc as *const c_void as u64;
    let used: u64 = vring.used as *const c_void as u64;
    let available: u64 = vring.avail as *const c_void as u64;

    let mut i2c = Generic::new(
        "none".to_string(),
        vu_cfg,
        SeccompAction::Allow,
        EventFd::new(EFD_NONBLOCK).unwrap(),
    ).unwrap();

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/xen/privcmd").unwrap();

    let mmap_region = unsafe{MmapRegionBuilder::new_with_bitmap(size as usize, vm_memory::bitmap::AtomicBitmap::with_len(size as usize))
        .with_raw_mmap_pointer(vaddr as *mut u8)
        .with_mmap_prot(PROT_READ | PROT_WRITE)
        .with_mmap_flags(MAP_SHARED)
        .with_file_offset( FileOffset::new(file, 0))
        .build().unwrap()};

    let region = GuestRegionMmap::new(mmap_region, GuestAddress(addr)).unwrap();
    let mem = GuestMemoryAtomic::new(
        GuestMemoryMmap::from_regions(vec![region]).unwrap()
    );

    let mut queue = Queue::<GuestMemoryAtomic<GuestMemoryMmap>, QueueState>::new(mem.clone(), QUEUE_SIZE as u16);
    queue.set_desc_table_address(Some((descriptor & 0xFFFFFFFF) as u32), Some((descriptor >> 32) as u32));
    queue.set_avail_ring_address(Some((available & 0xFFFFFFFF) as u32), Some((available >> 32) as u32));
    queue.set_used_ring_address(Some((used & 0xFFFFFFFF) as u32), Some((used >> 32) as u32));
    queue.set_next_avail(0 as u16);

    i2c.activate(
        mem.clone(),
        Arc::new(NoopVirtioInterrupt::new_with_eventfd(call)),
        vec![queue],
        vec![kick],
    ).unwrap();

    i2c
}
