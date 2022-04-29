// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use vhost::vhost_user::message::VHOST_USER_CONFIG_OFFSET;
use vhost_user_master::{Generic, GuestMemoryMmap, VirtioDevice};
use virtio_queue::{Queue, QueueState};
use vm_memory::{ByteValued, GuestMemoryAtomic};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::{xdm::XenDeviceModel, xgm::XenGuestMem, Error, Result};
use libxen_sys::{
    ioreq, IOREQ_READ, IOREQ_TYPE_COPY, IOREQ_TYPE_INVALIDATE, IOREQ_WRITE, VIRTIO_F_VERSION_1,
    VIRTIO_MMIO_CONFIG_GENERATION, VIRTIO_MMIO_DEVICE_FEATURES, VIRTIO_MMIO_DEVICE_FEATURES_SEL,
    VIRTIO_MMIO_DEVICE_ID, VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_MMIO_DRIVER_FEATURES_SEL,
    VIRTIO_MMIO_INTERRUPT_ACK, VIRTIO_MMIO_INTERRUPT_STATUS, VIRTIO_MMIO_MAGIC_VALUE,
    VIRTIO_MMIO_QUEUE_AVAIL_LOW, VIRTIO_MMIO_QUEUE_AVAIL_HIGH, VIRTIO_MMIO_QUEUE_DESC_LOW,
    VIRTIO_MMIO_QUEUE_DESC_HIGH, VIRTIO_MMIO_QUEUE_NOTIFY, VIRTIO_MMIO_QUEUE_NUM,
    VIRTIO_MMIO_QUEUE_NUM_MAX, VIRTIO_MMIO_QUEUE_READY, VIRTIO_MMIO_QUEUE_SEL, VIRTIO_MMIO_STATUS,
    VIRTIO_MMIO_QUEUE_USED_LOW, VIRTIO_MMIO_QUEUE_USED_HIGH, VIRTIO_MMIO_VENDOR_ID,
    VIRTIO_MMIO_VERSION,
};

pub const VIRTIO_MMIO_IO_SIZE: u64 = 0x200;

struct VirtQueue {
    ready: u32,
    size: u32,
    size_max: u32,
    desc_lo: u32,
    desc_hi: u32,
    avail_lo: u32,
    avail_hi: u32,
    used_lo: u32,
    used_hi: u32,
    queue: Option<Queue<GuestMemoryAtomic<GuestMemoryMmap>>>,

    // Guest to device
    kick: EventFd,
}

pub struct XenMmio {
    addr: u64,
    magic: [u8; 4],
    version: u8,
    vendor_id: u32,
    status: u32,
    queue_sel: u32,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    interrupt_state: u32,
    vq: Vec<VirtQueue>,

    // Indicates readiness of the virtqueue
    ready: EventFd,
}

impl XenMmio {
    pub fn new(xdm: &mut XenDeviceModel, dev: &Generic, addr: u64) -> Result<Self> {
        xdm.map_io_range_to_ioreq_server(addr, VIRTIO_MMIO_IO_SIZE)?;

        let mut mmio = Self {
            addr,
            magic: [b'v', b'i', b'r', b't'],
            version: 2,
            vendor_id: 0x4d564b4c,
            status: 0,
            queue_sel: 0,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            interrupt_state: 0,
            vq: Vec::new(),
            ready: EventFd::new(EFD_NONBLOCK).unwrap(),
        };

        for size in dev.queue_max_sizes() {
            mmio.vq.push(VirtQueue {
                ready: 0,
                size: 0,
                size_max: *size as u32,
                desc_lo: 0,
                desc_hi: 0,
                avail_lo: 0,
                avail_hi: 0,
                used_lo: 0,
                used_hi: 0,
                kick: EventFd::new(EFD_NONBLOCK).unwrap(),
                queue: None,
            });
        }

        Ok(mmio)
    }

    pub fn ready(&self) -> EventFd {
        self.ready.try_clone().unwrap()
    }

    pub fn get_kick(&self) -> Vec<EventFd> {
        let mut events = Vec::new();

        for vq in &self.vq {
            events.push(vq.kick.try_clone().unwrap());
        }

        events
    }

    pub fn queues(&self) -> Vec<Queue<GuestMemoryAtomic<GuestMemoryMmap>>> {
        let mut queues = Vec::new();

        for vq in &self.vq {
            queues.push(vq.queue.as_ref().unwrap().clone());
        }

        queues
    }

    fn kick(&self, vq: u64) -> Result<()> {
        // Notify backend
        self.vq[vq as usize]
            .kick
            .write(1)
            .map_err(Error::EventFdWriteFailed)
    }

    pub fn update_interrupt_state(&mut self, mask: u32) {
        self.interrupt_state |= mask;
    }

    fn handle_config_read(&self, ioreq: &mut ioreq, dev: &Generic, offset: u64) -> Result<()> {
        let mut data: u64 = 0;
        dev.read_config(offset, &mut data.as_mut_slice()[0..ioreq.size as usize]);
        ioreq.data = data;

        Ok(())
    }

    fn handle_config_write(&self, ioreq: &mut ioreq, dev: &mut Generic, offset: u64) -> Result<()> {
        dev.write_config(offset, &ioreq.data.to_ne_bytes()[0..ioreq.size as usize]);
        Ok(())
    }

    fn handle_io_read(&self, ioreq: &mut ioreq, dev: &Generic, offset: u64) -> Result<()> {
        let vq = &self.vq[self.queue_sel as usize];

        ioreq.data = match offset as u32 {
            VIRTIO_MMIO_MAGIC_VALUE => u32::from_le_bytes(self.magic),
            VIRTIO_MMIO_VERSION => self.version as u32,
            VIRTIO_MMIO_DEVICE_ID => dev.device_type() as u32,
            VIRTIO_MMIO_VENDOR_ID => self.vendor_id,
            VIRTIO_MMIO_STATUS => self.status,
            VIRTIO_MMIO_INTERRUPT_STATUS => self.interrupt_state,
            VIRTIO_MMIO_QUEUE_NUM_MAX => vq.size_max,
            VIRTIO_MMIO_DEVICE_FEATURES => {
                if self.device_features_sel > 1 {
                    return Err(Error::InvalidFeatureSel(self.device_features_sel));
                }

                let features = (1 << VIRTIO_F_VERSION_1) | dev.device_features();
                (features >> (32 * self.device_features_sel)) as u32
            }
            VIRTIO_MMIO_QUEUE_READY => vq.ready,
            VIRTIO_MMIO_QUEUE_DESC_LOW => vq.desc_lo,
            VIRTIO_MMIO_QUEUE_DESC_HIGH => vq.desc_hi,
            VIRTIO_MMIO_QUEUE_USED_LOW => vq.used_lo,
            VIRTIO_MMIO_QUEUE_USED_HIGH => vq.used_hi,
            VIRTIO_MMIO_QUEUE_AVAIL_LOW => vq.avail_lo,
            VIRTIO_MMIO_QUEUE_AVAIL_HIGH => vq.avail_hi,
            VIRTIO_MMIO_CONFIG_GENERATION => {
                // TODO
                0
            }

            _ => return Err(Error::InvalidMmioAddr("read", offset)),
        } as u64;

        Ok(())
    }

    fn handle_io_write(
        &mut self,
        ioreq: &ioreq,
        dev: &mut Generic,
        gm: &XenGuestMem,
        offset: u64,
    ) -> Result<()> {
        let vq = &mut self.vq[self.queue_sel as usize];
        match offset as u32 {
            VIRTIO_MMIO_DEVICE_FEATURES_SEL => self.device_features_sel = ioreq.data as u32,
            VIRTIO_MMIO_DRIVER_FEATURES_SEL => self.driver_features_sel = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_SEL => self.queue_sel = ioreq.data as u32,
            VIRTIO_MMIO_STATUS => self.status = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_NUM => vq.size = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_DESC_LOW => vq.desc_lo = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_DESC_HIGH => vq.desc_hi = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_USED_LOW => vq.used_lo = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_USED_HIGH => vq.used_hi = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_AVAIL_LOW => vq.avail_lo = ioreq.data as u32,
            VIRTIO_MMIO_QUEUE_AVAIL_HIGH => vq.avail_hi = ioreq.data as u32,
            VIRTIO_MMIO_INTERRUPT_ACK => {
                self.interrupt_state &= !(ioreq.data as u32);
            }
            VIRTIO_MMIO_DRIVER_FEATURES => {
                self.driver_features |=
                    ((ioreq.data as u32) as u64) << (32 * self.driver_features_sel);

                if self.driver_features_sel == 1 {
                    if (self.driver_features & (1 << VIRTIO_F_VERSION_1)) == 0 {
                        return Err(Error::MmioLegacyNotSupported);
                    }
                } else {
                    // Guest sends feature sel 1 first, followed by 0. Once that is done, lets
                    // negotiate features.
                    dev.negotiate_features(self.driver_features)
                        .map_err(Error::VhostMasterError)?;
                }
            }
            VIRTIO_MMIO_QUEUE_READY => {
                self.vq[self.queue_sel as usize].ready = ioreq.data as u32;
                if self.vq[self.queue_sel as usize].ready == 1 {
                    self.init_vq(gm);
                }
            }
            VIRTIO_MMIO_QUEUE_NOTIFY => self.kick(ioreq.data)?,

            _ => return Err(Error::InvalidMmioAddr("write", offset)),
        }

        Ok(())
    }

    fn init_vq(&mut self, gm: &XenGuestMem) {
        let vq = &mut self.vq[self.queue_sel as usize];

        let desc = ((vq.desc_hi as u64) << 32) | vq.desc_lo as u64 ;
        let avail = ((vq.avail_hi as u64) << 32) | vq.avail_lo as u64 ;
        let used = ((vq.used_hi as u64) << 32) | vq.used_lo as u64 ;

        if desc == 0 || avail == 0 || used == 0 {
            panic!();
        }

        let mut queue =
            Queue::<GuestMemoryAtomic<GuestMemoryMmap>, QueueState>::new(gm.mem(), vq.size as u16);
        queue.set_desc_table_address(Some((desc & 0xFFFFFFFF) as u32), Some((desc >> 32) as u32));
        queue.set_avail_ring_address(
            Some((avail & 0xFFFFFFFF) as u32),
            Some((avail >> 32) as u32),
        );
        queue.set_used_ring_address(Some((used & 0xFFFFFFFF) as u32), Some((used >> 32) as u32));
        queue.set_next_avail(0);

        vq.queue = Some(queue);

        // Wait for all virtqueues to get intialized.
        for vq in &self.vq {
            if vq.ready == 0 {
                return;
            }
        }

        self.ready.write(1).unwrap();
    }

    pub fn handle_ioreq(
        &mut self,
        ioreq: &mut ioreq,
        dev: &mut Generic,
        gm: &XenGuestMem,
    ) -> Result<()> {
        match ioreq.type_ as u32 {
            IOREQ_TYPE_COPY => {
                let mut offset = ioreq.addr - self.addr;

                if offset >= VHOST_USER_CONFIG_OFFSET as u64 {
                    offset -= VHOST_USER_CONFIG_OFFSET as u64;

                    match ioreq.dir() as u32 {
                        IOREQ_READ => self.handle_config_read(ioreq, dev, offset)?,
                        IOREQ_WRITE => self.handle_config_write(ioreq, dev, offset)?,
                        _ => return Err(Error::InvalidMmioDir(ioreq.dir())),
                    }
                } else {
                    match ioreq.dir() as u32 {
                        IOREQ_READ => self.handle_io_read(ioreq, dev, offset)?,
                        IOREQ_WRITE => self.handle_io_write(ioreq, dev, gm, offset)?,
                        _ => return Err(Error::InvalidMmioDir(ioreq.dir())),
                    }
                }
            }

            IOREQ_TYPE_INVALIDATE => println!("Invalidate Ioreq type is Not implemented"),
            t => println!("Ioreq type unknown: {}", t),
        }
        Ok(())
    }
}
