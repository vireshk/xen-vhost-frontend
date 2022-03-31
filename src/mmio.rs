// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::mem;
use std::os::raw::c_void;
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};
use libxen_sys::*;
use super::{xdm::XenDeviceModel, xgm::XenGuestMem, Error, Result};

pub const VIRTIO_QUEUE_SIZE: u32 = 1024;
pub const VIRTIO_I2C_F_ZERO_LENGTH_REQUEST: u32 = 0;
pub const VIRTIO_MMIO_IO_SIZE: u64 = 0x200;

struct VirtQueue {
    pfn: u32,
    size: u32,
    size_max: u32,
    vring: vring,

    // Physical addresses
    phys_vring: vring,
    // Guest to Backend
    kick: EventFd,
    // Backend to Guest
    call: EventFd,
}

#[derive(Default)]
pub struct XenMmio {
    addr: u64,
    irq: u8,
    magic: [u8; 4],
    version: u8,
    device_id: u32,
    vendor_id: u32,
    status: u32,
    irq_status: u32,
    queue_sel: u32,
	host_features: u32,
	host_features_sel: u32,
	guest_features: u32,
	guest_features_sel: u32,
    guest_page_size: u32,
    queue_align: u32,
    interrupt_state: u32,
    queue: Vec<VirtQueue>,

    // Hack for main to wait.
    ready: Option<EventFd>,
}

impl XenMmio {
    pub fn new(xdm: &mut XenDeviceModel, addr: u64, irq: u8) -> Result<Self> {
        xdm.map_io_range_to_ioreq_server(addr, VIRTIO_MMIO_IO_SIZE)?;

        Ok (Self {
            addr,
            irq,
            magic: [b'v', b'i', b'r', b't'],
            version: 1,
            device_id: VIRTIO_ID_I2C_ADAPTER,
            vendor_id: 0x4d564b4c,
            queue: vec![VirtQueue {
                pfn: 0,
                size: 0,
                size_max: VIRTIO_QUEUE_SIZE,
                vring: unsafe { mem::zeroed() },
                phys_vring: unsafe { mem::zeroed() },
                kick: EventFd::new(EFD_NONBLOCK).unwrap(),
                call: EventFd::new(EFD_NONBLOCK).unwrap(),
            }],
            ready: None,
            ..Default::default()
        })
    }

    pub fn set_ready(&mut self, ready: EventFd) {
        self.ready = Some(ready)
    }

    pub fn irq(&self) -> u8 {
        self.irq
    }

    pub fn get_kick(&self, vq: u64) -> EventFd {
        self.queue[vq as usize].kick.try_clone().unwrap()
    }

    pub fn get_call(&self, vq: u64) -> EventFd {
        self.queue[vq as usize].call.try_clone().unwrap()
    }

    pub fn get_vring(&self, vq: u64) -> vring {
        self.queue[vq as usize].phys_vring.clone()
    }

    fn kick(&self, vq: u64) -> Result<()> {
        // Notify backend
        self.queue[vq as usize].kick.write(1).map_err(Error::EventFdWriteFailed)
    }

    pub fn update_interrupt_state(&mut self, mask: u32) {
        self.interrupt_state |= mask;
    }

    pub fn call(&self, vq: u64) {
        while self.queue[vq as usize].call.read().is_err() {}
    }

    fn handle_config_read(&self, _ioreq: &ioreq, _offset: u64) -> Result<()> {
        Ok(())
    }

    fn handle_config_write(&self, _ioreq: &ioreq, _offset: u64) -> Result<()> {
        Ok(())
    }

    fn handle_io_read(&self, ioreq: &mut ioreq, offset: u64) -> Result<()> {
        ioreq.data = match offset as u32 {
            VIRTIO_MMIO_MAGIC_VALUE => u32::from_le_bytes(self.magic),
            VIRTIO_MMIO_VERSION => self.version as u32,
            VIRTIO_MMIO_DEVICE_ID => self.device_id,
            VIRTIO_MMIO_VENDOR_ID => self.vendor_id,
            VIRTIO_MMIO_STATUS => self.status,
            VIRTIO_MMIO_INTERRUPT_STATUS => self.interrupt_state,
            VIRTIO_MMIO_QUEUE_PFN => self.queue[self.queue_sel as usize].pfn,
            VIRTIO_MMIO_QUEUE_NUM_MAX => self.queue[self.queue_sel as usize].size_max,

            VIRTIO_MMIO_DEVICE_FEATURES => {
                1 << VIRTIO_F_NOTIFY_ON_EMPTY
                    | 1 << VIRTIO_RING_F_INDIRECT_DESC
                    | 1 << VIRTIO_RING_F_EVENT_IDX
                    | 1 << VIRTIO_I2C_F_ZERO_LENGTH_REQUEST
            }

            _ => return Err(Error::XsError),
        } as u64;

        Ok(())
    }

    fn handle_io_write(&mut self, xgm: &XenGuestMem, ioreq: &ioreq, offset: u64) -> Result<()> {
        match offset as u32 {
            VIRTIO_MMIO_DEVICE_FEATURES_SEL => self.host_features_sel = ioreq.data as u32,
            VIRTIO_MMIO_DRIVER_FEATURES_SEL => self.guest_features_sel = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_SEL => self.queue_sel = ioreq.data as u32,
            VIRTIO_MMIO_STATUS => self.status = ioreq.data as u32,
            VIRTIO_MMIO_DRIVER_FEATURES => self.guest_features = ioreq.data as u32,
            VIRTIO_MMIO_GUEST_PAGE_SIZE => {
                self.guest_page_size = ioreq.data as u32;
                if self.guest_page_size != XC_PAGE_SIZE {
                    panic!();
                }
            }
            VIRTIO_MMIO_QUEUE_NUM => self.queue[self.queue_sel as usize].size = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_ALIGN => {
                self.queue_align = ioreq.data as u32;
                if self.queue_align != XC_PAGE_SIZE {
                    panic!();
                }
            }
            VIRTIO_MMIO_INTERRUPT_ACK => {
                self.interrupt_state &= !(ioreq.data as u32);
            }
            VIRTIO_MMIO_QUEUE_PFN => self.init_vq(xgm, ioreq.data),
            VIRTIO_MMIO_QUEUE_NOTIFY => {
                self.kick(ioreq.data)?;
            }

            _ => return Err(Error::XsError),
        }

        Ok(())
    }

    fn init_vq(&mut self, xgm: &XenGuestMem, pfn: u64) {
        if pfn == 0 {
            println!("Exit vq here");
            return;
        }

        let offset = pfn.checked_mul(self.guest_page_size as u64).unwrap();
        let queue = &mut self.queue[self.queue_sel as usize];
        queue.pfn = pfn as u32;

        queue.vring = vring_init(xgm.offset_to_addr(offset) as *mut c_void, queue.size, self.queue_align);
        queue.phys_vring = vring_init(offset as *mut c_void, queue.size, self.queue_align);

        self.ready.as_ref().unwrap().write(1).unwrap();
    }

    pub fn handle_ioreq(&mut self, xgm: &XenGuestMem, ioreq: &mut ioreq) -> Result<()> {
        match ioreq.type_ as u32 {
            IOREQ_TYPE_COPY => {
                let mut offset = ioreq.addr - self.addr;

                if offset >= VIRTIO_MMIO_CONFIG as u64 {
                    offset -= VIRTIO_MMIO_CONFIG as u64;

                    match ioreq.dir() as u32 {
                        IOREQ_READ => self.handle_config_read(ioreq, offset)?,
                        IOREQ_WRITE => self.handle_config_write(ioreq, offset)?,
                        _ => return Err(Error::XsError),
                    }
                }

                match ioreq.dir() as u32 {
                    IOREQ_READ => self.handle_io_read(ioreq, offset)?,
                    IOREQ_WRITE => self.handle_io_write(xgm, ioreq, offset)?,
                    _ => return Err(Error::XsError),
                }
            }

            IOREQ_TYPE_INVALIDATE => println!("Invalidate Ioreq type is Not implemented"),
            t => println!("Ioreq type unknown: {}", t),
        }
        Ok(())
    }
}

impl Drop for XenMmio {
    fn drop(&mut self) {
    }
}

fn vring_init(addr: *mut c_void, size: u32, align: u32)-> vring {
    let mut vring: vring = unsafe { mem::zeroed() };

    vring.num = size;
    vring.desc = addr as *mut vring_desc_t;
    vring.avail = unsafe { vring.desc.offset(size as isize) as *mut vring_avail_t };
    let used = unsafe { (*vring.avail).ring.as_mut_ptr().offset((size + 1) as isize) as *mut c_void };
    vring.used = unsafe { used.add(used.align_offset(align as usize)) as *mut vring_used_t };

    vring
}
